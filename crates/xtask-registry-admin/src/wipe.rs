//! Wipe and recreate the registry's data from zero
//! (`registry/docs/runbook.md`, "Wipe procedure (pre-launch only)"),
//! ported one-to-one from `registry/scripts/wipe.sh`:
//!
//! ```text
//!   cargo registry-wipe            the deployed registry (asks to confirm)
//!   cargo registry-wipe --local    the local .wrangler/ state (smoke, dev)
//! ```
//!
//! Remote drops and recreates the database, reapplies every migration,
//! bakes the recreated id into `wrangler.jsonc`, deletes the primary
//! bucket's archive blobs, bumps the registry generation and redeploys.
//! `--local` resets the entire local emulated state, the emulated backup
//! bucket included - local state is test data, not a backup; the
//! append-only invariant protects the deployed BACKUP bucket only, and
//! nothing here ever touches it.
//!
//! Pre-launch only: [`launch_guard`] refuses once `meta.launched` is
//! `true`.  It runs AFTER the confirmation prompt and immediately before
//! anything destructive, so a flag flipped while the prompt sat waiting
//! still refuses.  On remote it also proves the `DB` binding and the
//! account's database named `cabin-registry` are the same database, so
//! the reads here and the `d1 delete` below cannot diverge.
//!
//! **The guard is a function call, and that is the point of this port.**
//! The shell reached it through `(cd .. && cargo run … -- launch-guard)`,
//! spelled out rather than through the `registry-launch-guard` alias
//! because a Cargo alias is overridable by `CARGO_ALIAS_<NAME>` in the
//! environment.  That spelling closed one hole and left another: a
//! `cargo` earlier on `PATH`, or `CARGO_TARGET_<TRIPLE>_RUNNER`, still
//! replaced what ran, so the guard could be made not to run at all.  The
//! shell guard before it had the same shape of hole through `npx`.  The
//! recorded decision is not to carry the hop forward in any form: the
//! guard is this crate, so [`launch_guard::run`] is called directly and
//! no environment variable stands between the refusal and the `rm -rf`.
//! What remains reachable through the environment - `wrangler` through
//! `npx`, the R2 base below - is what the shell also had; the guard is a
//! fail-safe against a launched registry and a stale config, not against
//! whoever sets the environment.
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior.  Each was pinned by running the original
//! under `bash`:
//!
//! - **The generation is read before the wipe and written after it.**
//!   Every authenticated response echoes `x-cabin-registry-generation`,
//!   so clients and smoke runs can tell the wipe happened; a run that
//!   read it afterwards would have nothing to bump.
//! - **That read arrives through `console.log`**, not through `String()`.
//!   The two coercions differ on exactly the values the numeric guard
//!   exists to catch - see `logged` below.
//! - **`$((old + 1))` is bash arithmetic**, which is 64-bit and wrapping,
//!   and which reads a leading zero as octal - see `increment` below.
//! - **The `--local` reset deletes four subtrees and nothing else.**  The
//!   governor Durable Object's ledger and the emulated edge cache go with
//!   the D1 and R2 state: a wiped registry must not keep accounting for
//!   (or serving) deleted blobs.
//! - **The textual `wrangler.jsonc` rewrite targets the FIRST
//!   `database_id`**, which is only the `DB` binding's while it is the
//!   only one - so a count guards it beforehand, and a second count
//!   confirms the new id landed on both sites.
//! - **The R2 sweep drains the listing by re-fetching the first page**
//!   until nothing matches the prefix.  Deleting drains it, so no cursor
//!   is handled at all (opaque cursors carry URL-hostile characters).
//! - **The stamp is refreshed from [`migration_files`]**, the deploy
//!   gate's own glob rule, so this reading of `migrations/*.sql` cannot
//!   drift from the gate's or from `cargo registry-migrate`'s.  Its
//!   redirection truncated `migrations-applied` before the pipeline ran,
//!   so a failing `cat` still left the digest of what it did deliver -
//!   see `stamp` below.
//!
//! Diagnostics split the way `docs/architecture.md` draws the line for a
//! ported script, as [`crate::migrate`]'s do: every refusal the script
//! wrote through `lib.sh`'s
//! `fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }` keeps those bytes,
//! through this module's own `fail`.  An *incidental* failure the shell
//! died on under `set -e` - a wrangler invocation that will not run, an
//! unreadable `wrangler.jsonc`, a `migrations/` that will not list -
//! carries no such text and reports through this crate's `Result` and
//! the shim's `error:` prefix instead.
//!
//! Ceilings, where this deliberately stops short of the shell.  All keep
//! the exit code and the direction of every refusal:
//!
//! - the R2 sweep goes through the crate's own HTTP layer
//!   (the audit's pinned `ureq` agent) rather than `curl`, which is the
//!   curl-versus-library decision this crate already made for R2.
//!   `curl`'s flags and diagnostics are not reproduced: a proxy named
//!   only in the environment is not read, and a transfer failure carries
//!   `ureq`'s wording under the script's own `FAIL:` line;
//! - the base of that API is `CF_API_BASE`, defaulting to the literal the
//!   script hardcoded.  It is the same variable the Worker reads
//!   (`registry/src/backup_glue.rs`) and the smoke run already points at
//!   a local mock.  It also means the environment
//!   names where `CLOUDFLARE_API_TOKEN` is sent - as it did for the
//!   shell, whose `curl` a `curl` earlier on `PATH` answered for;
//! - the `node` hops are gone: the D1 and R2 answers are parsed here.
//!   The refusal messages are the script's, but the `TypeError` or
//!   `SyntaxError` underneath one is not reproduced.  Wrangler's own
//!   stderr still reaches the operator's terminal;
//! - the comment in `registry/migrations/0001_init.sql` still names
//!   `scripts/wipe.sh`.  Applied migrations are byte-frozen: the
//!   deploy gate compares `migrations-applied` against the files'
//!   content, so editing that comment would block every deploy until
//!   an operator re-runs this very command against the live database.
//!   The wording follows the next baseline edit that ships through a
//!   wipe, which re-stamps as part of the procedure;
//! - `wrangler.jsonc` is read as UTF-8.  A config carrying invalid bytes
//!   ends the run, where `node`'s `readFileSync(…, "utf8")` replaced them
//!   with U+FFFD and wrote that corruption back;
//! - the four `rm -rf` targets are removed in order and the first failure
//!   ends the run, where `rm -rf` attempted all four and then died.  A
//!   target that is a symlink to a directory refuses here and was
//!   unlinked there;
//! - the argument surface is clap's, in the binary: the mode arrives
//!   here already parsed, and anything malformed is refused before this
//!   module runs;
//! - every abort exits 1, where `set -e` propagated wrangler's own status
//!   from a failed call.  Nothing reads the distinction;
//! - the confirmation is read as UTF-8: an answer carrying invalid bytes
//!   refuses through the read's own error, and one carrying NUL bytes
//!   refuses as "not confirmed", where bash dropped the NULs before
//!   comparing and could confirm on `wi\0pe`.  Both refuse where the
//!   shell also refused on everything an operator can type;
//! - a confirmation that ends at end of input rather than at a newline
//!   refuses saying `FAIL: not confirmed`, where `set -e` ended the
//!   script at the failing `read` with nothing on stderr.  Both exit 1
//!   without touching anything.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use xtask_workflow_guard::migrations_pending::migration_files;

use crate::launch_guard::{self, Mode};
use crate::{
    BLOBS_BUCKET, declared_account_id, display, output, registry_root, results, status, step,
    wrangler,
};

/// The database this drops and recreates.
const DATABASE: &str = "cabin-registry";

const GENERATION: &str = "SELECT value FROM meta WHERE key = 'registry_generation'";

/// What the script hardcoded, and what `CF_API_BASE` overrides.
const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// `lib.sh`'s `fail`: the script's own refusal text on stderr, exit 1.
///
/// Held apart from the `Result` the incidental failures take, because
/// the shim renders those with its own `error:` prefix and these carry
/// bytes an operator reads.
fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1)
}

/// [`fail`] over a `Result` whose error is one of the script's own.
fn or_fail<T>(result: Result<T>) -> T {
    result.unwrap_or_else(|error| fail(&format!("{error:#}")))
}

/// Wipes and recreates the registry's data.
///
/// # Errors
///
/// On the incidental failures the shell died on under `set -e`.  The
/// script's own refusals leave through `fail` instead.
pub fn run(mode: Mode) -> Result<()> {
    if matches!(mode, Mode::Remote) {
        // The one follow-up that must happen BEFORE the wipe, so it is
        // printed before the confirmation rather than with the others:
        // a ports-publish run mints its own publish token through the
        // trusted-publishing exchange, and the post-wipe governor wipe
        // refuses while any live publish token exists.
        println!(
            "before wiping: gh workflow disable ports-publish.yml, and cancel any \
             in-flight ports-publish run (docs/runbook.md, \"Post-wipe re-provisioning\")"
        );
        or_fail(confirm());
    }

    step("launch guard");
    launch_guard::run(mode)?;

    step("reading the pre-wipe registry generation");
    let old = generation(mode)?;
    if !numeric(&old) {
        fail(&format!("meta.registry_generation is not numeric: '{old}'"));
    }
    let new = increment(&old)?;

    match mode {
        Mode::Local => local(&old, new),
        Mode::Remote => remote(&old, new),
    }
}

/// The local analogue of the whole remote procedure: the emulated D1 and
/// R2 state under `.wrangler/` simply goes away, and migrations recreate
/// the schema from zero.  No config ids, no deploy.
fn local(old: &str, new: i64) -> Result<()> {
    let root = registry_root();
    step("deleting the local D1, R2, Durable Object, and cache state");
    for state in ["d1", "r2", "do", "cache"] {
        let path = root.join(".wrangler/state/v3").join(state);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            // `rm -rf` says nothing about what is not there.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    step("reapplying migrations from zero");
    wrangler_run(
        &["d1", "migrations", "apply", "DB", "--local"],
        Show::Stdout,
    )?;

    step(&format!("bumping the registry generation to {new}"));
    bump(Mode::Local, new)?;

    println!("local wipe OK (generation {old} -> {new})");
    Ok(())
}

fn remote(old: &str, new: i64) -> Result<()> {
    let root = registry_root();
    // The account id anchors the R2 REST calls below; it is a plain var
    // in `wrangler.jsonc` (not a secret - it is in every dashboard URL).
    let account = or_fail(account(&root));
    // `: "${CLOUDFLARE_API_TOKEN:?…}"`, which bash refused for an unset
    // AND an empty value, naming the script's own path and line.
    let token = std::env::var("CLOUDFLARE_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CLOUDFLARE_API_TOKEN is required for the R2 sweep");
    }

    step(&format!("dropping and recreating the {DATABASE} database"));
    wrangler_run(&["d1", "delete", DATABASE, "-y"], Show::Stdout)?;
    // The script's `>/dev/null`: the create says nothing, and the id is
    // read from the listing rather than from its output.
    wrangler_run(&["d1", "create", DATABASE], Show::Nothing)?;
    let listing = output(wrangler(&["d1", "list", "--json"]).current_dir(&root))?;
    let Some(new_id) = listed_id(&listing) else {
        fail("the recreated database is missing from d1 list");
    };
    if !database_id(&new_id) {
        fail(&format!("unexpected database id: '{new_id}'"));
    }

    // The nightly dump exports whatever `D1_DATABASE_ID` names, and the
    // binding deploys whatever `database_id` names - both must be the
    // recreated id before migrating and deploying (docs/runbook.md).
    let path = root.join("wrangler.jsonc");
    let config =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if lines_holding(&config, "\"database_id\"") != 1 {
        fail(
            "wrangler.jsonc carries more than one d1 binding; bake the DB binding's new id in by \
             hand",
        );
    }
    step(&format!(
        "baking the new database id into wrangler.jsonc ({new_id})"
    ));
    let baked = bake(&config, &new_id);
    std::fs::write(&path, &baked).with_context(|| format!("write {}", path.display()))?;
    // `grep -c` re-read the file this just wrote, and counted LINES: a
    // config binding both ids on one line counts once and refuses. The
    // re-read matters - the disk bytes, not this process's copy, are
    // what every wrangler call below consumes.
    let written =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if lines_holding(&written, &new_id) != 2 {
        fail("wrangler.jsonc does not carry the new id exactly twice; fix it by hand");
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

    step(&format!("deleting blobs/ from {BLOBS_BUCKET}"));
    let deleted = sweep(&account, &token);
    println!("    deleted {deleted} blob(s)");

    step(&format!("bumping the registry generation to {new}"));
    bump(Mode::Remote, new)?;

    step("redeploying (bakes the new database id into the bindings)");
    wrangler_run(&["deploy"], Show::Stdout)?;

    print!(
        "wipe OK (generation {old} -> {new})

Follow-ups, IN THIS ORDER (docs/runbook.md, \"Post-wipe re-provisioning\"):
  1. commit the wrangler.jsonc database-id change and the refreshed
     migrations-applied stamp
  2. sign in again and re-claim scopes (/claim/<scope>; a GitHub org's
     OAuth app grant survives the wipe, so re-claims grant immediately);
     the cabin-ports claim is what re-arms ports publishing - the
     trusted-publishing exchange refuses an unclaimed scope
  3. mint a login-session token for the governor step below as the
     operator - only the operator's session carries the verify scope that
     authenticates the admin endpoint; the verifier workflow needs no
     secret of its own -
     each run mints its own through the trusted-publishing exchange, and
     the baseline migration seeds the backing identity it resolves
  4. run cargo registry-governor wipe (from the repository root); its
     no-delayed-publisher evidence gate requires zero live publish tokens
     and a login-session token carries publish, so block writes first,
     wait out the in-flight window (~5 min), then run under writes_blocked
     - the gate clears on that path; ports-publish stays disabled until
     the next step
  5. re-enable ports-publish (gh workflow enable ports-publish.yml),
     prove the binding with an exchange-check dispatch, then dispatch
     it plainly from main to republish the set - no publish token or
     secret to mint: each run's OIDC exchange is the credential
  6. rerun whatever main CI went red against the old registry
     (gh run rerun <id> --failed)
"
    );
    Ok(())
}

/// The interactive confirmation, skipped when its escape hatch is set to
/// exactly `1`.  Remote only: the shell prompted for the deployed
/// registry and never for the local state.
fn confirm() -> Result<()> {
    if std::env::var(cabin_env::CABIN_WIPE_YES).as_deref() == Ok("1") {
        return Ok(());
    }
    print!(
        "About to WIPE the deployed registry ({DATABASE}, {BLOBS_BUCKET}). \
         Type \"wipe\" to confirm: "
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

/// Whether one line of input is the confirmation, as `read -r answer`
/// followed by `[[ "$answer" == "wipe" ]]` judged it: an answer that is
/// not newline-terminated is end of input, where `read` failed and
/// `set -e` ended the run before the comparison; otherwise the default
/// `IFS` strips leading and trailing spaces and tabs - and nothing else,
/// so a CRLF line's `\r` still refuses.
fn confirmed(answer: &str) -> bool {
    let Some(answer) = answer.strip_suffix('\n') else {
        return false;
    };
    answer.trim_matches([' ', '\t']) == "wipe"
}

/// The pre-wipe generation, as the script's `node` projection printed
/// it.  A missing row, an unparsable answer or an answer that is not the
/// documented shape reached `console.log` through a `TypeError` there,
/// which failed the pipeline under `set -e` before the numeric guard ran
/// at all - so each is an incidental failure here, not the script's own
/// refusal.
fn generation(mode: Mode) -> Result<String> {
    let answer = output(
        wrangler(&[
            "d1",
            "execute",
            "DB",
            mode.flag(),
            "--json",
            "--command",
            GENERATION,
        ])
        .current_dir(registry_root()),
    )?;
    let rows = results(&answer)?;
    let row = rows
        .first()
        .context("meta.registry_generation has no row")?;
    Ok(logged(row.get("value")))
}

/// `console.log(out[0].results[0].value)`, which is NOT [`display`]'s
/// coercion.  `console.log` renders an array or an object through
/// `util.inspect` (`[ '1' ]`, `{ value: 1 }`), never as the bare element,
/// where `String(["1"])` IS `"1"` - so routing one through [`display`]
/// would let `["1"]` pass the numeric guard and wipe against a generation
/// the shell refused.  The JSON text used for those here is not Node's
/// inspect text either, which is the ceiling: both fail the guard, and
/// failing it is all a non-primitive value is ever read for.
fn logged(value: Option<&serde_json::Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => {
            value.to_string()
        }
        Some(value) => display(value),
    }
}

/// `[[ "$old_generation" =~ ^[0-9]+$ ]]`.
fn numeric(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `$((old_generation + 1))` as bash computed it.  Bash arithmetic is
/// 64-bit and wraps - while reading a literal too wide to hold, and
/// again at the signed edge - and it reads a literal with a leading zero
/// as octal, so `010` increments to 9 and `08` is not a number at all
/// (bash diagnosed it and `set -e` ended the run, which is the incidental
/// failure this returns).  The numeric guard admits all three, so this is
/// where the distinction is made; no generation the registry itself
/// writes carries a leading zero.
fn increment(digits: &str) -> Result<i64> {
    let radix = if digits.len() > 1 && digits.starts_with('0') {
        8
    } else {
        10
    };
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        let digit = byte.checked_sub(b'0').map_or(radix, i64::from);
        if digit >= radix {
            bail!("meta.registry_generation is not a base-{radix} number: '{digits}'");
        }
        value = value.wrapping_mul(radix).wrapping_add(digit);
    }
    Ok(value.wrapping_add(1))
}

/// The account id, matched the way the shell's regex matched it.  The
/// script reached both a missing file and a config that binds no id
/// through one `|| fail`, so both carry that one message here.
fn account(root: &Path) -> Result<String> {
    std::fs::read_to_string(root.join("wrangler.jsonc"))
        .ok()
        .and_then(|text| declared_account_id(&text))
        .ok_or_else(|| anyhow!("CF_ACCOUNT_ID not found in wrangler.jsonc"))
}

/// One entry of `wrangler d1 list --json`, read as JavaScript read it:
/// whatever type each field carries.
#[derive(Deserialize)]
struct Database {
    name: Option<serde_json::Value>,
    uuid: Option<serde_json::Value>,
    #[serde(rename = "database_id")]
    id: Option<serde_json::Value>,
}

/// `list.find((db) => db.name === "cabin-registry")` and then
/// `console.log(db.uuid || db.database_id)`.  The FIRST entry of that
/// name wins, as it does for the launch guard's own cross-check.
///
/// `||` is a *falsy* fallback, so an empty, null, zero or `false` `uuid`
/// falls through to `database_id` exactly as a missing one does.  A found
/// entry carrying neither printed the word `undefined` and reached the id
/// grammar, which is why that is `Some("undefined")` here and `None` only
/// for no such database at all - the two took different refusals in the
/// script.
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

/// `[[ "$new_id" =~ ^[0-9a-f-]{36}$ ]]`, and the alphabet the rewrite's
/// own regex matched.  The same ceiling [`crate::declared_database_id`]
/// carries: 36 hyphens satisfy it, because what is being checked is that
/// a config and an account agree, and a malformed id agrees with nothing.
fn database_id(text: &str) -> bool {
    text.len() == 36
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

/// `grep -c <needle> <file>`, which counts LINES holding it rather than
/// occurrences.  Both counts the script takes are of lines.
fn lines_holding(text: &str, needle: &str) -> usize {
    text.lines().filter(|line| line.contains(needle)).count()
}

/// The two textual replaces, in the script's order.
fn bake(config: &str, id: &str) -> String {
    let baked = replace_id(config, "database_id", id);
    replace_id(&baked, "D1_DATABASE_ID", id)
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

/// The script's stamp pipeline - `cat migrations/*.sql` through
/// `shasum -a 256` and `cut`, redirected into `migrations-applied` -
/// over [`migration_files`]' reading of the glob.
///
/// The redirection truncated the stamp file before the pipeline ran, and
/// `shasum` digested whatever `cat` delivered, so the file is written on
/// every path - the empty digest of an unexpanded glob included - and
/// only then does `pipefail` end the run.  A stamp that is never read
/// again beats one silently left describing the pre-wipe schema.
///
/// Ceiling: `cat` streams, so a file that failed mid-read still
/// contributed its prefix there and contributes nothing here, and the
/// diagnostics are this port's own.
fn stamp(root: &Path) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut delivered = true;
    let files = migration_files(&root.join("migrations")).unwrap_or_else(|error| {
        eprintln!("{error:#}");
        delivered = false;
        Vec::new()
    });
    // An unexpanded glob reached `cat` as the literal pattern, which is
    // a file that does not exist.
    delivered &= !files.is_empty();
    for file in files {
        match std::fs::read(&file) {
            Ok(bytes) => hasher.update(bytes),
            Err(error) => {
                eprintln!("read {}: {error}", file.display());
                delivered = false;
            }
        }
    }

    let digest = cabin_core::hash::hex_digest(&hasher.finalize());
    let path = root.join("migrations-applied");
    // `cut` ends its line, so the stamp file carries one.
    std::fs::write(&path, format!("{digest}\n"))
        .with_context(|| format!("write {}", path.display()))?;
    if !delivered {
        bail!("cat migrations/*.sql failed");
    }
    Ok(())
}

/// One page of the R2 REST listing, read for its keys alone.
#[derive(Deserialize)]
struct Page {
    success: bool,
    result: Vec<Object>,
}

#[derive(Deserialize)]
struct Object {
    key: String,
}

/// The sweep: list `blobs/`, delete every key, and list again until
/// nothing matches.  The BACKUP bucket is append-only and deliberately
/// not swept.
fn sweep(account: &str, token: &str) -> u64 {
    let agent = crate::audit::agent();
    let api = format!(
        "{}/accounts/{account}/r2/buckets/{BLOBS_BUCKET}/objects",
        api_base()
    );
    let mut deleted: u64 = 0;
    loop {
        let page = or_fail(
            crate::audit::get(&agent, &format!("{api}?prefix=blobs/&per_page=500"), token)
                .map_err(|_| anyhow!("listing {BLOBS_BUCKET} failed")),
        );
        let listed = or_fail(keys(&page));
        // An empty capture: no object matched the prefix - and equally,
        // one object whose key is empty, which ended the shell's loop the
        // same way.
        if listed.is_empty() {
            return deleted;
        }
        for key in listed.split('\n') {
            if key.is_empty() {
                continue;
            }
            or_fail(
                delete(&agent, &format!("{api}/{key}"), token)
                    .map_err(|_| anyhow!("deleting {key} failed")),
            );
            deleted += 1;
        }
    }
}

/// The Cloudflare API base.  `CF_API_BASE` is the seam the Worker
/// (`registry/src/backup_glue.rs`) and the smoke run already use to point
/// Cloudflare API calls at a local server.
fn api_base() -> String {
    std::env::var("CF_API_BASE")
        .ok()
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_owned())
}

/// The keys of one page, as the `node` projection printed them and `$(…)`
/// captured them: one percent-encoded key per line with the slashes left
/// literal (the API requires it), trailing newlines stripped.
///
/// # Errors
///
/// If the page is not a successful listing of objects with keys - each of
/// which the script reached through the same `|| fail`, whether by
/// `!out.success`, a `SyntaxError` or a `TypeError`.
fn keys(page: &str) -> Result<String> {
    let unexpected = || anyhow!("unexpected R2 list response: {page}");
    let parsed: Page = serde_json::from_str(page).map_err(|_| unexpected())?;
    if !parsed.success {
        return Err(unexpected());
    }
    let mut text = String::new();
    for object in &parsed.result {
        text.push_str(&crate::governor::encode_prefix(&object.key));
        text.push('\n');
    }
    Ok(text.trim_end_matches('\n').to_owned())
}

/// `curl -fsS -o /dev/null -X DELETE`: the body is discarded and only the
/// status is read.
fn delete(agent: &ureq::Agent, url: &str, token: &str) -> Result<()> {
    agent
        .delete(url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()?;
    Ok(())
}

/// `wrangler d1 execute DB <flag> --command "UPDATE meta …"`.  The value
/// is interpolated into the statement as the script interpolated it: it
/// is a number this run computed, not an answer from anywhere.
fn bump(mode: Mode, new: i64) -> Result<()> {
    wrangler_run(
        &[
            "d1",
            "execute",
            "DB",
            mode.flag(),
            "--command",
            &format!("UPDATE meta SET value = '{new}' WHERE key = 'registry_generation'"),
        ],
        Show::Stdout,
    )
}

/// Whether a wrangler call's stdout is the operator's sign of life or the
/// script's `>/dev/null`.
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
    //! Every expectation below was taken from the shell the port
    //! replaces, run over the same values.

    use super::*;

    /// An id of the shape the account hands back, and a second one to
    /// tell a rewritten site from an untouched one.
    const NEW: &str = "1234abcd-1234-abcd-1234-abcd1234abcd";
    const OLD: &str = "00000000-0000-0000-0000-000000000000";

    #[test]
    fn the_generation_is_read_as_console_log_printed_it() {
        use serde_json::json;
        assert_eq!(logged(Some(&json!("7"))), "7");
        assert_eq!(logged(Some(&json!(7))), "7");
        assert_eq!(logged(None), "undefined");
        assert_eq!(logged(Some(&json!(null))), "null");
        // The trap: `String(["7"])` is `"7"`, so rendering an array
        // through `display` would pass the numeric guard and wipe against
        // a generation `console.log` printed as `[ '7' ]`.
        assert!(!numeric(&logged(Some(&json!(["7"])))));
        assert!(!numeric(&logged(Some(&json!({ "value": 7 })))));
        assert!(!numeric(&logged(None)));
        assert!(!numeric(&logged(Some(&json!(null)))));
        assert!(!numeric(""), "an empty capture is not a number");
        assert!(!numeric("7 "), "the pattern carries its own anchors");
        assert!(!numeric("-7"));
        assert!(numeric("0"));
    }

    #[test]
    fn the_bump_is_bash_arithmetic() {
        assert_eq!(increment("0").unwrap(), 1);
        assert_eq!(increment("7").unwrap(), 8);
        // A leading zero is octal: `010` is eight.
        assert_eq!(increment("010").unwrap(), 9);
        assert_eq!(increment("007").unwrap(), 8);
        assert_eq!(increment("000").unwrap(), 1);
        // Which makes `08` no number at all - bash diagnosed it and
        // `set -e` ended the run there.
        assert!(increment("08").is_err());
        assert!(increment("09").is_err());
        // 64-bit and wrapping, both at the signed edge and while reading
        // a literal too wide to hold.
        assert_eq!(increment("9223372036854775807").unwrap(), i64::MIN);
        assert_eq!(
            increment("99999999999999999999").unwrap(),
            7_766_279_631_452_241_920
        );
    }

    #[test]
    fn the_confirmation_reads_one_line_the_way_read_r_did() {
        assert!(confirmed("wipe\n"));
        // Default IFS strips leading and trailing blanks.
        assert!(confirmed("  wipe  \n"));
        assert!(confirmed("\twipe\t\n"));

        assert!(!confirmed("Wipe\n"));
        assert!(!confirmed("wipe it\n"));
        assert!(!confirmed("\n"));
        // `read` keeps a CRLF line's `\r`: it is not an IFS blank.
        assert!(!confirmed("wipe\r\n"));
        // End of input, where `read` returned non-zero and `set -e` ended
        // the run before the comparison ran at all.
        assert!(!confirmed(""));
        assert!(!confirmed("wipe"));
    }

    #[test]
    fn the_listed_id_falls_back_the_way_javascript_did() {
        assert_eq!(
            listed_id(&format!(r#"[{{"name":"cabin-registry","uuid":"{NEW}"}}]"#)),
            Some(NEW.to_owned())
        );
        // Every falsy `uuid` falls through to `database_id`, a missing
        // one included.
        for falsy in ["\"\"", "null", "0", "false"] {
            assert_eq!(
                listed_id(&format!(
                    r#"[{{"name":"cabin-registry","uuid":{falsy},"database_id":"{NEW}"}}]"#
                )),
                Some(NEW.to_owned()),
                "uuid {falsy} is falsy"
            );
        }
        assert_eq!(
            listed_id(&format!(
                r#"[{{"name":"cabin-registry","database_id":"{NEW}"}}]"#
            )),
            Some(NEW.to_owned())
        );
        // A truthy `uuid` wins even when it is not an id at all, and the
        // grammar refuses it rather than reaching for the other field.
        assert_eq!(
            listed_id(&format!(
                r#"[{{"name":"cabin-registry","uuid":"nope","database_id":"{NEW}"}}]"#
            )),
            Some("nope".to_owned())
        );
        // The first entry of the name wins, as it does for the guard.
        assert_eq!(
            listed_id(&format!(
                r#"[{{"name":"cabin-registry","uuid":"{NEW}"}},
                    {{"name":"cabin-registry","uuid":"{OLD}"}}]"#
            )),
            Some(NEW.to_owned())
        );
        // Found, but carrying neither: `console.log(undefined)`, which
        // the id grammar then refuses.
        assert_eq!(
            listed_id(r#"[{"name":"cabin-registry"}]"#),
            Some("undefined".to_owned())
        );
        assert!(!database_id("undefined"));
        // No such database, and an answer that is not a listing at all:
        // the script's other refusal.
        assert_eq!(listed_id(r#"[{"name":"other","uuid":"x"}]"#), None);
        assert_eq!(listed_id(r#"[{"name":42,"uuid":"x"}]"#), None);
        assert_eq!(listed_id("not json"), None);
        assert_eq!(listed_id("[]"), None);
    }

    #[test]
    fn the_id_grammar_checks_the_alphabet_and_the_width() {
        assert!(database_id(NEW));
        // The carried-over ceiling: 36 hyphens satisfy it.
        assert!(database_id(&"-".repeat(36)));
        assert!(!database_id(&"a".repeat(35)));
        assert!(!database_id(&"a".repeat(37)));
        assert!(!database_id("1234ABCD-1234-abcd-1234-abcd1234abcd"));
        assert!(!database_id("1234abcg-1234-abcd-1234-abcd1234abcd"));
    }

    #[test]
    fn the_rewrite_lands_on_the_first_binding_of_each_name() {
        let config = format!(
            "{{\n  \"d1_databases\": [\n    {{ \"database_id\": \"{OLD}\" }}\n  ],\n  \
             \"vars\": {{\n    \"D1_DATABASE_ID\": \"{OLD}\"\n  }}\n}}\n"
        );
        let baked = bake(&config, NEW);
        assert_eq!(lines_holding(&baked, NEW), 2);
        assert_eq!(lines_holding(&baked, OLD), 0);

        // A second `database_id` is left alone - the count guard before
        // the rewrite is what refuses that config in the first place.
        let two = format!("\"database_id\": \"{OLD}\"\n\"database_id\": \"{OLD}\"\n");
        let baked = bake(&two, NEW);
        assert_eq!(lines_holding(&baked, NEW), 1);
        assert_eq!(lines_holding(&baked, OLD), 1);

        // The separator is the literal `": "` the regex spelled, and the
        // value must fit the shape: none of these is a match, and the
        // twice-present count is what catches it.
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
        assert_eq!(lines_holding(&bake(&late, NEW), NEW), 1);
    }

    #[test]
    fn the_counts_are_of_lines() {
        // `grep -c` counts lines, so two bindings on one line count once,
        // which is what makes the twice-present check refuse a config it
        // could not have baked correctly.
        let one_line = format!("\"database_id\": \"{NEW}\" \"D1_DATABASE_ID\": \"{NEW}\"");
        assert_eq!(lines_holding(&one_line, "\"database_id\""), 1);
        assert_eq!(lines_holding(&one_line, NEW), 1);
        assert_eq!(lines_holding("a\na\na", "a"), 3);
        // A file without a trailing newline still ends a line.
        assert_eq!(lines_holding("a\nb", "b"), 1);
        assert_eq!(lines_holding("", "a"), 0);
    }

    #[test]
    fn the_listing_projects_one_encoded_key_per_line() {
        let page = |result: &str| format!("{{\"success\":true,\"result\":{result}}}");
        assert_eq!(keys(&page("[]")).unwrap(), "");
        assert_eq!(
            keys(&page(r#"[{"key":"blobs/sha256/ab"},{"key":"blobs/x y"}]"#)).unwrap(),
            "blobs/sha256/ab\nblobs/x%20y"
        );
        // Slashes stay literal; every other character is escaped, a
        // newline included - so no key can become two lines.
        assert_eq!(
            keys(&page(r#"[{"key":"blobs/a\nb"}]"#)).unwrap(),
            "blobs/a%0Ab"
        );
        // One empty key projects one empty line, which `$(…)` strips to
        // nothing - and an empty capture ended the shell's loop.
        assert_eq!(keys(&page(r#"[{"key":""}]"#)).unwrap(), "");

        // Everything the script reached through one `|| fail`.
        for refused in [
            r#"{"success":false,"result":[]}"#,
            r#"{"success":true}"#,
            r#"{"success":true,"result":{}}"#,
            r#"{"success":true,"result":[{"size":1}]}"#,
            r#"{"success":true,"result":[{"key":42}]}"#,
            "not json",
            "",
        ] {
            let refusal = keys(refused)
                .expect_err("an answer the script refused")
                .to_string();
            assert!(
                refusal.starts_with("unexpected R2 list response: "),
                "{refused}: {refusal}"
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
        let mut hasher = Sha256::new();
        hasher.update(b"onetwo");
        let stamped = || std::fs::read_to_string(root.join("migrations-applied")).expect("a stamp");
        assert_eq!(
            stamped(),
            format!("{}\n", cabin_core::hash::hex_digest(&hasher.finalize()))
        );

        // An unexpanded glob: `cat` failed on the literal pattern, but
        // the redirection had already truncated the file and `shasum`
        // still digested the empty input it was handed.
        std::fs::remove_dir_all(&migrations).expect("an empty selection");
        std::fs::create_dir_all(&migrations).expect("the migrations directory");
        stamp(root).expect_err("the pipeline failed");
        assert_eq!(
            stamped(),
            format!(
                "{}\n",
                cabin_core::hash::hex_digest(&Sha256::new().finalize())
            )
        );
    }
}
