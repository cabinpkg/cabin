//! Wipe and recreate the registry's data from zero
//! (`api/docs/runbook.md`, "Wipe procedure (pre-launch only)"):
//!
//! ```text
//!   cargo registry-wipe            the deployed registry (asks to confirm)
//!   cargo registry-wipe --local    the local .wrangler/ state (smoke, dev)
//! ```
//!
//! Remote first empties the registry's four buckets - archives,
//! uploads, download logs and index - then drops and recreates the
//! database, bakes the recreated id into `wrangler.jsonc`, reapplies
//! every migration and redeploys.
//! `--local` is the same reset of the local emulated state.  Once
//! `meta.launched` is `true`, neither runs.
//!
//! Pre-launch only: [`launch_guard`] refuses once `meta.launched` is
//! `true`.  It runs AFTER the confirmation prompt and immediately before
//! anything destructive, so a flag flipped while the prompt sat waiting
//! still refuses.  On remote it also proves the `DB` binding and the
//! account's database named `registry` are the same database, so
//! the `d1 create` and `d1 list` below act on the database the guard
//! read and the `d1 delete` removes.
//!
//! **The guard is a function call.**  Spawning it through the
//! `registry-launch-guard` alias or `cargo run` would let the
//! environment replace what ran - `CARGO_ALIAS_<NAME>`, a `cargo`
//! earlier on `PATH`, `CARGO_TARGET_<TRIPLE>_RUNNER` - so the guard
//! could be made not to run at all.  [`launch_guard::run`] is called
//! directly, and no environment variable stands between the refusal and
//! the first destructive call.  What remains reachable through the
//! environment - `wrangler` through `node` on `PATH`, the R2 base
//! below - the guard does not defend against: it is a fail-safe
//! against a launched registry and a stale config, not against whoever
//! sets the environment.
//!
//! Properties:
//!
//! - **The `--local` reset deletes four subtrees and nothing else.**  The
//!   governor Durable Object's state and the emulated edge cache go with
//!   the D1 and R2 state: a wiped registry must not keep serving deleted
//!   archives.
//! - **The textual `wrangler.jsonc` rewrite targets the FIRST
//!   `database_id`**, which is only the `DB` binding's while it is the
//!   only one - so a count guards it beforehand, and a second count
//!   confirms the new id landed.
//! - **The R2 sweeps run before the database is dropped**, so a sweep
//!   that fails partway - a token that cannot list or delete, a refused
//!   key - leaves the launch guard's flag readable and a re-run of this
//!   command resumes it.
//! - **A sweep ends only on an empty listing** - see `sweep` below.
//! - **The stamp is refreshed from [`migrations_pending::stamp`]**, the
//!   deploy gate's own reading of `migrations/*.sql`, so it cannot
//!   drift from the gate's or from `cargo registry-migrate`'s.  The
//!   stamp file is rewritten on every path, emptied when the stamp
//!   cannot be computed, so a failure never leaves the pre-wipe stamp
//!   in place - see `stamp` below.
//!
//! Diagnostics split as [`crate::migrate`]'s do: every refusal this
//! module makes itself is `FAIL: <message>` on stderr and exit 1,
//! through its own `fail`.  An *incidental* failure - a wrangler
//! invocation that will not run, an unreadable `wrangler.jsonc`, a
//! `migrations/` that will not list - carries no such text and reports
//! through this crate's `Result` and the shim's `error:` prefix instead.
//!
//! Ceilings:
//!
//! - the R2 sweep's `ureq` agent does not read a proxy named only in
//!   the environment;
//! - a key the delete URL cannot carry verbatim - one with a `.` or
//!   `..` path segment, which URL parsing resolves away - cannot be
//!   swept: the run names it and stops before sending its delete.  No
//!   registry writer produces such a key, so one is an incident, and
//!   `wrangler r2 object delete` builds the same URL;
//! - the base of that API is `CF_API_BASE`, defaulting to Cloudflare's,
//!   so the environment names where `CLOUDFLARE_API_TOKEN` is sent;
//! - `wrangler.jsonc` is read as UTF-8; a config carrying invalid bytes
//!   ends the run;
//! - the four local state directories are removed in order and the
//!   first failure ends the run;
//! - every abort exits 1, even when a failed wrangler call exited
//!   otherwise;
//! - the confirmation is read as UTF-8: an answer carrying invalid bytes
//!   refuses through the read's own error, and one carrying NUL bytes
//!   refuses as "not confirmed";
//! - a confirmation that ends at end of input rather than at a newline
//!   refuses saying `FAIL: not confirmed`.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use xtask_workflow_guard::migrations_pending;

use crate::launch_guard::{self, Mode};
use crate::r2::Buckets;
use crate::{BUCKETS, declared_account_id, display, output, registry_root, status, step, wrangler};

/// The database this drops and recreates.
const DATABASE: &str = "registry";

/// This module's own refusal text on stderr, exit 1.
///
/// Held apart from the `Result` the incidental failures take, because
/// the shim renders those with its own `error:` prefix and these carry
/// bytes an operator reads.
fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1)
}

/// [`fail`] over a `Result` whose error is one of this module's own.
fn or_fail<T>(result: Result<T>) -> T {
    result.unwrap_or_else(|error| fail(&format!("{error:#}")))
}

/// Wipes and recreates the registry's data.
///
/// # Errors
///
/// On the incidental failures.  This module's own refusals leave
/// through `fail` instead.
pub fn run(mode: Mode) -> Result<()> {
    if matches!(mode, Mode::Remote) {
        // The one follow-up that must happen BEFORE the wipe, so it is
        // printed before the confirmation rather than with the others:
        // a ports publish landing after a bucket's sweep would leave an
        // object the recreated database has no row for.
        println!(
            "before wiping: gh workflow disable publish.yml -R cabin-ports/ports and gh \
             workflow disable registry-verify.yml, and cancel every queued or in-flight run of \
             either (api/docs/runbook.md, \"Post-wipe re-provisioning\")"
        );
        or_fail(confirm());
    }

    step("launch guard");
    launch_guard::run(mode)?;

    match mode {
        Mode::Local => local(),
        Mode::Remote => remote(),
    }
}

/// The local analogue of the whole remote procedure: the emulated D1,
/// R2, Durable Object and cache state under `.wrangler/` goes away, and
/// migrations recreate the schema from zero.  No config ids, no deploy.
fn local() -> Result<()> {
    let root = registry_root();
    step("deleting the local D1, R2, Durable Object, and cache state");
    for state in ["d1", "r2", "do", "cache"] {
        let path = root.join(".wrangler/state/v3").join(state);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            // A directory that is not there is already reset.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    step("reapplying migrations from zero");
    wrangler_run(
        &["d1", "migrations", "apply", "DB", "--local"],
        Show::Stdout,
    )?;

    println!("local wipe OK");
    Ok(())
}

fn remote() -> Result<()> {
    let root = registry_root();
    // The account id anchors the R2 REST calls below; it is a plain var
    // in `wrangler.jsonc` (not a secret - it is in every dashboard URL).
    let account = or_fail(account(&root));
    let token = std::env::var("CLOUDFLARE_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CLOUDFLARE_API_TOKEN is required for the R2 sweep");
    }
    let buckets = Buckets::new(&api_base(), &account, token);

    // The buckets go first, while the database - and with it the launch
    // guard's flag - still exists: a sweep that fails partway, a token
    // that cannot list or delete included, leaves nothing a re-run of
    // this command cannot finish.
    let mut deleted = 0;
    for bucket in BUCKETS {
        step(&format!("deleting every object from {bucket}"));
        let swept = or_fail(sweep(&buckets, bucket));
        println!("    deleted {swept} object(s); none is listed any more");
        deleted += swept;
    }

    step(&format!("dropping and recreating the {DATABASE} database"));
    wrangler_run(&["d1", "delete", DATABASE, "-y"], Show::Stdout)?;
    // The create says nothing, and the id is read from the listing
    // rather than from its output.
    wrangler_run(&["d1", "create", DATABASE], Show::Nothing)?;
    let listing = output(wrangler(&["d1", "list", "--json"]).current_dir(&root))?;
    let Some(new_id) = listed_id(&listing) else {
        fail("the recreated database is missing from d1 list");
    };
    if !database_id(&new_id) {
        fail(&format!("unexpected database id: '{new_id}'"));
    }

    // The binding deploys whatever `database_id` names, which must be
    // the recreated id before migrating and deploying
    // (`api/docs/runbook.md`).
    let path = root.join("wrangler.jsonc");
    let config =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if config.matches("\"database_id\"").count() != 1 {
        fail(
            "wrangler.jsonc carries more than one d1 binding; bake the DB binding's new id in by \
             hand",
        );
    }
    step(&format!(
        "baking the new database id into wrangler.jsonc ({new_id})"
    ));
    let baked = replace_id(&config, "database_id", &new_id);
    std::fs::write(&path, &baked).with_context(|| format!("write {}", path.display()))?;
    // Re-read the file this just wrote: the disk bytes, not this
    // process's copy, are what every wrangler call below consumes.
    let written =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if written.matches(new_id.as_str()).count() != 1 {
        fail("wrangler.jsonc does not carry the new id exactly once; fix it by hand");
    }

    step("applying all migrations from zero");
    wrangler_run(
        &["d1", "migrations", "apply", "DB", "--remote"],
        Show::Stdout,
    )?;

    // The database now runs exactly the files' content, so the deploy
    // gate's stamp is refreshed here rather than by hand (the pre-launch
    // policy edits migrations in place, which is exactly the state the
    // gate exists to block until a wipe like this one lands it).
    step("refreshing the migrations-applied stamp");
    stamp(&root)?;

    step("redeploying (bakes the new database id into the bindings)");
    wrangler_run(&["deploy"], Show::Stdout)?;

    print!(
        r#"wipe OK ({deleted} object(s) deleted)

Follow-ups, IN THIS ORDER (api/docs/runbook.md, "Post-wipe re-provisioning"):
  1. commit the wrangler.jsonc database-id change and the refreshed
     migrations-applied stamp to the pull request that changes the
     migrations (to one of their own when main already carries them)
     and merge it now: main still names the deleted database
  2. sign in again and re-claim scopes (/api/auth/claim/<scope>; a GitHub org's
     OAuth app grant survives the wipe, so re-claims grant immediately);
     the cabin-ports claim is what re-arms ports publishing - the
     trusted-publishing exchange refuses an unclaimed scope
  3. start the ports receipts log over while publishing is still
     disabled (its docs/publishing.md, "Registry wipe"), re-enable it (gh
     workflow enable publish.yml -R cabin-ports/ports), prove the
     binding with an exchange-check dispatch, then dispatch it plainly
     from main to republish the set - no publish token or secret to
     mint: each run's OIDC exchange is the credential; then, no sooner
     than 15 minutes after this wipe, delete whatever the
     registry-index bucket lists other than config.json and
     re-enable the verifier (gh workflow enable registry-verify.yml)
  4. rerun whatever main CI went red against the old registry
     (gh run rerun <id> --failed)
"#
    );
    Ok(())
}

/// The interactive confirmation, skipped when its escape hatch is set to
/// exactly `1`.  Remote only.
fn confirm() -> Result<()> {
    if std::env::var(cabin_env::CABIN_WIPE_YES).as_deref() == Ok("1") {
        return Ok(());
    }
    print!(
        "About to WIPE the deployed registry (the {DATABASE} database and every object in {}). \
         Type \"wipe\" to confirm: ",
        BUCKETS.join(", ")
    );
    std::io::stdout().flush().context("write the prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read the confirmation")?;
    if !confirmed(&answer) {
        bail!("not confirmed");
    }
    Ok(())
}

/// Whether one line of input is the confirmation: an answer that is not
/// newline-terminated is end of input and refuses; otherwise leading and
/// trailing spaces and tabs are stripped - and nothing else, so a CRLF
/// line's `\r` still refuses.
fn confirmed(answer: &str) -> bool {
    let Some(answer) = answer.strip_suffix('\n') else {
        return false;
    };
    answer.trim_matches([' ', '\t']) == "wipe"
}

/// The account id.  A missing file and a config that binds no id carry
/// the same message.
fn account(root: &Path) -> Result<String> {
    std::fs::read_to_string(root.join("wrangler.jsonc"))
        .ok()
        .and_then(|text| declared_account_id(&text))
        .ok_or_else(|| anyhow!("CF_ACCOUNT_ID not found in wrangler.jsonc"))
}

/// One entry of `wrangler d1 list --json`, whatever type each field
/// carries.
#[derive(Deserialize)]
struct Database {
    name: Option<serde_json::Value>,
    uuid: Option<serde_json::Value>,
    #[serde(rename = "database_id")]
    id: Option<serde_json::Value>,
}

/// `list.find((db) => db.name === "registry")` and then
/// `db.uuid || db.database_id`, rendered by `logged`.  The FIRST entry of
/// that name wins, as it does for the launch guard's own cross-check.
///
/// `||` is a *falsy* fallback, so an empty, null, zero or `false` `uuid`
/// falls through to `database_id` exactly as a missing one does.  A found
/// entry carrying neither yields the word `undefined`, which the id
/// grammar refuses; `None` is for no such database or an unparsable
/// listing - the two take different refusals.
fn listed_id(answer: &str) -> Option<String> {
    let databases: Vec<Database> = serde_json::from_str(answer).ok()?;
    let database = databases.into_iter().find(|database| {
        // `===` against a string: no coercion, so a numeric or absent
        // name is simply not this database.
        database.name.as_ref().and_then(serde_json::Value::as_str) == Some(DATABASE)
    })?;
    let held = |value: Option<serde_json::Value>| value.filter(truthy);
    Some(logged(
        held(database.uuid).or_else(|| held(database.id)).as_ref(),
    ))
}

/// A value as text, which is NOT [`display`]'s coercion: an array or an
/// object renders as its JSON text, never as the bare element, where
/// `String(["x"])` IS `"x"` - so routing one through [`display`] would
/// let an id wrapped in an array pass the id grammar.
fn logged(value: Option<&serde_json::Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => {
            value.to_string()
        }
        Some(value) => display(value),
    }
}

/// JavaScript truthiness over the values JSON can carry: `null`, `""`,
/// `0` (`-0` included) and `false` are the falsy ones, and every array or
/// object - `[]` included - is truthy.
fn truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(held) => *held,
        serde_json::Value::Number(number) => number.as_f64().is_some_and(|held| held.abs() > 0.0),
        serde_json::Value::String(text) => !text.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
    }
}

/// `^[0-9a-f-]{36}$`, which is also the shape `replace_id` replaces.
/// The same ceiling [`crate::declared_database_id`] carries: 36 hyphens
/// satisfy it, because what is being checked is that a config and an
/// account agree, and a malformed id agrees with nothing.
fn database_id(text: &str) -> bool {
    text.len() == 36
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

/// ``text.replace(/("<key>": ")[0-9a-f-]{36}(")/, `$1<id>$2`)``: the
/// FIRST occurrence that fits the shape, and no error when there is
/// none, which is what the counts around the call are for.  The
/// separator is the literal `": "` the regex spelled, one space and no
/// `\s`, so a config that formats its bindings otherwise is one this
/// leaves alone.
fn replace_id(text: &str, key: &str, id: &str) -> String {
    let needle = format!("\"{key}\": \"");
    let found = text.match_indices(&needle).find_map(|(index, matched)| {
        let value = index + matched.len();
        let held = text.get(value..value + 36)?;
        (database_id(held) && text.get(value + 36..=value + 36) == Some("\"")).then_some(value)
    });
    let Some(start) = found else {
        return text.to_owned();
    };
    format!("{}{id}{}", &text[..start], &text[start + 36..])
}

/// Writes [`migrations_pending::stamp`] into `migrations-applied`.
///
/// The file is written on every path, emptied when the stamp cannot be
/// computed, and only then does the run end: a stamp that is never read
/// again beats one silently left describing the pre-wipe schema.
fn stamp(root: &Path) -> Result<()> {
    let stamp = migrations_pending::stamp(root);
    let path = root.join("migrations-applied");
    let line = stamp
        .as_ref()
        .map_or_else(|_| String::new(), |stamp| format!("{stamp}\n"));
    std::fs::write(&path, line).with_context(|| format!("write {}", path.display()))?;
    stamp.map(drop)
}

/// The sweep: list the bucket, delete every listed key, and list again
/// until a listing comes back empty - the only way it ends without an
/// error.
///
/// Deleting drains the listing, so the first page is re-fetched each
/// time and no cursor is handled (opaque cursors carry URL-hostile
/// characters).  A key listed again after its delete answered success
/// ends the run rather than looping: a writer is putting it back.
fn sweep(buckets: &Buckets, bucket: &str) -> Result<u64> {
    let mut deleted = HashSet::new();
    loop {
        let keys = buckets.keys(bucket)?;
        if keys.is_empty() {
            return Ok(deleted.len() as u64);
        }
        for key in keys {
            if deleted.contains(&key) {
                bail!("{key} is still listed in {bucket} after its delete succeeded");
            }
            buckets.delete(bucket, &key)?;
            deleted.insert(key);
        }
    }
}

/// The Cloudflare API base.  `CF_API_BASE` is the seam that points
/// Cloudflare API calls at a local server.
fn api_base() -> String {
    std::env::var("CF_API_BASE")
        .ok()
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| crate::r2::API_BASE.to_owned())
}

/// Whether a wrangler call's stdout is the operator's sign of life or
/// discarded.
#[derive(Clone, Copy)]
enum Show {
    Stdout,
    Nothing,
}

fn wrangler_run(arguments: &[&str], show: Show) -> Result<()> {
    let mut command = wrangler(arguments);
    command.current_dir(registry_root());
    if matches!(show, Show::Nothing) {
        command.stdout(Stdio::null());
    }
    status(&mut command)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An id of the shape the account hands back, and a second one to
    /// tell a rewritten site from an untouched one.
    const NEW: &str = "1234abcd-1234-abcd-1234-abcd1234abcd";
    const OLD: &str = "00000000-0000-0000-0000-000000000000";

    /// The bucket the sweep tests empty, and one they must leave alone.
    const SWEPT: &str = BUCKETS[1];
    const OTHER: &str = BUCKETS[0];

    #[test]
    fn the_confirmation_is_one_blank_trimmed_newline_terminated_line() {
        assert!(confirmed("wipe\n"));
        // Leading and trailing blanks are stripped.
        assert!(confirmed("  wipe  \n"));
        assert!(confirmed("\twipe\t\n"));

        assert!(!confirmed("Wipe\n"));
        assert!(!confirmed("wipe it\n"));
        assert!(!confirmed("\n"));
        // A CRLF line's `\r` is kept: it is not a blank.
        assert!(!confirmed("wipe\r\n"));
        // End of input.
        assert!(!confirmed(""));
        assert!(!confirmed("wipe"));
    }

    #[test]
    fn the_listed_id_falls_back_past_a_falsy_uuid() {
        assert_eq!(
            listed_id(&format!(r#"[{{"name":"registry","uuid":"{NEW}"}}]"#)),
            Some(NEW.to_owned())
        );
        // Every falsy `uuid` falls through to `database_id`, a missing
        // one included.
        for falsy in ["\"\"", "null", "0", "false"] {
            assert_eq!(
                listed_id(&format!(
                    r#"[{{"name":"registry","uuid":{falsy},"database_id":"{NEW}"}}]"#
                )),
                Some(NEW.to_owned()),
                "uuid {falsy} is falsy"
            );
        }
        assert_eq!(
            listed_id(&format!(r#"[{{"name":"registry","database_id":"{NEW}"}}]"#)),
            Some(NEW.to_owned())
        );
        // A truthy `uuid` wins even when it is not an id at all, and the
        // grammar refuses it rather than reaching for the other field.
        assert_eq!(
            listed_id(&format!(
                r#"[{{"name":"registry","uuid":"nope","database_id":"{NEW}"}}]"#
            )),
            Some("nope".to_owned())
        );
        // The first entry of the name wins, as it does for the guard.
        assert_eq!(
            listed_id(&format!(
                r#"[{{"name":"registry","uuid":"{NEW}"}},
                    {{"name":"registry","uuid":"{OLD}"}}]"#
            )),
            Some(NEW.to_owned())
        );
        // Found, but carrying neither: the word `undefined`, which the
        // id grammar then refuses.
        assert_eq!(
            listed_id(r#"[{"name":"registry"}]"#),
            Some("undefined".to_owned())
        );
        assert!(!database_id("undefined"));
        // An id wrapped in an array is not that id: it renders as its
        // JSON text, which the id grammar refuses.
        let wrapped = listed_id(&format!(r#"[{{"name":"registry","uuid":["{NEW}"]}}]"#));
        assert!(!database_id(&wrapped.expect("the entry is found")));
        // No such database, and an answer that is not a listing at all:
        // the other refusal.
        assert_eq!(listed_id(r#"[{"name":"other","uuid":"x"}]"#), None);
        assert_eq!(listed_id(r#"[{"name":42,"uuid":"x"}]"#), None);
        assert_eq!(listed_id("not json"), None);
        assert_eq!(listed_id("[]"), None);
    }

    #[test]
    fn the_id_grammar_checks_the_alphabet_and_the_width() {
        assert!(database_id(NEW));
        // The ceiling: 36 hyphens satisfy it.
        assert!(database_id(&"-".repeat(36)));
        assert!(!database_id(&"a".repeat(35)));
        assert!(!database_id(&"a".repeat(37)));
        assert!(!database_id("1234ABCD-1234-abcd-1234-abcd1234abcd"));
        assert!(!database_id("1234abcg-1234-abcd-1234-abcd1234abcd"));
    }

    #[test]
    fn the_rewrite_lands_on_the_first_binding() {
        let bake = |config: &str, id: &str| replace_id(config, "database_id", id);
        let config = format!(
            r#"{{
  "d1_databases": [
    {{ "database_id": "{OLD}" }}
  ]
}}
"#
        );
        let baked = bake(&config, NEW);
        assert_eq!(baked.matches(NEW).count(), 1);
        assert_eq!(baked.matches(OLD).count(), 0);

        // A second `database_id` is left alone - the count guard before
        // the rewrite is what refuses that config in the first place.
        let two = format!("\"database_id\": \"{OLD}\"\n\"database_id\": \"{OLD}\"\n");
        let baked = bake(&two, NEW);
        assert_eq!(baked.matches(NEW).count(), 1);
        assert_eq!(baked.matches(OLD).count(), 1);

        // The separator is the literal `": "` the regex spelled, and the
        // value must fit the shape: none of these is a match, and the
        // once-present count is what catches it.
        for missed in [
            format!("\"database_id\":\"{OLD}\""),
            format!("\"database_id\":  \"{OLD}\""),
            "\"database_id\": \"short\"".to_owned(),
            format!("\"database_id\": \"{}\"", &OLD[1..]),
        ] {
            assert_eq!(bake(&missed, NEW), missed, "matched {missed}");
        }
        // A candidate that does not fit is skipped, not fatal, so a later
        // one still wins.
        let late = format!("\"database_id\": \"short\"\n\"database_id\": \"{OLD}\"\n");
        assert_eq!(bake(&late, NEW).matches(NEW).count(), 1);
    }

    /// A sweep over pages of two takes everything its bucket holds -
    /// keys under any prefix, keys that need escaping - and nothing
    /// another bucket holds, and a second sweep has nothing left.
    #[test]
    fn a_sweep_empties_its_bucket_and_no_other() {
        use crate::r2_mock::{Mock, State};
        let mock = Mock::start(State {
            page_cap: 2,
            ..State::holding(&[
                (SWEPT, "uploads/aa.zip"),
                (SWEPT, "uploads/bb.zip"),
                (SWEPT, "uploads/cc.zip"),
                (SWEPT, "rehearsal/notes.txt"),
                (SWEPT, "odd key/a+b%c?d#e&f=g;ü"),
                (SWEPT, "nested//empty/segment"),
                (SWEPT, "x"),
                (OTHER, "uploads/aa.zip"),
            ])
        });
        let buckets = mock.buckets();
        assert_eq!(sweep(&buckets, SWEPT).unwrap(), 7);
        assert_eq!(
            mock.remaining(),
            [(OTHER.to_owned(), "uploads/aa.zip".to_owned())]
        );
        assert_eq!(sweep(&buckets, SWEPT).unwrap(), 0);
    }

    /// No answer short of a successful empty last page ends a sweep, so
    /// a failed or malformed listing is never read as an emptied bucket.
    #[test]
    fn a_listing_that_proves_nothing_fails_the_sweep() {
        use crate::r2_mock::{Mock, State};
        for (status, body) in [
            (500, "boom"),
            (403, r#"{"success":false,"errors":[{"code":10000}]}"#),
            (200, "not json"),
            (200, ""),
            (200, r#"{"success":false,"result":[]}"#),
            (200, r#"{"success":true}"#),
            (200, r#"{"success":true,"result":[{"size":1}]}"#),
            (200, r#"{"success":true,"result":[{"key":42}]}"#),
            (
                200,
                r#"{"success":true,"result":[],"result_info":{"is_truncated":true}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[],"result_info":{"cursor":"next"}}"#,
            ),
            (200, r#"{"success":true,"result":[],"result_info":null}"#),
        ] {
            let mock = Mock::start(State {
                list_answer: Some((status, body.to_owned())),
                ..State::holding(&[(SWEPT, "uploads/aa.zip")])
            });
            assert!(sweep(&mock.buckets(), SWEPT).is_err(), "{status} {body}");
        }
    }

    /// A refused delete stops the sweep where it stands, and a re-run
    /// once the refusal clears finishes the job.
    #[test]
    fn a_refused_delete_stops_the_sweep_and_a_rerun_finishes_it() {
        use crate::r2_mock::{Mock, State};
        let mock = Mock::start(State {
            undeletable: Some("b".to_owned()),
            ..State::holding(&[(SWEPT, "a"), (SWEPT, "b"), (SWEPT, "c")])
        });
        let error = sweep(&mock.buckets(), SWEPT).unwrap_err();
        assert!(format!("{error:#}").contains("deleting b"), "{error:#}");
        assert_eq!(mock.remaining().len(), 2, "a is gone, b and c are not");
        mock.state().undeletable = None;
        assert_eq!(sweep(&mock.buckets(), SWEPT).unwrap(), 2);
        assert!(mock.remaining().is_empty());
    }

    /// A key a writer puts straight back ends the sweep naming the key
    /// instead of looping forever.
    #[test]
    fn a_delete_that_succeeds_without_deleting_ends_the_sweep() {
        use crate::r2_mock::{Mock, State};
        let mock = Mock::start(State {
            sticky: Some("uploads/aa.zip".to_owned()),
            ..State::holding(&[(SWEPT, "uploads/aa.zip")])
        });
        let error = sweep(&mock.buckets(), SWEPT).unwrap_err();
        assert!(
            error.to_string().contains("uploads/aa.zip is still listed"),
            "{error:#}"
        );
    }

    /// A key whose delete URL would resolve to another path of the API -
    /// `../..` from a key is another bucket's own URL - is refused
    /// before any delete goes out.
    #[test]
    fn a_dot_segment_key_is_refused_before_its_delete_is_sent() {
        use crate::r2_mock::{Mock, State};
        for key in ["../../registry-archives", "uploads/./a", "uploads/.."] {
            let mock = Mock::start(State::holding(&[(SWEPT, key)]));
            let error = sweep(&mock.buckets(), SWEPT).unwrap_err();
            assert!(error.to_string().contains("dot segment"), "{error:#}");
            assert!(
                !mock
                    .state()
                    .log
                    .iter()
                    .any(|line| line.starts_with("DELETE")),
                "{key}"
            );
        }
    }

    #[test]
    fn the_stamp_is_the_glob_concatenated() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let root = temp.path();
        let migrations = root.join("migrations");
        std::fs::create_dir_all(&migrations).expect("the migrations directory");
        std::fs::write(migrations.join("0001_a.sql"), b"one").expect("a migration");
        std::fs::write(migrations.join("0002_b.sql"), b"two").expect("a migration");
        // Outside the glob, exactly as the deploy gate reads it.
        std::fs::write(migrations.join(".draft.sql"), b"draft").expect("a dotfile");
        std::fs::write(migrations.join("notes.txt"), b"notes").expect("a stray");

        stamp(root).expect("the stamp");
        let stamped = || std::fs::read_to_string(root.join("migrations-applied")).expect("a stamp");
        // `printf onetwo | shasum -a 256`.
        assert_eq!(
            stamped(),
            "25b6746d5172ed6352966a013d93ac846e1110d5a25e8f183b5931f4688842a1\n"
        );

        // An empty selection has no stamp, and the pre-wipe one is gone.
        std::fs::remove_dir_all(&migrations).expect("an empty selection");
        std::fs::create_dir_all(&migrations).expect("the migrations directory");
        stamp(root).expect_err("nothing to stamp");
        assert_eq!(stamped(), "");
    }
}
