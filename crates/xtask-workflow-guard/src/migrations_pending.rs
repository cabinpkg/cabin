//! The migrations deploy gate: the "Block the deploy until changed D1
//! migrations are applied by hand" step of
//! `.github/workflows/registry.yml`.
//!
//! This module owns the stamp: [`stamp`], [`digest`], [`applied`] and
//! [`current`] are also how `cargo registry-migrate`,
//! `cargo registry-wipe` and `cargo registry-diagnose`
//! (`xtask-registry-admin`) compute and read it, so no two readings of
//! the stamp can drift.
//!
//! - **Anything but a matching stamp fails the step.** A stamp that no
//!   longer matches the migrations, and every state in which the gate
//!   cannot establish the comparison - an empty, missing or unreadable
//!   `migrations/`, an unreadable migration, an unreadable
//!   `migrations-applied` - prints one `::error` annotation naming the
//!   cause and the by-hand recovery and exits 1, so the deploy job goes
//!   red instead of green with its Deploy step skipped: a deploy that
//!   did not happen never reads as one that did. The gate never applies
//!   a migration or touches the stamp; a matching stamp prints nothing.
//! - **The glob's selection and order.** `*.sql` skips dotfiles, is
//!   case-sensitive, matches bytes rather than UTF-8, matches a
//!   directory (whose read then fails), and sorts in byte order.
//!   [`migration_files`] is that rule.
//! - **The comparison is bytes.** `migrations-applied` loses its NUL
//!   bytes and every trailing newline; whatever else it holds (a CRLF
//!   ending included) must equal the stamp's lowercase hex bytes
//!   exactly.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

const APPLY: &str = "Apply the changed migrations by hand from the repository root with \
     `cargo registry-migrate --remote`, which applies the pending files, confirms D1 reports \
     nothing left pending, and only then refreshes the stamp; land that stamp change through a \
     PR, whose main push deploys and supersedes this run (pre-launch, an edited already-applied \
     migration ships through `cargo registry-wipe` instead)";
const REPAIR: &str = "Repair what the error names through a PR, whose main push reruns this gate \
     and supersedes this run";
const NEVER: &str = "Never edit the stamp by hand. registry/docs/runbook.md, \"Deploy skew\"";

/// Pass when the committed D1 migrations still match the stamp the
/// operator recorded in `registry/migrations-applied`.
///
/// # Errors
///
/// When they do not, or when the comparison cannot be made: the
/// deploy it gates is blocked.
pub fn run() -> Result<()> {
    run_in(Path::new("."))
}

/// The whole gate against one repository root; [`run`] passes the
/// working directory.
fn run_in(base: &Path) -> Result<()> {
    let (cause, recovery) = match current(&base.join("registry")) {
        Ok(true) => return Ok(()),
        Ok(false) => (
            "registry/migrations/ no longer matches the registry/migrations-applied stamp, so \
             the live D1 schema may predate this Worker"
                .to_owned(),
            APPLY,
        ),
        Err(error) => (
            format!(
                "the gate cannot compare registry/migrations/ with its stamp ({error:#}), so \
                 the live D1 schema is unknown"
            ),
            REPAIR,
        ),
    };
    println!("::error title=Registry deploy blocked::{cause}. {recovery}. {NEVER}.");
    bail!("deploy blocked: {cause}")
}

/// Whether `<registry>/migrations-applied` records the stamp of
/// `<registry>/migrations/`. Every caller reads an error as pending.
///
/// # Errors
///
/// If either side cannot be read; see [`stamp`] and [`applied`].
pub fn current(registry: &Path) -> Result<bool> {
    Ok(stamp(registry)?.as_bytes() == applied(registry)?)
}

/// The files `migrations/*.sql` expands to, in byte order.
///
/// # Errors
///
/// If the directory cannot be read.
pub fn migration_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("read {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .with_context(|| format!("read {}", directory.display()))?;
    files.retain(|path| {
        let Some(name) = path.file_name() else {
            return false;
        };
        // The glob matches bytes, not UTF-8: requiring `to_str` here
        // would silently drop a migration with a non-UTF-8 name. And
        // it skips dotfiles, so an operator's `.draft.sql` scratch
        // file is outside the stamp.
        let name = name.as_encoded_bytes();
        !name.starts_with(b".") && name.ends_with(b".sql")
    });
    // A glob expands sorted; `read_dir` does not.
    files.sort();
    Ok(files)
}

/// The [`digest`] of `<registry>/migrations/*.sql`, which
/// `migrations-applied` records as `"{stamp}\n"`.
///
/// # Errors
///
/// If the directory cannot be read, or as [`digest`].
pub fn stamp(registry: &Path) -> Result<String> {
    digest(&migration_files(&registry.join("migrations"))?)
}

/// The SHA-256 of `files` concatenated in the given order, as
/// lowercase hex.
///
/// # Errors
///
/// If `files` is empty or one of them cannot be read.
pub fn digest(files: &[PathBuf]) -> Result<String> {
    if files.is_empty() {
        bail!("no migrations match migrations/*.sql");
    }
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(std::fs::read(file).with_context(|| format!("read {}", file.display()))?);
    }
    Ok(cabin_core::hash::hex_digest(&hasher.finalize()))
}

/// `<registry>/migrations-applied` with its NUL bytes and trailing
/// newlines dropped.
///
/// # Errors
///
/// If the file cannot be read.
pub fn applied(registry: &Path) -> Result<Vec<u8>> {
    let path = registry.join("migrations-applied");
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(crate::substitute(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest of empty input, which an empty selection must never
    /// be read as matching.
    const EMPTY_STAMP: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn write(dir: &Path, name: &str, contents: &[u8]) {
        std::fs::create_dir_all(dir).expect("the fixture directory");
        std::fs::write(dir.join(name), contents).expect("the fixture file");
    }

    #[test]
    fn the_selection_is_the_glob() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let dir = temp.path();
        write(dir, "0001_a.sql", b"a");
        write(dir, ".draft.sql", b"hidden");
        write(dir, "0002_b.SQL", b"upper");
        write(dir, "notes.txt", b"prose");
        write(dir, "sql", b"bare");
        std::fs::create_dir(dir.join("0003_dir.sql")).expect("a directory entry");

        let names: Vec<String> = migration_files(dir)
            .expect("a readable directory")
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // The directory entry is selected - the glob matches it, and
        // its read failure is the reader's concern, not the
        // selection's.
        assert_eq!(names, ["0001_a.sql", "0003_dir.sql"]);
    }

    #[test]
    fn the_order_is_bytewise_like_the_runners_c_utf8() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let dir = temp.path();
        write(dir, "0002_a.sql", b"lower");
        write(dir, "0002_B.sql", b"upper");
        let names: Vec<String> = migration_files(dir)
            .expect("a readable directory")
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // `B` (0x42) sorts before `a` (0x61); an en_US collation would
        // reverse them, which is the ceiling in the module docs.
        assert_eq!(names, ["0002_B.sql", "0002_a.sql"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_name_matches_by_bytes() {
        use std::os::unix::ffi::OsStringExt as _;
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let name = std::ffi::OsString::from_vec(b"0002_legacy\xe9.sql".to_vec());
        if std::fs::write(temp.path().join(&name), b"latin1").is_err() {
            // APFS refuses non-UTF-8 names outright; the runner's ext4
            // (where the gate actually runs) does not.
            eprintln!("skipping: this filesystem refuses non-UTF-8 names");
            return;
        }
        write(temp.path(), "0001_a.sql", b"ascii");
        let files = migration_files(temp.path()).expect("a readable directory");
        assert_eq!(
            files
                .iter()
                .map(|path| path.file_name().unwrap().to_owned())
                .collect::<Vec<_>>(),
            ["0001_a.sql".into(), name],
            "an invalid multibyte basename still matches *.sql"
        );
    }

    #[test]
    fn the_stamp_concatenates_in_selection_order() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let migrations = temp.path().join("migrations");
        write(&migrations, "0001_a.sql", b"one");
        write(&migrations, "0002_b.sql", b"two");
        let mut hasher = Sha256::new();
        hasher.update(b"onetwo");
        assert_eq!(
            stamp(temp.path()).expect("the stamp"),
            cabin_core::hash::hex_digest(&hasher.finalize())
        );
    }

    #[test]
    fn the_applied_stamp_drops_nuls_and_trailing_newlines() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let registry = temp.path();
        let read = || applied(registry).expect("the stamp file");
        write(registry, "migrations-applied", b"abc\n\n\n");
        assert_eq!(read(), b"abc");
        write(registry, "migrations-applied", b"ab\0c\n");
        assert_eq!(read(), b"abc");
        // The `\r` of a CRLF ending survives into the comparison.
        write(registry, "migrations-applied", b"abc\r\n");
        assert_eq!(read(), b"abc\r");
    }

    #[test]
    fn an_empty_selection_is_never_current() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let registry = temp.path();
        std::fs::create_dir(registry.join("migrations")).expect("the empty migrations directory");
        write(
            registry,
            "migrations-applied",
            format!("{EMPTY_STAMP}\n").as_bytes(),
        );
        current(registry).expect_err("an empty selection has no stamp");
    }
}
