//! Apply D1 migrations and keep the deploy gate's stamp honest
//! (`registry/docs/runbook.md`, "Integrated topology and route
//! management"), ported one-to-one from `registry/scripts/migrate.sh`:
//!
//! ```text
//!   cargo registry-migrate --local    apply to the local .wrangler/ state
//!   cargo registry-migrate --remote   apply to the live database, then stamp
//! ```
//!
//! The CI deploy stays skipped while `migrations/` disagrees with
//! `migrations-applied`, and the stamp must only ever be refreshed
//! after the live database really runs the files' content.  Only
//! `--remote` touches the stamp: it attests the LIVE schema, and a
//! local apply proves nothing about production.  The applied set is
//! read from D1's own bookkeeping (the `d1_migrations` table, by
//! filename), so this refuses every state a stamp refresh would
//! wrongly certify: an already-applied file edited in place (D1 never
//! replays it), and a recorded applied migration whose file was
//! renamed or removed (its effects live on in the schema while the
//! files pretend otherwise).  Both route through [`crate::wipe`]
//! pre-launch (drop, recreate, apply from zero); post-launch, schema
//! changes are only ever NEW files.
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior.  Each was pinned by running the original
//! under `bash`:
//!
//! - **The refusals run in the script's order**, which is not the
//!   order that reads most naturally: the missing-file and
//!   edited-in-place refusals both come BEFORE the "nothing pending"
//!   early exit, so a run with nothing to apply still refuses a
//!   corrupt state rather than reporting the stamp current.
//! - **The stamp written at the end is computed before the apply**, over
//!   ALL migration files.  It is the digest the deploy gate will
//!   compare against, and computing it after the apply would let a
//!   file edited mid-run reach `migrations-applied`.
//! - **An unreadable applied set is never an empty one.**  Only an
//!   absent `d1_migrations` table - the never-migrated database of a
//!   first provisioning - reads as "nothing applied"; every other
//!   failure refuses, because an empty set makes every file pending
//!   and every refusal vacuous.
//! - **`$(cat migrations-applied)` is a command substitution**, so it
//!   drops NUL bytes and strips trailing newlines and nothing else - a
//!   CRLF ending's `\r` survives into the comparison, as it does in
//!   the deploy gate that reads the same file
//!   ([`xtask_workflow_guard::migrations_pending`]).  Unlike that
//!   gate's, this read is not inside an `if` condition, so a missing
//!   stamp file ends the run under `set -e` instead of comparing as
//!   empty.
//! - **`read -r` under default `IFS`** strips leading and trailing
//!   spaces and tabs from the confirmation but keeps a trailing `\r`,
//!   and returns non-zero at end of input - which `set -e` turned into
//!   an exit, so an answer that is not newline-terminated never
//!   reached the comparison at all.
//! - **The migrations glob** is [`migration_files`], the deploy gate's
//!   own rule, so the two readings of `migrations/*.sql` cannot drift.
//!
//! Diagnostics split the way `docs/architecture.md` draws the line for
//! a ported script: every refusal the script itself wrote through
//! `lib.sh`'s `fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }` keeps
//! those bytes, through `or_fail` - as `xtask-registry-guard` and
//! `xtask-registry-smoke` keep theirs, and as [`crate::verify`]'s
//! `abort` keeps its script's.  An *incidental* failure the shell would
//! have died on under `set -e` - a wrangler spawn, an unreadable stamp
//! file, a `migrations/` that will not list - carries no such text, and
//! reports through this crate's `Result` and the shim's `error:`
//! prefix instead.
//!
//! Ceilings, where this deliberately stops short of the shell.  All
//! keep the exit code and the direction of every refusal:
//!
//! - the argument surface is clap's, in the binary: the mode arrives
//!   here already parsed, so there is no usage error to render;
//! - the pending-sorts-before-applied refusal compares bytes.  Bash's
//!   `[[ < ]]` collates through `strcoll` in the operator's locale,
//!   where `en_US.UTF-8` sorts `0002_a.sql` before `0002_B.sql` and
//!   `C` does not - the same locale divergence
//!   [`xtask_workflow_guard::migrations_pending`] carries for the
//!   glob's order, and the byte order is what a fresh database's
//!   replay actually follows;
//! - a confirmation that ends at end of input rather than at a newline
//!   refuses saying `FAIL: not confirmed`, where `set -e` ended the
//!   script at the failing `read` with nothing on stderr.  Both exit 1
//!   without applying anything; the alternative - accepting an
//!   EOF-terminated `migrate` - would apply migrations the shell did
//!   not;
//! - an empty or unreadable `migrations/` ends the run at the stamp
//!   computation, where the shell's unexpanded glob had already
//!   counted one pending file literally named `migrations/*.sql` and
//!   then died in `cat` under `pipefail`.  Same exit code, and the
//!   count line above it reads `0 pending` rather than `1 pending`;
//! - membership is an exact whole-line match on bytes.  `grep -qxF`
//!   reads a pattern carrying a newline as two alternative patterns,
//!   so a migration filename containing one would have matched either
//!   half there and matches neither here;
//! - every abort exits 1, where `set -e` propagated wrangler's own
//!   status from a failed apply.  Nothing reads the distinction;
//! - the `node` hop is gone: the D1 answers are parsed here.  Both
//!   refusal messages are the script's, but the diagnostic underneath
//!   one - a `TypeError` from `node` - is not reproduced.  Wrangler's
//!   own stderr still reaches the operator's terminal;
//! - arguments after the mode are rejected, where `$1` alone was read
//!   and the rest ignored - this crate's dispatcher convention, and a
//!   refusal rather than a silent acceptance;
//! - the confirmation is read as UTF-8: an answer carrying invalid
//!   bytes refuses through the read's own error, and one carrying NUL
//!   bytes refuses as "not confirmed", where bash dropped the NULs
//!   before comparing and could confirm on `mig\0rate`.  Both refuse
//!   where the shell also refused on everything an operator can type.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use sha2::{Digest as _, Sha256};
use xtask_workflow_guard::migrations_pending::migration_files;

use crate::launch_guard::Mode;
use crate::{
    Nullish, column_lines, display, output, registry_dir, results, status, step, wrangler,
};

/// Is there anything to read the applied names out of?  A first
/// provisioning has no `d1_migrations` table, which is the one absence
/// that means "nothing applied" rather than "unreadable".
const HAS_TABLE: &str = "SELECT COUNT(*) AS n FROM sqlite_master
     WHERE type = 'table' AND name = 'd1_migrations'";

const APPLIED_NAMES: &str = "SELECT name FROM d1_migrations ORDER BY name";

/// `lib.sh`'s `fail`: the script's own refusal text on stderr, exit 1.
/// The prefix lands on the first line only, which is what
/// `printf 'FAIL: %s\n'` does with a multi-line argument, so the
/// refusals' continuation lines stay unindented.
///
/// Held apart from the `Result` the incidental failures take, because
/// the shim renders those with its own `error:` prefix and these carry
/// bytes an operator reads.
fn or_fail<T>(result: Result<T>) -> T {
    result.unwrap_or_else(|error| {
        eprintln!("FAIL: {error:#}");
        std::process::exit(1)
    })
}

/// Applies the migrations in `mode`.
///
/// # Errors
///
/// On the incidental failures the shell died on under `set -e` - a
/// wrangler invocation that will not run, an unreadable `migrations/`
/// or stamp file.  The script's own refusals leave through `or_fail`
/// instead.
pub fn run(mode: Mode) -> Result<()> {
    match mode {
        Mode::Local => local(),
        Mode::Remote => remote(),
    }
}

/// The local apply, which proves nothing about production and so never
/// reaches the stamp.
fn local() -> Result<()> {
    step("applying migrations to the local database");
    apply("--local")?;
    println!("local migrate OK (the migrations-applied stamp tracks the live");
    println!("database only; a local apply never touches it)");
    Ok(())
}

fn remote() -> Result<()> {
    let root = root();
    let migrations = root.join("migrations");

    step("reading the applied set from the live database");
    let applied = or_fail(applied_names());
    let files = migration_files(&migrations)?;
    let (applied_files, pending): (Vec<PathBuf>, Vec<PathBuf>) = files
        .iter()
        .cloned()
        .partition(|file| recorded(&applied, file));
    println!(
        "    {} applied, {} pending migration file(s)",
        applied_files.len(),
        pending.len()
    );

    or_fail(every_recorded_name_exists(&migrations, &applied));

    // Computed before anything is applied, and written unchanged at
    // the end: this is the digest the deploy gate compares against.
    let stamp = digest(&files)?;
    or_fail(still_hashes_to_the_stamp(
        &applied_files,
        &recorded_stamp(&root)?,
    ));

    if pending.is_empty() {
        println!("remote migrate OK (nothing pending; the stamp is already current)");
        return Ok(());
    }

    or_fail(every_pending_sorts_after(&applied_files, &pending));
    or_fail(confirm(pending.len()));

    step("applying migrations to the live database");
    apply("--remote")?;

    step("verifying the live database now records every migration file");
    let applied = or_fail(applied_names());
    or_fail(every_file_is_recorded(&files, &applied));

    step("refreshing the migrations-applied stamp");
    let path = root.join("migrations-applied");
    std::fs::write(&path, format!("{stamp}\n"))
        .with_context(|| format!("write {}", path.display()))?;

    println!("remote migrate OK (stamp {stamp})");
    println!();
    println!("Follow-ups:");
    println!("  - commit the migrations-applied change; the CI deploy stays skipped");
    println!("    until it reaches main (docs/runbook.md, \"Integrated topology\")");
    Ok(())
}

/// One recorded-migration name per line, from D1's own bookkeeping.
fn applied_names() -> Result<Vec<String>> {
    let bookkeeping = || anyhow!("could not read the live database's migration bookkeeping");
    let answer = d1(HAS_TABLE).map_err(|_| bookkeeping())?;
    let rows = results(&answer).map_err(|_| bookkeeping())?;
    // `console.log(out[0].results[0].n)`: no row at all was the
    // `TypeError` that failed the pipeline, while a row carrying no `n`
    // printed the word `undefined` and read as "the table is there".
    let count = rows.first().ok_or_else(bookkeeping)?;
    if table_absent(count.get("n")) {
        return Ok(Vec::new());
    }

    let names = || anyhow!("could not read the live database's applied-migration names");
    let answer = d1(APPLIED_NAMES).map_err(|_| names())?;
    let rows = results(&answer).map_err(|_| names())?;
    Ok(column_lines(&rows, "name", Nullish::Printed))
}

/// `[[ "$has_table" == "0" ]]` against `console.log`'s rendering of the
/// count, which is `"0"` only for the number or the string zero.
/// `console.log` renders an array or object through `util.inspect`
/// (`[ 0 ]`, `{ n: 0 }`), never as the bare `0` - and `display` is the
/// other coercion (`${x}`, where `String([0])` IS `"0"`), so routing a
/// malformed count through it would read "no bookkeeping table" and
/// skip every refusal built on the applied set.
fn table_absent(count: Option<&serde_json::Value>) -> bool {
    match count {
        None | Some(serde_json::Value::Array(_) | serde_json::Value::Object(_)) => false,
        Some(count) => display(count) == "0",
    }
}

/// Refusal (a): every name D1 recorded must still exist as a file.  A
/// renamed or removed applied migration leaves its effects in the live
/// schema while the files no longer describe them, and no stamp may
/// certify that.
fn every_recorded_name_exists(migrations: &Path, applied: &[String]) -> Result<()> {
    for name in applied {
        // The here-string that fed the shell's loop always carried a
        // final newline, so an empty applied set arrived as one blank
        // line.
        if name.is_empty() {
            continue;
        }
        // `[[ -f migrations/$name ]]` concatenates, so no recorded
        // name could escape the directory the shell tested in: a
        // POSIX-absolute one stayed inside (`migrations//etc/passwd`),
        // and a Windows-rooted one (`C:\...`, a UNC path) named a file
        // that could not exist there. `Path::join` would discard or
        // reset the base for both, so the trimmed name must carry no
        // root of its own before it is joined.
        let name_path = Path::new(name.trim_start_matches('/'));
        let escapes = name_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        });
        if escapes || !migrations.join(name_path).is_file() {
            bail!(
                "D1 records applied migration '{name}' but migrations/{name} is gone
(renamed or removed). The live schema still carries its effects; restore
the file, or reset pre-launch via cargo registry-wipe."
            );
        }
    }
    Ok(())
}

/// Refusal (b): the already-applied files must still hash to the
/// recorded stamp before anything new is applied.  D1 tracks applied
/// migrations by FILENAME, so an in-place edit of an applied file would
/// never replay, and refreshing the aggregate stamp after applying only
/// new files would certify a live schema that does not match
/// `migrations/`.  A database with nothing applied yet has nothing to
/// have been edited.
fn still_hashes_to_the_stamp(applied_files: &[PathBuf], recorded: &[u8]) -> Result<()> {
    if applied_files.is_empty() || digest(applied_files)?.as_bytes() == recorded {
        return Ok(());
    }
    bail!(
        "an already-applied migration file was edited in place (the applied
files no longer hash to the migrations-applied stamp). D1 will NOT
replay it, and stamping would unblock deploys against a stale live
schema. Pre-launch, the edited baseline ships through cargo registry-wipe;
post-launch, write a NEW migration file instead of editing an applied
one."
    );
}

/// Refusal (c): every pending file must sort after every applied one.
/// A fresh database replays `migrations/*.sql` in glob order, so a new
/// file sorting before an applied one would give the live database and
/// a rebuilt one different histories under the same stamp.
fn every_pending_sorts_after(applied_files: &[PathBuf], pending: &[PathBuf]) -> Result<()> {
    let Some(last_applied) = applied_files.last() else {
        return Ok(());
    };
    let last_applied = file_name(last_applied);
    for file in pending {
        if file_name(file) < last_applied {
            bail!(
                "pending migration {} sorts before applied {}; a
fresh database would replay them in a different order than the live one
ran. Name new migrations after every applied one.",
                basename(file),
                last_applied.to_string_lossy()
            );
        }
    }
    Ok(())
}

/// The post-apply verification: D1 must now record every migration
/// file.  A file the apply did not reach must leave the stamp alone -
/// refreshing it would certify a live schema missing that file.
fn every_file_is_recorded(files: &[PathBuf], applied: &[String]) -> Result<()> {
    for file in files {
        if !recorded(applied, file) {
            bail!(
                "{} is not recorded as applied after the apply; do not stamp",
                basename(file)
            );
        }
    }
    Ok(())
}

/// The interactive confirmation, skipped when its escape hatch is set
/// to exactly `1`.
fn confirm(pending: usize) -> Result<()> {
    if std::env::var(cabin_env::CABIN_MIGRATE_YES).as_deref() == Ok("1") {
        return Ok(());
    }
    print!(
        "About to apply {pending} migration file(s) to the LIVE database. \
         Type \"migrate\" to confirm: "
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
/// followed by `[[ "$answer" == "migrate" ]]` judged it: an answer that
/// is not newline-terminated is end of input, where `read` failed and
/// `set -e` ended the run before the comparison; otherwise the default
/// `IFS` strips leading and trailing spaces and tabs - and nothing
/// else, so a CRLF line's `\r` still refuses.
fn confirmed(answer: &str) -> bool {
    let Some(answer) = answer.strip_suffix('\n') else {
        return false;
    };
    answer.trim_matches([' ', '\t']) == "migrate"
}

/// The stamp file through `$(cat migrations-applied)`.  This one is
/// not inside an `if` condition, so an unreadable stamp file ends the
/// run rather than comparing as empty - which would read as "every
/// applied file was edited in place".
fn recorded_stamp(root: &Path) -> Result<Vec<u8>> {
    let path = root.join("migrations-applied");
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(substitute(bytes))
}

/// A command substitution's own reading of a file's bytes: NUL bytes
/// dropped, trailing newlines stripped, everything else kept.
fn substitute(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.retain(|byte| *byte != 0);
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

/// `cat <files> | shasum -a 256 | cut -d' ' -f1`, over the files in
/// glob order.
fn digest(files: &[PathBuf]) -> Result<String> {
    if files.is_empty() {
        bail!("no migrations match migrations/*.sql");
    }
    digest_of(files)
}

/// [`digest`] without the emptiness guard: the diagnostics bundle
/// digests an empty tree to the empty-input digest so its stamp
/// comparison still renders PENDING instead of aborting.
pub(crate) fn digest_of(files: &[PathBuf]) -> Result<String> {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(std::fs::read(file).with_context(|| format!("read {}", file.display()))?);
    }
    Ok(cabin_core::hash::hex_digest(&hasher.finalize()))
}

/// `grep -qxF "$(basename "$file")" <<<"$applied"`: an exact
/// whole-line match, on bytes rather than UTF-8 - the glob selects
/// names that are not UTF-8 at all, and rendering one lossily here
/// would match a name D1 never recorded.
fn recorded(applied: &[String], file: &Path) -> bool {
    let name = file_name(file);
    // `grep -qxF "$name"` read a leading-dash basename as options and
    // failed, and every caller treated that failure as "not recorded" -
    // the fail-closed reading this keeps.
    if name.as_encoded_bytes().starts_with(b"-") {
        return false;
    }
    applied
        .iter()
        .any(|recorded| OsStr::new(recorded.as_str()) == name)
}

fn file_name(file: &Path) -> &OsStr {
    file.file_name().unwrap_or(file.as_os_str())
}

/// `basename "$file"` as a message renders it.
fn basename(file: &Path) -> std::borrow::Cow<'_, str> {
    file_name(file).to_string_lossy()
}

/// The registry root this run reads, writes and runs wrangler in.
/// `CABIN_REGISTRY_DIR` overrides [`registry_dir`], which is how tests
/// point the command at a synthetic registry
/// root.  Everything the run touches resolves
/// through this one function, so the migrations, the stamp and
/// wrangler's working directory cannot come from different trees.
fn root() -> PathBuf {
    std::env::var_os(cabin_env::CABIN_REGISTRY_DIR)
        .filter(|value| !value.is_empty())
        .map_or_else(registry_dir, PathBuf::from)
}

fn d1(sql: &str) -> Result<String> {
    output(
        wrangler(&[
            "d1",
            "execute",
            "DB",
            "--remote",
            "--json",
            "--command",
            sql,
        ])
        .current_dir(root()),
    )
}

/// `wrangler d1 migrations apply DB <flag>`, whose output is the
/// operator's only sign of life while it runs and so is not captured.
fn apply(flag: &str) -> Result<()> {
    status(wrangler(&["d1", "migrations", "apply", "DB", flag]).current_dir(root()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        std::fs::create_dir_all(dir).expect("the fixture directory");
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("the fixture file");
        path
    }

    #[test]
    fn only_a_console_logged_zero_reads_as_no_table() {
        use serde_json::json;
        assert!(table_absent(Some(&json!(0))));
        assert!(table_absent(Some(&json!("0"))));
        assert!(!table_absent(Some(&json!(1))));
        assert!(!table_absent(None), "`undefined` is not `0`");
        // console.log renders these `[ 0 ]` and `{ n: 0 }`, never the
        // bare `0` - where `${x}` coercion would say "0" for the array
        // and fail open to "no bookkeeping table".
        assert!(!table_absent(Some(&json!([0]))));
        assert!(!table_absent(Some(&json!({ "n": 0 }))));
    }

    #[test]
    fn an_absolute_recorded_name_stays_inside_migrations() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let migrations = temp.path().join("migrations");
        write(&migrations, "0001_init.sql", b"create table a;\n");
        // The shell tested `migrations//etc/passwd`; a join that
        // discarded the base would test the host's real /etc/passwd
        // and could pass.
        let refusal = every_recorded_name_exists(&migrations, &["/etc/passwd".to_owned()])
            .expect_err("an absolute name is a missing file");
        assert!(refusal.to_string().contains("'/etc/passwd'"), "{refusal}");
        // A Windows-rooted name reads as missing on every platform;
        // on a Windows host `Path::join` would otherwise reset the
        // base to the drive.
        for rooted in ["C:\\evil.sql", "\\\\server\\share\\evil.sql"] {
            every_recorded_name_exists(&migrations, &[rooted.to_owned()])
                .expect_err("a rooted name is a missing file");
        }
    }

    #[test]
    fn a_leading_dash_name_is_never_recorded() {
        // grep read `-x.sql` as options and failed; every caller took
        // that as "not recorded".
        let applied = ["-x.sql".to_owned()];
        assert!(!recorded(&applied, Path::new("migrations/-x.sql")));
    }

    #[test]
    fn membership_is_an_exact_whole_line_match() {
        let applied = ["0001_init.sql".to_owned(), String::new()];
        assert!(recorded(&applied, Path::new("migrations/0001_init.sql")));
        // `-x` anchors both ends: neither a prefix nor a suffix of a
        // recorded name is that name.
        assert!(!recorded(&applied, Path::new("migrations/0001_init.sq")));
        assert!(!recorded(&applied, Path::new("migrations/00001_init.sql")));
        assert!(!recorded(&applied, Path::new("migrations/0002_next.sql")));
        // The blank line an empty applied set arrives as matches
        // nothing at all.
        let blank = [String::new()];
        assert!(!recorded(&blank, Path::new("migrations/0001_init.sql")));
    }

    #[test]
    fn a_recorded_name_without_its_file_refuses() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        write(temp.path(), "0001_init.sql", b"create");
        let applied = ["0001_init.sql".to_owned(), String::new()];
        every_recorded_name_exists(temp.path(), &applied).expect("the file is there");

        let applied = ["0001_init.sql".to_owned(), "0002_gone.sql".to_owned()];
        assert_eq!(
            every_recorded_name_exists(temp.path(), &applied)
                .expect_err("the renamed migration")
                .to_string(),
            "D1 records applied migration '0002_gone.sql' but migrations/0002_gone.sql is gone
(renamed or removed). The live schema still carries its effects; restore
the file, or reset pre-launch via cargo registry-wipe."
        );
    }

    #[test]
    fn an_edited_applied_file_refuses() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let first = write(temp.path(), "0001_init.sql", b"one");
        let second = write(temp.path(), "0002_next.sql", b"two");
        let applied = [first, second];
        let stamp = digest(&applied).expect("the applied digest");

        still_hashes_to_the_stamp(&applied, stamp.as_bytes()).expect("the stamp still matches");
        // Nothing applied yet has nothing to have been edited, whatever
        // the stamp says.
        still_hashes_to_the_stamp(&[], b"stale").expect("an unmigrated database");

        assert_eq!(
            still_hashes_to_the_stamp(&applied, b"stale")
                .expect_err("the edited file")
                .to_string(),
            "an already-applied migration file was edited in place (the applied
files no longer hash to the migrations-applied stamp). D1 will NOT
replay it, and stamping would unblock deploys against a stale live
schema. Pre-launch, the edited baseline ships through cargo registry-wipe;
post-launch, write a NEW migration file instead of editing an applied
one."
        );
    }

    #[test]
    fn the_digest_concatenates_in_glob_order() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        write(temp.path(), "0001_a.sql", b"one");
        write(temp.path(), "0002_b.sql", b"two");
        let files = migration_files(temp.path()).expect("the glob");
        let mut hasher = Sha256::new();
        hasher.update(b"onetwo");
        assert_eq!(
            digest(&files).expect("the digest"),
            cabin_core::hash::hex_digest(&hasher.finalize())
        );
        // Reversing the order is a different digest, which is what the
        // sort-order refusal protects.
        let reversed: Vec<PathBuf> = files.iter().rev().cloned().collect();
        assert_ne!(
            digest(&reversed).expect("the digest"),
            digest(&files).expect("the digest")
        );
    }

    #[test]
    fn an_empty_selection_has_no_digest() {
        assert_eq!(
            digest(&[]).expect_err("nothing to hash").to_string(),
            "no migrations match migrations/*.sql"
        );
    }

    #[test]
    fn a_pending_file_sorting_before_an_applied_one_refuses() {
        let applied = [PathBuf::from("migrations/0002_b.sql")];
        every_pending_sorts_after(&applied, &[PathBuf::from("migrations/0003_c.sql")])
            .expect("a later name");
        // Nothing applied means no order to contradict.
        every_pending_sorts_after(&[], &[PathBuf::from("migrations/0001_a.sql")])
            .expect("an unmigrated database");

        assert_eq!(
            every_pending_sorts_after(&applied, &[PathBuf::from("migrations/0001_a.sql")])
                .expect_err("the out-of-order migration")
                .to_string(),
            "pending migration 0001_a.sql sorts before applied 0002_b.sql; a
fresh database would replay them in a different order than the live one
ran. Name new migrations after every applied one."
        );
    }

    #[test]
    fn the_sort_order_is_bytewise() {
        // `B` (0x42) sorts before `a` (0x61), so a pending `0002_a.sql`
        // is out of order after an applied `0002_B.sql`. An en_US
        // collation - the operator's locale, and what bash's `[[ < ]]`
        // used - reverses that; the ceiling is in the module docs, and
        // the byte order is the one a fresh database replays in.
        let applied = [PathBuf::from("0002_B.sql")];
        assert!(every_pending_sorts_after(&applied, &[PathBuf::from("0002_a.sql")]).is_ok());
        let applied = [PathBuf::from("0002_a.sql")];
        assert!(every_pending_sorts_after(&applied, &[PathBuf::from("0002_B.sql")]).is_err());
    }

    #[test]
    fn the_confirmation_reads_one_line_the_way_read_r_did() {
        assert!(confirmed("migrate\n"));
        // Default IFS strips leading and trailing blanks.
        assert!(confirmed("  migrate  \n"));
        assert!(confirmed("\tmigrate\t\n"));

        assert!(!confirmed("Migrate\n"));
        assert!(!confirmed("migrate now\n"));
        assert!(!confirmed("\n"));
        // `read` keeps a CRLF line's `\r`: it is not an IFS blank.
        assert!(!confirmed("migrate\r\n"));
        // End of input, where `read` returned non-zero and `set -e`
        // ended the run before the comparison ran at all.
        assert!(!confirmed(""));
        assert!(!confirmed("migrate"));
    }

    #[test]
    fn a_file_the_apply_did_not_reach_refuses_to_stamp() {
        let files = [PathBuf::from("m/0001_a.sql"), PathBuf::from("m/0002_b.sql")];
        let applied = ["0001_a.sql".to_owned(), "0002_b.sql".to_owned()];
        every_file_is_recorded(&files, &applied).expect("both recorded");

        assert_eq!(
            every_file_is_recorded(&files, &applied[..1])
                .expect_err("the file the apply did not reach")
                .to_string(),
            "0002_b.sql is not recorded as applied after the apply; do not stamp"
        );
    }

    #[test]
    fn the_recorded_stamp_reads_like_a_command_substitution() {
        assert_eq!(substitute(b"abc\n\n\n".to_vec()), b"abc");
        assert_eq!(substitute(b"ab\0c\n".to_vec()), b"abc");
        // The `\r` of a CRLF ending survives into the comparison, so a
        // stamp file with CRLF endings refuses rather than passing.
        assert_eq!(substitute(b"abc\r\n".to_vec()), b"abc\r");
    }
}
