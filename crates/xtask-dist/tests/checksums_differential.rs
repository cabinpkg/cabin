//! The whole-step differential for release checksums: the shell it
//! replaces and the port, run over one corpus of throwaway artifact
//! directories, compared on stdout, stderr, exit status and every file
//! the step leaves behind.
//!
//! `tests/fixtures/checksums.sh.orig` is the original, byte for byte:
//! the `run:` block of the "Generate checksums" step of
//! `.github/workflows/dist.yml` as it stood on `main` at `3c419655d`,
//! dedented 10 spaces, `sha256`
//! `e7a5c666ac70482fbca5680a159b098a46e53204dc322a6dda05a0559acf6921`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! The step declares `shell: bash`, for which GitHub runs
//! `bash --noprofile --norc -eo pipefail {0}`; that is reproduced
//! verbatim, and the block's own `set -euo pipefail` adds `-u`.
//!
//! # What is compared, and where the comparison stops
//!
//! stdout is compared as bytes everywhere: the final `cat sha256.sum`
//! is the step's log contract. So is every file the step writes - the
//! suite lists each side's directory afterward and compares the full
//! name-to-bytes map, which is what catches a wrong `.sha256` sibling,
//! a missing accumulation line, or an extra file, independently of the
//! log. The exit status is compared exactly everywhere: the shell's
//! failures here are `exit 1` (the refusal) and `sha256sum`'s own 1
//! under `pipefail` (an unreadable entry), which the port's documented
//! collapse also renders as 1.
//!
//! stderr is compared byte for byte where the step's own words are all
//! there is - the refusal is `echo`'d by the block, so a port that
//! rendered it through an error type would prefix it and diverge. On
//! the unreadable-entry path the wording is `sha256sum`'s on one side
//! and the port's on the other, so the assertion narrows to "both
//! sides said something".
//!
//! The suite is Unix-only outright; every test skips rather than fails
//! when a tool it needs is missing, and the harness's own failures
//! panic.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards. Both perturb the port's side alone:
//!
//! - Dropping the `*` binary marker from the port's line format failed
//!   every content scenario on the file map (`sha256.sum` differing in
//!   every line) and on stdout, so the byte contract really is read
//!   from both sides rather than assumed.
//! - Sorting the port's selection globally instead of per glob group
//!   failed [`the_order_is_two_glob_groups`] - whose corpus is built so
//!   the two orders differ - and [`every_archive_is_checksummed`],
//!   whose release-shaped names happen to discriminate too
//!   (`...-pc-windows-msvc.zip` sorts before
//!   `...-unknown-linux-gnu.tar.xz` globally and after it grouped),
//!   with the order-insensitive scenarios still green: the grouping is
//!   load-bearing and the catch is order-specific.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_fs::TempDir;

/// What the refusal writes, `echo`'s newline included.
const NO_ARCHIVES: &str = "no binary archives found\n";

/// How far stderr can be compared.
enum Diagnostics {
    /// The step's own words are all there is: compare byte for byte.
    Quiet,
    /// `sha256sum` spoke on the shell side and the port for itself:
    /// assert both sides said something.
    Tool,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// Every regular file in the directory afterward, name to bytes.
    files: BTreeMap<String, Vec<u8>>,
}

/// One scenario's artifact directory, staged once and copied per side.
struct Corpus {
    dir: TempDir,
}

impl Corpus {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("a scratch directory"),
        }
    }

    fn write(&self, name: &str, contents: &[u8]) {
        fs::write(self.dir.path().join("stage").join(name), contents).expect("the corpus file");
    }

    fn stage(&self) -> &Self {
        fs::create_dir_all(self.dir.path().join("stage")).expect("the staging directory");
        self
    }

    /// Runs both sides over two byte-identical copies of the staged
    /// directory.
    fn both(&self) -> (Outcome, Outcome) {
        let stage = self.dir.path().join("stage");
        let shell_dir = self.dir.path().join("shell");
        let port_dir = self.dir.path().join("port");
        for side in [&shell_dir, &port_dir] {
            copy_tree(&stage, side);
        }

        // GitHub's invocation for an explicit `shell: bash`.
        let mut bash = Command::new("bash");
        bash.args(["--noprofile", "--norc", "-eo", "pipefail"]);
        bash.arg(fixture());
        let shell = run(bash, &shell_dir);

        let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-dist"));
        ported.arg("checksums");
        let port = run(ported, &port_dir);

        (shell, port)
    }
}

fn run(mut command: Command, dir: &Path) -> Outcome {
    command.current_dir(dir);
    let produced: Output = command.output().expect("running one side of the scenario");
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(dir).expect("the directory afterward") {
        let entry = entry.expect("a directory entry");
        if entry.file_type().expect("a file type").is_file() {
            files.insert(
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("a produced file"),
            );
        }
    }
    Outcome {
        stdout: produced.stdout,
        stderr: produced.stderr,
        status: produced.status.code(),
        files,
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("the copy's root");
    for entry in fs::read_dir(from).expect("the staged tree") {
        let entry = entry.expect("a staged entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a staged file copy");
        }
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/checksums.sh.orig")
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn ready(test: &str) -> bool {
    for tool in ["bash", "sha256sum", "tee"] {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, diagnostics: &Diagnostics) {
    assert!(
        shell.files == port.files,
        "{case}: the two sides left different files\nshell: {:#?}\nport:  {:#?}",
        rendered(&shell.files),
        rendered(&port.files)
    );
    assert!(
        shell.stdout == port.stdout,
        "{case}: stdout\nshell: {}\nport:  {}",
        shell.stdout.escape_ascii(),
        port.stdout.escape_ascii()
    );
    assert_eq!(shell.status, port.status, "{case}: exit status");
    match *diagnostics {
        Diagnostics::Quiet => {
            assert!(
                shell.stderr == port.stderr,
                "{case}: stderr\nshell: {}\nport:  {}",
                shell.stderr.escape_ascii(),
                port.stderr.escape_ascii()
            );
        }
        Diagnostics::Tool => {
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert!(
                    !outcome.stderr.is_empty(),
                    "{case}: the {side} said nothing about a read failure"
                );
            }
        }
    }
}

fn rendered(files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<&str, String> {
    files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.escape_ascii().to_string()))
        .collect()
}

/// The happy path: every archive checksummed, the summary printed, and
/// each `.sha256` sibling carrying its own line.
#[test]
fn every_archive_is_checksummed() {
    if !ready("every_archive_is_checksummed") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("cabin-0.14.0-x86_64-unknown-linux-gnu.tar.xz", b"linux");
    corpus.write("cabin-0.14.0-x86_64-pc-windows-msvc.zip", b"windows");

    let (shell, port) = corpus.both();
    diff("every archive", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a clean run is silent");
    assert_eq!(
        shell.files.keys().collect::<Vec<_>>(),
        [
            "cabin-0.14.0-x86_64-pc-windows-msvc.zip",
            "cabin-0.14.0-x86_64-pc-windows-msvc.zip.sha256",
            "cabin-0.14.0-x86_64-unknown-linux-gnu.tar.xz",
            "cabin-0.14.0-x86_64-unknown-linux-gnu.tar.xz.sha256",
            "sha256.sum",
        ]
    );
    let sum = &shell.files["sha256.sum"];
    assert!(
        sum.ends_with(b" *cabin-0.14.0-x86_64-pc-windows-msvc.zip\n"),
        "the zip group comes last: {}",
        sum.escape_ascii()
    );
    assert_eq!(shell.stdout, *sum, "the step's log is `cat sha256.sum`");
}

/// The selection is two glob groups: every `.tar.xz` (sorted) precedes
/// every `.zip` (sorted). The names are chosen so a single global sort
/// would interleave them.
#[test]
fn the_order_is_two_glob_groups() {
    if !ready("the_order_is_two_glob_groups") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("a.tar.xz", b"one");
    corpus.write("b.zip", b"two");
    corpus.write("c.tar.xz", b"three");

    let (shell, port) = corpus.both();
    diff("glob groups", &shell, &port, &Diagnostics::Quiet);
    let order: Vec<&str> = std::str::from_utf8(&shell.files["sha256.sum"])
        .expect("hex and ASCII names")
        .lines()
        .map(|line| line.split_once('*').expect("the binary marker").1)
        .collect();
    assert_eq!(order, ["a.tar.xz", "c.tar.xz", "b.zip"]);
}

/// A dotfile matching a pattern is outside the selection on both
/// sides, and a name that matches neither pattern is ignored.
#[test]
fn a_dotfile_is_outside_the_selection() {
    if !ready("a_dotfile_is_outside_the_selection") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("cabin-0.14.0-aarch64-apple-darwin.tar.xz", b"mac");
    corpus.write(".hidden.tar.xz", b"scratch");
    corpus.write("notes.txt", b"prose");

    let (shell, port) = corpus.both();
    diff("a dotfile", &shell, &port, &Diagnostics::Quiet);
    assert!(
        !shell.files.contains_key(".hidden.tar.xz.sha256"),
        "the glob skipped the dotfile"
    );
    assert_eq!(shell.status, Some(0));
}

/// An empty selection refuses with the block's own sentence, before
/// `sha256.sum` is even truncated.
#[test]
fn an_empty_selection_refuses_before_truncating() {
    if !ready("an_empty_selection_refuses_before_truncating") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("sha256.sum", b"stale");
    corpus.write("notes.txt", b"prose");

    let (shell, port) = corpus.both();
    diff("an empty selection", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.status, Some(1));
    assert_eq!(shell.stderr, NO_ARCHIVES.as_bytes());
    assert_eq!(
        shell.files["sha256.sum"],
        b"stale".to_vec(),
        "the refusal came before the truncation"
    );
}

/// A side file `tee` cannot open - a directory sitting where
/// `<archive>.sha256` goes - does not stop the hash: the line still
/// reaches `sha256.sum`, and only then does the pipeline die.
#[test]
fn an_unopenable_side_file_still_accumulates_the_line() {
    if !ready("an_unopenable_side_file_still_accumulates_the_line") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("a.tar.xz", b"first");
    fs::create_dir(corpus.dir.path().join("stage/a.tar.xz.sha256"))
        .expect("the directory in the way");

    let (shell, port) = corpus.both();
    diff("an unopenable side file", &shell, &port, &Diagnostics::Tool);
    assert_eq!(shell.status, Some(1), "pipefail carried tee's 1");
    assert!(shell.stdout.is_empty(), "the final cat never ran");
    assert!(
        shell.files["sha256.sum"].ends_with(b" *a.tar.xz\n"),
        "tee forwarded the line before the pipeline died"
    );
}

/// An entry the hasher cannot read - a directory matching the glob -
/// dies mid-state under `pipefail`: the earlier file's line is already
/// accumulated, the failing entry's `.sha256` exists empty, and
/// nothing reaches stdout.
#[test]
fn an_unreadable_entry_dies_mid_state() {
    if !ready("an_unreadable_entry_dies_mid_state") {
        return;
    }
    let corpus = Corpus::new();
    corpus.stage();
    corpus.write("a.tar.xz", b"first");
    fs::create_dir(corpus.dir.path().join("stage/b.zip")).expect("the directory entry");

    let (shell, port) = corpus.both();
    diff("an unreadable entry", &shell, &port, &Diagnostics::Tool);
    assert_eq!(shell.status, Some(1), "pipefail carried sha256sum's 1");
    assert!(shell.stdout.is_empty(), "the final cat never ran");
    assert_eq!(
        shell.files["b.zip.sha256"],
        b"".to_vec(),
        "the redirections opened before the hash failed"
    );
    assert!(
        shell.files["sha256.sum"].ends_with(b" *a.tar.xz\n"),
        "the earlier line was already accumulated"
    );
}
