//! Restore drill (`registry/docs/runbook.md`, "Disaster recovery"):
//! proves the latest nightly dump actually restores.  Downloads the
//! dump named by `meta.last_backup_key` from the BACKUP bucket,
//! verifies its sidecar checksum, imports it into a scratch D1
//! database, compares per-table row counts against the live database,
//! spot-checks one version's metadata JSON byte-for-byte, and tears the
//! scratch database down.  Run it after enabling backups and again
//! whenever the dump machinery changes.
//!
//! Row counts can legitimately drift on an active database (the dump is
//! from the last nightly pass); on a quiet registry they match exactly.
//!
//! Requires `CLOUDFLARE_API_TOKEN` in the environment.  The scratch
//! database is `cabin-registry-drill`, and the drill refuses to run
//! when one already exists - that is a previous drill to inspect and
//! delete, not something to write over.
//!
//! Two databases, two namespaces, and they are not interchangeable:
//! the live side is addressed as the binding `DB`, resolved out of
//! `wrangler.jsonc`, and the scratch side by its account-level name.
//! Collapsing them into one identifier would hide exactly the
//! disagreement between config and account that the launch guard
//! exists to catch.
//!
//! Ceilings, where this deliberately stops short of the shell it
//! replaces.  All are fail-closed: the drill stops where the shell
//! carried on, never the reverse.
//!
//! - a `wrangler d1 list` that does not answer with a list of
//!   databases ends the run.  The shell piped it into `node` inside an
//!   `if`, so an expired token, a network fault or a banner on stdout
//!   all threw, and every one of them read as "the scratch database is
//!   not there" - which sent it on to create one;
//! - the sidecar must be SHA-256 and every line of it must parse.
//!   `shasum -a 256 -c` ignores `-a` and infers the algorithm from the
//!   digest's length, so a SHA-1 sidecar passed; it also passed a
//!   sidecar whose one good line sat beside a malformed one.  A name
//!   with a path separator is refused here, where `shasum` resolved it
//!   against the working directory and reported it unreadable;
//! - the teardown is attempted on every path that returns, including a
//!   failed comparison, but not when a signal kills the process, where
//!   the shell's `trap ... EXIT` still fired.  Attempted, not
//!   guaranteed, in both: a delete that itself fails leaves the
//!   database and says nothing, as the trap's `|| true` did.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    BACKUP_BUCKET as BACKUP, Nullish, column_lines, column_text, output, results, step, wrangler,
};

const SCRATCH: &str = "cabin-registry-drill";

/// Enumerated from the LIVE database on purpose: a table the dump
/// failed to carry then fails the scratch-side count below, instead of
/// silently never being compared.
const TABLES: &str = "SELECT name FROM sqlite_master WHERE type = 'table'
  AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
  AND name NOT LIKE '\\_cf\\_%' ESCAPE '\\' ORDER BY name";

const VIEWS: &str = "SELECT name FROM sqlite_master WHERE type = 'view' ORDER BY name";

const SPOT: &str = "SELECT scope || '/' || name || '@' || version || '#' || revision AS pin, \
                    metadata_json
  FROM revisions ORDER BY scope, name, version, revision LIMIT 1";

/// The dump is exported before the job records its own success, so the
/// live `last_backup_at` / `last_backup_key` rows are legitimately
/// newer than the dump (and absent entirely from the first one).
const META_COUNT: &str = "SELECT COUNT(*) AS n FROM meta
      WHERE key NOT IN ('last_backup_at', 'last_backup_key')";

/// One entry of `wrangler d1 list --json`.  The name is optional
/// because the shell compared `db.name === ...` against every entry,
/// where an entry carrying no name is simply not the one being looked
/// for.
#[derive(Deserialize)]
struct Database {
    name: Option<String>,
}

/// The shell's `trap cleanup EXIT`.  Held across the whole drill so
/// that a failed comparison tears the scratch database down as surely
/// as a passing one: what the guard at the top refuses to run past is a
/// database left behind by an earlier drill.
struct Scratch {
    created: bool,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.created {
            return;
        }
        // The trap's `|| true`, with both streams sent to `/dev/null`:
        // a delete that fails leaves the database and says nothing.
        // The teardown on the passing path is the loud one.
        let _ = wrangler(&["d1", "delete", SCRATCH, "-y"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Runs the drill.
///
/// # Errors
///
/// If any wrangler invocation fails, if the dump cannot be resolved or
/// verified, if a scratch database already exists, or if the restored
/// database disagrees with the live one.
pub fn run() -> Result<()> {
    let work = tempfile::tempdir().context("create a working directory")?;

    step("resolving the latest dump from meta.last_backup_key");
    let key = value(
        "DB",
        "SELECT value FROM meta WHERE key = 'last_backup_key'",
        "value",
    )?;
    // Never the sidecar: `governor.sh` admits `.sha256` on the same
    // shape, and this deliberately does not.
    let Some(dump_name) = dump_name(&key) else {
        bail!("meta.last_backup_key is missing or malformed: '{key}' (has a dump run?)");
    };

    step(&format!(
        "downloading {key} and its checksum sidecar from {BACKUP}"
    ));
    let dump = work.path().join(dump_name);
    let sidecar = work.path().join(format!("{dump_name}.sha256"));
    fetch(&format!("{BACKUP}/{key}"), &dump)?;
    fetch(&format!("{BACKUP}/{key}.sha256"), &sidecar)?;
    verify(work.path(), &sidecar).context("dump checksum verification failed")?;

    step(&format!("creating the scratch database {SCRATCH}"));
    if exists()? {
        bail!("{SCRATCH} already exists (a previous drill?); inspect and delete it first");
    }
    // The status, not the output: `output` decodes stdout as UTF-8
    // after the create has already succeeded, and an undecodable byte
    // there would return before the guard below exists - leaking the
    // database the shell's `created_scratch=1` armed against.
    quiet(&["d1", "create", SCRATCH])?;
    let mut scratch = Scratch { created: true };

    step(&format!("importing the dump into {SCRATCH}"));
    let path = dump
        .to_str()
        .context("the working directory is not UTF-8")?;
    loud(&["d1", "execute", SCRATCH, "--remote", "--file", path, "-y"])?;

    step("comparing per-table row counts against the live database");
    counts()?;

    step("comparing views against the live database");
    views()?;

    step("spot-checking one version's metadata JSON");
    spot_check()?;

    step(&format!("tearing down {SCRATCH}"));
    quiet(&["d1", "delete", SCRATCH, "-y"])?;
    scratch.created = false;

    println!("restore drill OK ({key})");
    Ok(())
}

/// The object's own name, for a key matching exactly what the shell's
/// `^d1/[0-9]{4}-[0-9]{2}-[0-9]{2}\.sql$` matched: the whole string,
/// the `.sql` suffix literal, and no date validation - `0000-99-99`
/// passed there and passes here.
fn dump_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("d1/")?;
    let date = name.strip_suffix(".sql")?;
    let [year, month, day] = <[&str; 3]>::try_from(date.split('-').collect::<Vec<_>>()).ok()?;
    (year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit())))
    .then_some(name)
}

/// `wrangler r2 object get`.  The `r2 object` commands default to
/// local state, so `--remote` is what makes them reach the deployment.
fn fetch(object: &str, file: &Path) -> Result<()> {
    let file = file
        .to_str()
        .context("the working directory is not UTF-8")?;
    loud(&["r2", "object", "get", object, "--file", file, "--remote"])
}

/// A wrangler invocation the shell did not redirect, so its own output
/// stayed on the operator's terminal.  The download and the import are
/// the drill's two slow steps, and that output is the only sign of life
/// during them - capturing it, as the reads do, would leave the drill
/// looking hung.
fn loud(arguments: &[&str]) -> Result<()> {
    status(&mut wrangler(arguments))
}

/// A wrangler invocation the shell sent to `/dev/null`, kept apart from
/// [`output`] because these are run for their exit status alone -
/// nothing downstream reads a byte of what they print.
fn quiet(arguments: &[&str]) -> Result<()> {
    status(wrangler(arguments).stdout(Stdio::null()))
}

fn status(command: &mut std::process::Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command.status().with_context(|| format!("run {program}"))?;
    if !status.success() {
        bail!("{program} failed: {status}");
    }
    Ok(())
}

/// `shasum -a 256 -c`, over the sidecar the backup job writes:
/// `<64 hex><two spaces><name>`, with each name resolved against the
/// directory the dump was downloaded into.
fn verify(directory: &Path, sidecar: &Path) -> Result<()> {
    let text =
        std::fs::read_to_string(sidecar).with_context(|| format!("read {}", sidecar.display()))?;
    let mut checked = 0_u32;
    // `split`, not `lines`: the latter drops a `\r` before the newline,
    // where `shasum -c` kept it as part of the filename and then failed
    // to open that file. Accepting a CRLF sidecar it refused is the one
    // direction this must not diverge in.
    for line in text.split('\n').filter(|line| !line.is_empty()) {
        let (hex, rest) = line
            .split_once(' ')
            .context("a sidecar line carries no checksum and name")?;
        // `shasum -c` takes two spaces or a ` *` binary marker, and
        // refuses the single-space form outright.
        let name = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .context("a sidecar line separates its name by one space")?;
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("a sidecar line carries no sha256 digest: {hex}");
        }
        if name.is_empty() || name.contains(['/', '\\']) {
            bail!("a sidecar names a file outside the download: {name}");
        }
        // Streamed, because a dump is bounded only by the database it
        // came from and `shasum` never held one whole.
        let digest = std::fs::File::open(directory.join(name))
            .and_then(cabin_core::hash::hash_reader)
            .with_context(|| format!("read the file the sidecar names: {name}"))?;
        if !digest.eq_ignore_ascii_case(hex) {
            bail!("{name} does not match its recorded digest");
        }
        // `shasum -c`'s own line, which was never redirected: the
        // recorded drill in `registry/docs/verification.md` shows it
        // between the download and the scratch-database step.
        println!("{name}: OK");
        checked += 1;
    }
    if checked == 0 {
        bail!("no properly formatted sha checksum lines found");
    }
    Ok(())
}

/// Whether the account already holds the scratch database.
fn exists() -> Result<bool> {
    let answer = output(&mut wrangler(&["d1", "list", "--json"]))?;
    let databases: Vec<Database> =
        serde_json::from_str(&answer).context("parse the database list")?;
    Ok(databases
        .iter()
        .any(|database| database.name.as_deref() == Some(SCRATCH)))
}

/// One column of one remote query, as `$(...)` handed it to the shell.
fn value(database: &str, sql: &str, column: &str) -> Result<String> {
    Ok(lines(database, sql, column)?.join("\n"))
}

/// The same read, as the `while IFS= read -r` loop saw it.
fn lines(database: &str, sql: &str, column: &str) -> Result<Vec<String>> {
    Ok(column_lines(&rows(database, sql)?, column, Nullish::Empty))
}

fn rows(database: &str, sql: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        database,
        "--remote",
        "--json",
        "--command",
        sql,
    ]))?;
    results(&answer)
}

/// Per-table row counts, live against restored.
fn counts() -> Result<()> {
    let tables = value("DB", TABLES, "name")?;
    if tables.is_empty() {
        bail!("the live database contains no tables");
    }
    let mut mismatch = false;
    for table in tables.split('\n') {
        if table.is_empty()
            || !table
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            bail!("unexpected table name: {table}");
        }
        let count = if table == "meta" {
            META_COUNT.to_owned()
        } else {
            format!("SELECT COUNT(*) AS n FROM \"{table}\"")
        };
        let live = value("DB", &count, "n")?;
        let restored = value(SCRATCH, &count, "n")?;
        mismatch |= live != restored;
        println!("{}", count_line(table, &live, &restored));
    }
    if mismatch {
        bail!(
            "row counts differ (drift since the dump, or an incomplete restore - \
             compare timestamps)"
        );
    }
    Ok(())
}

/// Views carry no rows, so the count loop cannot notice a dump that
/// lost one - and the restored `d1_migrations` table keeps migration
/// 0001 from ever recreating it.  Every read projection resolves served
/// revisions through `current_revisions`, so every live view must be in
/// the restore.
fn views() -> Result<()> {
    let live = value("DB", VIEWS, "name")?;
    let restored = value(SCRATCH, VIEWS, "name")?;
    if live.is_empty() {
        bail!("the live database contains no views (current_revisions is required)");
    }
    if live != restored {
        bail!("views differ: live [{live}], restored [{restored}]");
    }
    println!("{}", views_line(&live));
    Ok(())
}

/// One row of the count table, as
/// `printf '    %-28s live %6s  restored %6s%s\n'` laid it out: minimum
/// widths, so an over-long name pushes the columns right rather than
/// being truncated, and the marker carries its own leading space.
fn count_line(table: &str, live: &str, restored: &str) -> String {
    let marker = if live == restored { "" } else { " <- MISMATCH" };
    format!("    {table:<28} live {live:>6}  restored {restored:>6}{marker}")
}

/// The matched-views line.  `tr '\n' ' '` ran over a here-string, so
/// the newline it appended became the trailing space this keeps.
fn views_line(views: &str) -> String {
    format!("    views match: {} ", views.replace('\n', " "))
}

/// One version's metadata JSON, byte for byte.
fn spot_check() -> Result<()> {
    let live_pin = value("DB", SPOT, "pin")?;
    // A NULL anywhere in the concatenation makes the whole pin NULL,
    // which reaches here as the empty string no-rows does. Both take
    // this branch, as they did in the shell.
    if live_pin.is_empty() {
        println!("    no versions in the live database; nothing to spot-check");
        return Ok(());
    }
    let restored_pin = value(SCRATCH, SPOT, "pin")?;
    if live_pin != restored_pin {
        bail!("spot-check row differs: live {live_pin}, restored {restored_pin}");
    }
    // The shell redirected each read into a file and ran `cmp -s`, so
    // the comparison is over the raw bytes the read loop wrote - one
    // trailing newline per row included, which is why the byte count
    // below is one more than the JSON's own length.
    let live = column_text(&rows("DB", SPOT)?, "metadata_json", Nullish::Empty);
    let restored = column_text(&rows(SCRATCH, SPOT)?, "metadata_json", Nullish::Empty);
    if live != restored {
        bail!("metadata_json for {live_pin} differs between live and restored");
    }
    serde_json::from_str::<serde_json::Value>(&restored)
        .with_context(|| format!("restored metadata_json for {live_pin} is not valid JSON"))?;
    println!(
        "    {live_pin}: metadata_json matches and parses ({} bytes)",
        restored.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Every expectation below was taken from the shell the port
    //! replaces, run over the same fixtures.  They live here rather
    //! than in `tests/` because none of what they exercise - the dump
    //! key's grammar, the sidecar check, the two rendered lines - is
    //! this crate's API.

    use super::*;

    /// Exactly the keys the shell's ERE matched, and no others.  The
    /// sidecar key is the one that matters: `governor.sh` admits it on
    /// the same shape, and a drill that imported a 64-byte checksum
    /// file as a dump would restore an empty database and then report
    /// every table as a mismatch.
    #[test]
    fn the_dump_key_admits_only_a_padded_date() {
        assert_eq!(dump_name("d1/2026-08-04.sql"), Some("2026-08-04.sql"));
        // No semantic date validation, exactly as the shell had none.
        assert_eq!(dump_name("d1/0000-99-99.sql"), Some("0000-99-99.sql"));
        for refused in [
            "d1/2026-08-04.sql.sha256",
            "d1/2026-8-4.sql",
            "d1/2026-08-04.SQL",
            "d1/2026-08-04Xsql",
            "d1/20260804.sql",
            "d1/2026-08-04-.sql",
            "xd1/2026-08-04.sqlx",
            "2026-08-04.sql",
            "d1/2026-08-04.sql ",
            " d1/2026-08-04.sql",
            "",
        ] {
            assert_eq!(dump_name(refused), None, "accepted {refused:?}");
        }
    }

    /// The count line's widths are minimums, as `printf`'s were: an
    /// over-long name pushes the columns right rather than being cut,
    /// and the marker carries its own leading space.
    #[test]
    fn the_count_line_lays_out_as_printf_laid_it_out() {
        assert_eq!(
            count_line("revisions", "12", "12"),
            "    revisions                    live     12  restored     12"
        );
        assert_eq!(
            count_line("meta", "3", "4"),
            "    meta                         live      3  restored      4 <- MISMATCH"
        );
        assert_eq!(
            count_line("a_very_long_table_name_that_exceeds_28", "1234567890", "2"),
            "    a_very_long_table_name_that_exceeds_28 live 1234567890  \
             restored      2 <- MISMATCH"
        );
    }

    /// `tr '\n' ' '` ran over a here-string, so the newline it appended
    /// became a trailing space.  `views.join(" ")` would not produce
    /// it.
    #[test]
    fn the_views_line_keeps_the_trailing_space() {
        assert_eq!(
            views_line("current_revisions\nlatest"),
            "    views match: current_revisions latest "
        );
        assert_eq!(
            views_line("current_revisions"),
            "    views match: current_revisions "
        );
    }

    /// Every sidecar shape `shasum -a 256 -c` accepted over the one the
    /// backup job writes, and the malformed ones it refused.
    #[test]
    fn the_sidecar_check_takes_what_shasum_took() {
        let work = tempfile::tempdir().unwrap();
        let dump = work.path().join("2026-08-04.sql");
        std::fs::write(&dump, b"CREATE TABLE t (a);\n").unwrap();
        let digest = cabin_core::hash::hash_reader(&b"CREATE TABLE t (a);\n"[..]).unwrap();

        let check = |sidecar: &str| {
            let path = work.path().join("2026-08-04.sql.sha256");
            std::fs::write(&path, sidecar).unwrap();
            verify(work.path(), &path).is_ok()
        };

        // The producer's own form: 64 lower-case hex, two spaces, the
        // bare object name (`registry/src/backup_glue.rs`).
        assert!(check(&format!("{digest}  2026-08-04.sql\n")));
        assert!(
            check(&format!("{digest}  2026-08-04.sql")),
            "no trailing newline"
        );
        assert!(check(&format!(
            "{}  2026-08-04.sql\n",
            digest.to_uppercase()
        )));
        assert!(
            check(&format!("{digest} *2026-08-04.sql\n")),
            "binary marker"
        );

        for refused in [
            format!("{}  2026-08-04.sql\n", "0".repeat(64)),
            // `shasum -c` refuses the single-space form outright.
            format!("{digest} 2026-08-04.sql\n"),
            format!("{digest}  missing.sql\n"),
            // The shell resolved every name against the download
            // directory; one that reaches outside it is refused here.
            format!("{digest}  d1/2026-08-04.sql\n"),
            String::new(),
            "not a checksum at all\n".to_owned(),
            // `shasum -c` kept the `\r` as part of the filename and
            // failed to open it; `str::lines` would drop it and accept.
            format!("{digest}  2026-08-04.sql\r\n"),
        ] {
            assert!(!check(&refused), "accepted {refused:?}");
        }
    }
}
