//! Apply D1 migrations and keep the deploy gate's stamp honest
//! (`registry/docs/runbook.md`, "Integrated topology and route
//! management"):
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
//! Properties:
//!
//! - **The refusals' order is deliberate**, and is not the order that
//!   reads most naturally: the missing-file and
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
//! - **The stamp file read drops NUL bytes and strips trailing
//!   newlines** and nothing else - a CRLF ending's `\r` survives into
//!   the comparison.  The read is the deploy gate's own
//!   [`migrations_pending::applied`].  A missing stamp file ends the
//!   run: read as empty, it would say every applied file was edited in
//!   place.
//! - **The confirmation** has leading and trailing spaces and tabs
//!   stripped but keeps a trailing `\r`, and an answer that is not
//!   newline-terminated refuses.
//! - **The migrations glob and digest** are the deploy gate's own
//!   [`migration_files`] and [`digest`], so the two
//!   readings of the stamp cannot drift.
//!
//! Diagnostics split in two: every refusal this module makes itself is
//! `FAIL: <message>` on stderr and exit 1, through `or_fail` - as
//! `xtask-registry-guard` and `xtask-registry-smoke` write theirs, and
//! as [`crate::verify`]'s `abort` keeps its own text.  An *incidental*
//! failure - a wrangler spawn, an unreadable stamp file, a
//! `migrations/` that will not list - carries no such text, and reports
//! through this crate's `Result` and the shim's `error:` prefix
//! instead.
//!
//! Ceilings:
//!
//! - the pending-sorts-before-applied refusal compares bytes, the order
//!   a fresh database's replay actually follows, not the operator's
//!   locale collation;
//! - a confirmation that ends at end of input rather than at a newline
//!   refuses saying `FAIL: not confirmed`, without applying anything;
//! - an empty `migrations/` ends the run at the stamp computation;
//! - membership is an exact whole-line match on bytes, so a migration
//!   filename containing a newline matches neither half;
//! - every abort exits 1, even when the failed apply exited otherwise;
//! - arguments after the mode are rejected;
//! - the confirmation is read as UTF-8: an answer carrying invalid
//!   bytes refuses through the read's own error, and one carrying NUL
//!   bytes refuses as "not confirmed".

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use xtask_workflow_guard::migrations_pending::{self, digest, migration_files};

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

/// This module's own refusal text on stderr, exit 1.  The prefix lands
/// on the first line only, so the refusals' continuation lines stay
/// unindented.
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
/// On the incidental failures - a wrangler invocation that will not
/// run, an unreadable `migrations/` or stamp file.  This module's own
/// refusals leave through `or_fail` instead.
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
        &migrations_pending::applied(&root)?,
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
    // No row at all is unreadable bookkeeping, while a row carrying no
    // `n` reads as "the table is there".
    let count = rows.first().ok_or_else(bookkeeping)?;
    if table_absent(count.get("n")) {
        return Ok(Vec::new());
    }

    let names = || anyhow!("could not read the live database's applied-migration names");
    let answer = d1(APPLIED_NAMES).map_err(|_| names())?;
    let rows = results(&answer).map_err(|_| names())?;
    Ok(column_lines(&rows, "name", Nullish::Printed))
}

/// Whether the count is zero, which only the number or the string zero
/// is.  An array or object never is - [`display`] alone would render
/// `[0]` as `"0"`, so routing a malformed count through it would read
/// "no bookkeeping table" and skip every refusal built on the applied
/// set.
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
        // An empty applied set arrives as one blank line.
        if name.is_empty() {
            continue;
        }
        // No recorded name may escape `migrations/`: a POSIX-absolute
        // one is read inside it (`migrations//etc/passwd`), and a
        // Windows-rooted one (`C:\...`, a UNC path) is refused as gone.
        // `Path::join` would discard or reset the base for both, so the
        // trimmed name must carry no root of its own before it is
        // joined.
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

/// Whether one line of input is the confirmation: an answer that is not
/// newline-terminated is end of input and refuses; otherwise leading
/// and trailing spaces and tabs are stripped - and nothing else, so a
/// CRLF line's `\r` still refuses.
fn confirmed(answer: &str) -> bool {
    let Some(answer) = answer.strip_suffix('\n') else {
        return false;
    };
    answer.trim_matches([' ', '\t']) == "migrate"
}

/// Whether D1 recorded `file`'s name: an exact whole-line match, on
/// bytes rather than UTF-8 - the glob selects names that are not UTF-8
/// at all, and rendering one lossily here would match a name D1 never
/// recorded.
fn recorded(applied: &[String], file: &Path) -> bool {
    let name = file_name(file);
    // A leading-dash name is never read as recorded - the fail-closed
    // reading.
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

/// The file name as a message renders it.
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
    fn only_a_scalar_zero_count_reads_as_no_table() {
        use serde_json::json;
        assert!(table_absent(Some(&json!(0))));
        assert!(table_absent(Some(&json!("0"))));
        assert!(!table_absent(Some(&json!(1))));
        assert!(!table_absent(None), "`undefined` is not `0`");
        // Neither is zero - where `${x}` coercion would say "0" for the
        // array and fail open to "no bookkeeping table".
        assert!(!table_absent(Some(&json!([0]))));
        assert!(!table_absent(Some(&json!({ "n": 0 }))));
    }

    #[test]
    fn an_absolute_recorded_name_stays_inside_migrations() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let migrations = temp.path().join("migrations");
        write(&migrations, "0001_init.sql", b"create table a;\n");
        // This tests `migrations//etc/passwd`; a join that discarded
        // the base would test the host's real /etc/passwd and could
        // pass.
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
        let applied = ["-x.sql".to_owned()];
        assert!(!recorded(&applied, Path::new("migrations/-x.sql")));
    }

    #[test]
    fn membership_is_an_exact_whole_line_match() {
        let applied = ["0001_init.sql".to_owned(), String::new()];
        assert!(recorded(&applied, Path::new("migrations/0001_init.sql")));
        // Both ends are anchored: neither a prefix nor a suffix of a
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
        // collation reverses that; the byte order is the one a fresh
        // database replays in.
        let applied = [PathBuf::from("0002_B.sql")];
        assert!(every_pending_sorts_after(&applied, &[PathBuf::from("0002_a.sql")]).is_ok());
        let applied = [PathBuf::from("0002_a.sql")];
        assert!(every_pending_sorts_after(&applied, &[PathBuf::from("0002_B.sql")]).is_err());
    }

    #[test]
    fn the_confirmation_is_one_blank_trimmed_newline_terminated_line() {
        assert!(confirmed("migrate\n"));
        // Leading and trailing blanks are stripped.
        assert!(confirmed("  migrate  \n"));
        assert!(confirmed("\tmigrate\t\n"));

        assert!(!confirmed("Migrate\n"));
        assert!(!confirmed("migrate now\n"));
        assert!(!confirmed("\n"));
        // A CRLF line's `\r` is kept: it is not a blank.
        assert!(!confirmed("migrate\r\n"));
        // End of input.
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
}
