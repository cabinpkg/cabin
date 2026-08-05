//! The migrations deploy gate, ported one-to-one from the `run:` body
//! of the "Skip until changed D1 migrations are applied by hand" step
//! of `.github/workflows/registry.yml`.
//!
//! ```text
//! L1  stamp=$(cat registry/migrations/*.sql | sha256sum | cut -d' ' -f1)
//! L2  if [ "$stamp" != "$(cat registry/migrations-applied)" ]; then
//! L3    echo "pending=true" >> "$GITHUB_OUTPUT"
//! L4  fi
//! ```
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior. Each was pinned by running the original
//! under `bash -e`, GitHub's default `run:` shell (`-e` on, `-u` and
//! `-o pipefail` off):
//!
//! - **Every read fails toward "pending", never toward an error.**
//!   L1's pipeline ends in `sha256sum | cut`, which succeed whatever
//!   `cat` did, so an empty `migrations/` (the glob stays unexpanded
//!   and `cat` fails on the literal pattern), a missing directory, or
//!   an unreadable entry leaves a diagnostic on stderr and hashes
//!   whatever bytes were read, a prefix delivered before a mid-file
//!   failure included (`cat` streams) - an empty selection stamps as
//!   the digest of empty input, not as an error. L2's substitution sits in
//!   the `if` condition, where `set -e` is suppressed: a missing
//!   `migrations-applied` compares as empty. Nothing can fail the step
//!   except L3's redirect.
//! - **The glob's selection and order.** `*.sql` skips dotfiles, is
//!   case-sensitive, matches bytes rather than UTF-8 (bash falls back
//!   to byte matching for an invalid multibyte basename), matches a
//!   directory (whose `cat` diagnoses and contributes nothing), and
//!   expands in the runner's `C.UTF-8` collation, which is byte
//!   order. [`migration_files`] is that rule;
//!   the diagnose bundle (`xtask-registry-admin`) consumes the same
//!   function so the two readings of the stamp cannot drift.
//! - **The comparison is bytes.** `$(...)` drops NUL bytes and strips
//!   every trailing newline; whatever else `migrations-applied` holds
//!   (a CRLF ending included) must equal the stamp's lowercase hex
//!   bytes exactly.
//! - **L3 is reachable only in the pending case**, so an unset
//!   `$GITHUB_OUTPUT` fails the step there and nowhere else - the
//!   shell's `>> ""` exited 1 the same way.
//!
//! Stated ceiling: the shell's stderr diagnostics carry `cat`'s
//! wording and this port's carry its own; `registry.yml` reads the
//! step's output, never its stderr. The shell's collation follows the
//! locale (`en_US.UTF-8` sorts `0002_a.sql` before `0002_B.sql`); the
//! port is fixed to the byte order the runner's `C.UTF-8` produces.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use sha2::{Digest as _, Sha256};

/// L3's line, byte for byte.
const OUTPUT_LINE: &str = "pending=true\n";

/// Answer whether the committed D1 migrations still match the stamp
/// the operator recorded in `registry/migrations-applied`, recording
/// `pending=true` in `$GITHUB_OUTPUT` when they do not.
///
/// # Errors
///
/// Only when `$GITHUB_OUTPUT` is unusable in the pending case (L3).
pub fn run() -> Result<()> {
    run_in(Path::new("."))
}

/// The whole gate against one repository root; [`run`] passes the
/// working directory the way the shell's relative paths did.
fn run_in(base: &Path) -> Result<()> {
    if stamp(base).into_bytes() == applied(base) {
        return Ok(());
    }
    crate::append_github_output(OUTPUT_LINE)
}

/// The files `migrations/*.sql` expands to, in the order the shell
/// concatenated them.
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
        // The glob matches bytes, not UTF-8: bash falls back to byte
        // matching for an invalid multibyte basename, so requiring
        // `to_str` here would silently drop a migration the shell
        // counted. And it skips dotfiles unless `dotglob` is set, so
        // an operator's `.draft.sql` scratch file is outside the
        // stamp. `registry.yml`'s deploy gate and `scripts/migrate.sh`
        // hash the same glob; counting one here would report PENDING
        // while deploys stay unblocked.
        let name = name.as_encoded_bytes();
        !name.starts_with(b".") && name.ends_with(b".sql")
    });
    // A glob expands sorted; `read_dir` does not.
    files.sort();
    Ok(files)
}

/// L1: the digest of the migrations' concatenation, as lowercase hex.
/// Selection or read failures diagnose to stderr and contribute
/// nothing, exactly as `cat`'s did under the pipeline's swallowed
/// status.
fn stamp(base: &Path) -> String {
    let directory = base.join("registry/migrations");
    let files = match migration_files(&directory) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("{error:#}");
            Vec::new()
        }
    };
    if files.is_empty() {
        // The shell's glob stays unexpanded here and `cat` fails on
        // the literal pattern; the pipeline hashes empty input either
        // way.
        eprintln!("no migrations match {}/*.sql", directory.display());
    }
    let mut hasher = Sha256::new();
    for file in files {
        // `cat` streams: bytes read before an error have already
        // reached the pipeline, and the failure only moves it to the
        // next operand - so no whole-file read that would discard a
        // partially delivered prefix.
        if let Err(error) = hash_file(&mut hasher, &file) {
            eprintln!("{}: {error}", file.display());
        }
    }
    cabin_core::hash::hex_digest(&hasher.finalize())
}

fn hash_file(hasher: &mut Sha256, file: &Path) -> std::io::Result<()> {
    hash_reader(hasher, &mut std::fs::File::open(file)?)
}

/// Feeds `reader` into `hasher` until end of input or the first real
/// error; whatever arrived before the error stays in the digest.
fn hash_reader(hasher: &mut Sha256, reader: &mut dyn std::io::Read) -> std::io::Result<()> {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => hasher.update(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// L2's right-hand side: the stamp file's bytes through `$(...)`, or
/// the empty string a failing `cat` substituted.
fn applied(base: &Path) -> Vec<u8> {
    let path = base.join("registry/migrations-applied");
    match std::fs::read(&path) {
        Ok(bytes) => crate::substitute(bytes),
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest of empty input: what an empty selection stamps as.
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
        // `cat`'s per-entry failure is the reader's concern, not the
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
            "bash byte-matches an invalid multibyte basename against *.sql"
        );
    }

    #[test]
    fn a_partial_read_stays_in_the_digest() {
        /// Delivers a prefix, then fails - the shape `cat` streams
        /// through: the prefix has already reached the pipeline.
        struct FailsAfter(&'static [u8]);
        impl std::io::Read for FailsAfter {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Err(std::io::Error::other("injected read failure"));
                }
                let given = self.0.len().min(buffer.len());
                buffer[..given].copy_from_slice(&self.0[..given]);
                self.0 = &self.0[given..];
                Ok(given)
            }
        }

        let mut streamed = Sha256::new();
        streamed.update(b"before");
        hash_reader(&mut streamed, &mut FailsAfter(b"prefix")).expect_err("the injected failure");
        let mut expected = Sha256::new();
        expected.update(b"beforeprefix");
        assert_eq!(
            cabin_core::hash::hex_digest(&streamed.finalize()),
            cabin_core::hash::hex_digest(&expected.finalize()),
            "bytes read before the error have already reached the digest"
        );
    }

    #[test]
    fn an_empty_selection_stamps_as_the_digest_of_empty_input() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        std::fs::create_dir_all(temp.path().join("registry/migrations"))
            .expect("the empty migrations directory");
        assert_eq!(stamp(temp.path()), EMPTY_STAMP);
        // A missing directory reads the same way.
        let gone = assert_fs::TempDir::new().expect("a scratch directory");
        std::fs::create_dir_all(gone.path().join("registry")).expect("the registry directory");
        assert_eq!(stamp(gone.path()), EMPTY_STAMP);
    }

    #[test]
    fn the_stamp_concatenates_in_selection_order() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let migrations = temp.path().join("registry/migrations");
        write(&migrations, "0001_a.sql", b"one");
        write(&migrations, "0002_b.sql", b"two");
        let mut hasher = Sha256::new();
        hasher.update(b"onetwo");
        assert_eq!(
            stamp(temp.path()),
            cabin_core::hash::hex_digest(&hasher.finalize())
        );
    }

    #[test]
    fn the_applied_stamp_reads_like_a_command_substitution() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        let registry = temp.path().join("registry");
        write(&registry, "migrations-applied", b"abc\n\n\n");
        assert_eq!(applied(temp.path()), b"abc");
        write(&registry, "migrations-applied", b"ab\0c\n");
        assert_eq!(applied(temp.path()), b"abc");
        // The `\r` of a CRLF ending survives into the comparison.
        write(&registry, "migrations-applied", b"abc\r\n");
        assert_eq!(applied(temp.path()), b"abc\r");
    }

    #[test]
    fn a_missing_applied_stamp_compares_as_empty() {
        let temp = assert_fs::TempDir::new().expect("a scratch directory");
        assert!(applied(temp.path()).is_empty());
    }

    #[test]
    fn the_recorded_line_matches_the_shells_echo() {
        assert_eq!(OUTPUT_LINE.as_bytes(), b"pending=true\n");
    }
}
