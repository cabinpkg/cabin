//! The release checksums, ported one-to-one from the `run:` body of
//! the "Generate checksums" step of `.github/workflows/dist.yml`.
//!
//! ```text
//! L1  set -euo pipefail
//! L2  shopt -s nullglob
//! L3  files=(*.tar.xz *.zip)
//! L4  ((${#files[@]})) || { echo "no binary archives found" >&2; exit 1; }
//! L5  : > sha256.sum
//! L6  for file in "${files[@]}"; do
//! L7    sha256sum -b "$file" | tee "${file}.sha256" >> sha256.sum
//! L8  done
//! L9  cat sha256.sum
//! ```
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior. Each was pinned by running the original
//! under the step's own options (`-e`, `-u` and `-o pipefail` all on):
//!
//! - **The order is two glob groups, not one sorted list.** L3 expands
//!   `*.tar.xz` and `*.zip` separately, so every `.tar.xz` (sorted)
//!   precedes every `.zip` (sorted) - `a.tar.xz c.tar.xz b.zip`, where
//!   a single sorted list would interleave. Each group skips dotfiles,
//!   matches bytes rather than UTF-8, and includes a directory whose
//!   name matches; sorting is byte order, the runner's `C.UTF-8`
//!   collation.
//! - **An empty selection refuses before touching anything.** L4 runs
//!   before L5, so `sha256.sum` is not even truncated: the bare
//!   sentence goes to stderr and the step exits 1.
//! - **A failing hash dies mid-state.** `pipefail` makes L7's pipeline
//!   carry `sha256sum`'s failure (a directory, an unreadable file), so
//!   the step dies with earlier files' lines already in `sha256.sum`
//!   and the failing file's `.sha256` already created *empty* - the
//!   redirections open when the pipeline spawns, before any hashing.
//!   An unopenable `.sha256` conversely does not stop the hash: `tee`
//!   diagnoses, still forwards the line into `sha256.sum`, and only
//!   then fails the pipeline. L9 never runs either way, so nothing
//!   reaches stdout.
//! - **The line is `sha256sum -b`'s, byte for byte:** 64 lowercase hex
//!   characters, one space, `*`, the file name exactly as the glob
//!   produced it, newline. `tee` sends the same line to `<file>.sha256`
//!   and appends it to `sha256.sum`; L9 then prints the accumulated
//!   file to stdout, which is the step's log contract.
//!
//! Stated ceilings:
//!
//! - **The digest is computed here, not by `sha256sum`.** The line's
//!   bytes are fully determined (hex, space, `*`, name, newline), so
//!   unlike the archivers there is no byte contract only the tool can
//!   satisfy.
//! - **GNU escaping is not reproduced.** `sha256sum` prefixes the line
//!   with `\` and escapes the name when it contains a backslash or
//!   newline. The archives here are `cabin-<version>-<target>`
//!   packages whose names the workflow itself built; a name that
//!   needs escaping cannot reach this step.
//! - **Diagnostic wording on the failing paths is the port's own**
//!   (`sha256sum`'s was the shell's), and the exit status collapses
//!   to 1 where the shell propagated the tool's code. L4's sentence is
//!   reproduced exactly - it is the step's, not a tool's.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use sha2::{Digest as _, Sha256};

/// L4's line, byte for byte; `echo` adds the newline.
const NO_ARCHIVES: &str = "no binary archives found";

/// L5's target and L7's `>>` destination.
const SUM: &str = "sha256.sum";

/// L3's two patterns, in expansion order.
const GROUPS: [&[u8]; 2] = [b".tar.xz", b".zip"];

/// Checksum every release archive in the working directory, exactly as
/// the step did from its `working-directory: artifacts`.
#[must_use]
pub fn run() -> ExitCode {
    run_in(Path::new("."))
}

/// The whole step against one directory; [`run`] passes the working
/// directory the way the shell's relative paths did.
fn run_in(root: &Path) -> ExitCode {
    let files = match archives(root) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("{}: {error}", root.display());
            return ExitCode::FAILURE;
        }
    };
    // L4 comes before L5: nothing is truncated for an empty selection.
    if files.is_empty() {
        eprintln!("{NO_ARCHIVES}");
        return ExitCode::FAILURE;
    }

    // L5.
    let sum_path = root.join(SUM);
    let mut sum = match fs::File::create(&sum_path) {
        Ok(sum) => sum,
        Err(error) => {
            eprintln!("{SUM}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut log = Vec::new();
    for file in files {
        // The pipeline's redirections open when it spawns: the
        // `.sha256` exists, empty, before a byte is hashed. An open
        // that fails does not stop the hashing - `tee` diagnoses,
        // still forwards the line into `sha256.sum`, and only then
        // fails the pipeline (measured: an `Is a directory` side file
        // leaves the line accumulated and the step dead).
        let mut side_name = file.clone();
        side_name.push(".sha256");
        let side = fs::File::create(root.join(&side_name));

        let digest = match hash(&root.join(&file)) {
            Ok(digest) => digest,
            Err(error) => {
                eprintln!("{}: {error}", Path::new(&file).display());
                return ExitCode::FAILURE;
            }
        };

        // `sha256sum -b`'s line: hex, space, `*`, the name, newline.
        let mut line = digest.into_bytes();
        line.extend_from_slice(b" *");
        line.extend_from_slice(file.as_encoded_bytes());
        line.push(b'\n');
        if let Err(error) = sum.write_all(&line) {
            eprintln!("{SUM}: {error}");
            return ExitCode::FAILURE;
        }
        log.extend_from_slice(&line);
        let side_write = side.and_then(|mut side| side.write_all(&line));
        if let Err(error) = side_write {
            eprintln!("{}: {error}", Path::new(&side_name).display());
            return ExitCode::FAILURE;
        }
    }

    // L9: the accumulated file is the step's stdout.
    let mut stdout = std::io::stdout();
    if stdout
        .write_all(&log)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// L3: the two glob groups, each sorted, dotfiles skipped, names
/// matched and ordered by the bytes the glob saw ([`OsString`]'s own
/// order on Unix).
fn archives(root: &Path) -> std::io::Result<Vec<OsString>> {
    let mut names: Vec<OsString> = Vec::new();
    for entry in fs::read_dir(root)? {
        names.push(entry?.file_name());
    }
    let mut files = Vec::new();
    for group in GROUPS {
        let mut matched: Vec<OsString> = names
            .iter()
            .filter(|name| {
                let name = name.as_encoded_bytes();
                !name.starts_with(b".") && name.ends_with(group)
            })
            .cloned()
            .collect();
        matched.sort();
        files.extend(matched);
    }
    Ok(files)
}

/// One file's digest as 64 lowercase hex characters, streamed so a
/// mid-file failure dies exactly where `sha256sum`'s did.
fn hash(file: &Path) -> std::io::Result<String> {
    let mut reader = fs::File::open(file)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match std::io::Read::read(&mut reader, &mut buffer) {
            Ok(0) => return Ok(cabin_core::hash::hex_digest(&hasher.finalize())),
            Ok(count) => hasher.update(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;

    use super::*;

    fn write(root: &Path, name: &str, contents: &[u8]) {
        fs::write(root.join(name), contents).expect("the fixture file");
    }

    #[test]
    fn the_selection_is_two_sorted_groups() {
        let temp = TempDir::new().expect("a scratch directory");
        for name in ["b.zip", "c.tar.xz", "a.tar.xz", ".hidden.tar.xz", "notes"] {
            write(temp.path(), name, b"");
        }
        fs::create_dir(temp.path().join("d.zip")).expect("a directory entry");
        let files = archives(temp.path()).expect("a readable directory");
        // Every .tar.xz precedes every .zip; a global sort would put
        // b.zip second. The directory is selected - failing on it is
        // the hasher's business, as it was sha256sum's.
        assert_eq!(
            files,
            ["a.tar.xz", "c.tar.xz", "b.zip", "d.zip"].map(OsString::from)
        );
    }

    #[test]
    fn the_line_is_sha256sums_binary_format() {
        let temp = TempDir::new().expect("a scratch directory");
        write(temp.path(), "one.tar.xz", b"x");
        assert_eq!(run_in(temp.path()), ExitCode::SUCCESS);
        let side = fs::read(temp.path().join("one.tar.xz.sha256")).expect("the side file");
        assert_eq!(
            side,
            b"2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881 *one.tar.xz\n"
        );
        assert_eq!(
            fs::read(temp.path().join(SUM)).expect("the sum file"),
            side,
            "tee sent the same line both ways"
        );
    }

    #[test]
    fn an_empty_selection_refuses_before_truncating() {
        let temp = TempDir::new().expect("a scratch directory");
        write(temp.path(), SUM, b"stale");
        assert_eq!(run_in(temp.path()), ExitCode::FAILURE);
        assert_eq!(
            fs::read(temp.path().join(SUM)).expect("the sum file"),
            b"stale",
            "L4 runs before L5: an empty selection leaves sha256.sum alone"
        );
    }

    #[test]
    fn a_failing_hash_dies_mid_state() {
        let temp = TempDir::new().expect("a scratch directory");
        write(temp.path(), "a.tar.xz", b"first");
        fs::create_dir(temp.path().join("b.zip")).expect("the directory entry");
        assert_eq!(run_in(temp.path()), ExitCode::FAILURE);
        let sum = fs::read_to_string(temp.path().join(SUM)).expect("the sum file");
        assert!(
            sum.ends_with(" *a.tar.xz\n") && sum.lines().count() == 1,
            "the earlier file's line is already accumulated: {sum:?}"
        );
        assert_eq!(
            fs::read(temp.path().join("b.zip.sha256")).expect("the side file"),
            b"",
            "the redirections opened before the hash failed"
        );
    }

    #[test]
    fn an_unopenable_side_file_still_accumulates_the_line() {
        let temp = TempDir::new().expect("a scratch directory");
        write(temp.path(), "a.tar.xz", b"first");
        fs::create_dir(temp.path().join("a.tar.xz.sha256")).expect("the directory in the way");
        assert_eq!(run_in(temp.path()), ExitCode::FAILURE);
        let sum = fs::read_to_string(temp.path().join(SUM)).expect("the sum file");
        assert!(
            sum.ends_with(" *a.tar.xz\n") && sum.lines().count() == 1,
            "tee forwarded the line into sha256.sum before the pipeline died: {sum:?}"
        );
    }

    #[test]
    fn the_refusal_is_the_shells_sentence() {
        assert_eq!(NO_ARCHIVES, "no binary archives found");
    }
}
