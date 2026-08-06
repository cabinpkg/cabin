//! The whole-run differential for the migrations deploy gate: the
//! shell it replaces and the port, run over one corpus of synthetic
//! `registry/` trees, compared on `$GITHUB_OUTPUT`, stdout, stderr and
//! exit status.
//!
//! `tests/fixtures/migrations-pending.sh.orig` is the original, byte
//! for byte: the `run:` block of the "Skip until changed D1 migrations
//! are applied by hand" step of `.github/workflows/registry.yml` as it
//! stood on `main` at `db5634288`, dedented 10 spaces, `sha256`
//! `32d22b88e9cc6cf027bbee1b801ec01512d81c7eaa1ad570368c2cbdf72b1b1f`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # Locale
//!
//! Both sides run under `LC_ALL=C`. The shell's glob expands in the
//! locale's collation, and the workflow's runner (`ubuntu-latest`,
//! `C.UTF-8`) collates in byte order - which `LC_ALL=C` reproduces on
//! any host, where a developer's `en_US.UTF-8` would sort
//! `0002_a.sql` before `0002_B.sql` and flip the collation scenario.
//! The port is locale-free byte order outright; the pin is on the
//! shell side.
//!
//! # What the corpus pins
//!
//! Measured under `bash -e` (GitHub's default `run:` shell) before the
//! port was written:
//!
//! - Every read failure is lenient. The stamp pipeline ends in
//!   `sha256sum | cut`, which succeed whatever `cat` did, so an empty
//!   or missing `migrations/` diagnoses to stderr and stamps as the
//!   digest of empty input; a directory matching the glob diagnoses
//!   and contributes nothing; a missing `migrations-applied` compares
//!   as empty inside the `if` condition, where `set -e` is suppressed.
//!   Exit 0 in every one of those shapes.
//! - The glob matches bytes, not UTF-8: an invalid multibyte basename
//!   ending in `.sql` is inside the stamp. (A *partial* mid-file read
//!   failure - `cat` streams, so a delivered prefix stays in the
//!   digest - has no portable file shape to build here; the port pins
//!   it with a unit test against an injected reader instead.)
//! - The comparison is bytes: trailing newlines are stripped by the
//!   substitution (none, one and three all match), a CRLF ending is
//!   not (the `\r` survives).
//! - `pending=true` is *appended*: what the runner already wrote to
//!   `$GITHUB_OUTPUT` stays.
//! - The one failure the step can have: `$GITHUB_OUTPUT` unset in the
//!   pending case (`>> ""` exits 1); the current case never reaches
//!   the redirect.
//!
//! stderr is compared byte for byte only where the guard is the sole
//! writer - every scenario the tools stay quiet in. Where the shell's
//! `cat` wrote its own diagnostic the port writes its own wording, so
//! the assertion narrows to "both sides said something". The exit
//! status is compared exactly everywhere.
//!
//! The suite is Unix-only outright: the original is a bash script, and
//! a Windows host's `bash` lookalike would pass a presence check while
//! meaning something else. Every test skips rather than fails when a
//! tool it needs is missing.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards:
//!
//! - Changing the port's `pending=true` to `pending=TRUE` failed every
//!   scenario that expects a positive answer, so the file's bytes
//!   really are read rather than assumed.
//! - Removing the port's dotfile filter failed exactly
//!   [`a_dotfile_is_outside_the_stamp`], with the other tests still
//!   passing - the glob emulation is load-bearing and the catch is
//!   specific rather than collateral.
#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_fs::TempDir;

mod common;
use common::ready;

/// L3's line, which is the whole of what a positive answer writes.
const PENDING: &[u8] = b"pending=true\n";

/// The digest of empty input: what an empty selection stamps as.
const EMPTY_STAMP: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The tools every scenario drives, on top of the port itself.
const TOOLS: [&str; 4] = ["bash", "sha256sum", "cut", "cat"];

/// How far stderr can be compared.
enum Diagnostics {
    /// The tools stayed quiet, so the guard was the only writer:
    /// compare byte for byte.
    Quiet,
    /// `cat` (or the port's own reader) diagnosed. Assert both sides
    /// said something; the wording is each side's own.
    Tool,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// The `$GITHUB_OUTPUT` file's bytes, or `None` when the scenario
    /// ran without one.
    output: Option<Vec<u8>>,
}

/// One scenario's `registry/` tree, populated by the test.
struct Corpus {
    dir: TempDir,
}

impl Corpus {
    fn new() -> Self {
        let dir = TempDir::new().expect("a scratch directory");
        fs::create_dir_all(dir.path().join("registry/migrations"))
            .expect("the migrations directory");
        Self { dir }
    }

    /// A migration (or any other) file under `registry/`.
    fn write(&self, path: &str, contents: &[u8]) {
        let file = self.dir.path().join("registry").join(path);
        fs::create_dir_all(file.parent().expect("a path with a parent"))
            .expect("the file's directory");
        fs::write(file, contents).expect("the corpus file");
    }

    /// Records `stamp` (plus a trailing newline, as the operator's
    /// `cargo registry-migrate` writes it) as the applied stamp.
    fn applied(&self, stamp: &str) {
        self.write("migrations-applied", format!("{stamp}\n").as_bytes());
    }

    /// The digest the shell's pipeline computes over these exact
    /// bytes, via the same `sha256sum | cut` it runs - so the corpus
    /// never trusts the port's own hashing.
    fn stamp_of(contents: &[u8]) -> String {
        let mut shell = Command::new("sh");
        shell.arg("-c").arg("sha256sum | cut -d' ' -f1");
        shell.stdin(std::process::Stdio::piped());
        shell.stdout(std::process::Stdio::piped());
        let mut child = shell.spawn().expect("spawning sha256sum");
        child
            .stdin
            .take()
            .expect("a piped stdin")
            .write_all(contents)
            .expect("feeding sha256sum");
        let done = child.wait_with_output().expect("collecting sha256sum");
        assert!(done.status.success(), "the harness's sha256sum failed");
        String::from_utf8(done.stdout)
            .expect("a hex digest")
            .trim_end()
            .to_owned()
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/migrations-pending.sh.orig")
}

/// Runs one side and collects everything it produced. `seed` is what
/// the runner had already written to `$GITHUB_OUTPUT`; `None` runs
/// with the variable unset entirely.
fn once(mut command: Command, dir: &Path, seed: Option<&[u8]>) -> Outcome {
    command.current_dir(dir);
    command.env("LC_ALL", "C");
    let output_file = dir.join("github-output");
    match seed {
        Some(seed) => {
            fs::write(&output_file, seed).expect("the runner's $GITHUB_OUTPUT");
            command.env("GITHUB_OUTPUT", &output_file);
        }
        None => {
            command.env_remove("GITHUB_OUTPUT");
        }
    }
    let produced = command.output().expect("running one side of the scenario");
    Outcome {
        stdout: produced.stdout,
        stderr: produced.stderr,
        status: produced.status.code(),
        output: seed.map(|_| fs::read(&output_file).expect("the $GITHUB_OUTPUT file")),
    }
}

/// Runs both sides of one scenario over two byte-identical copies of
/// the corpus - each side gets its own working directory so neither
/// can leak state into the other's run.
fn both(corpus: &Corpus, seed: Option<&[u8]>) -> (Outcome, Outcome) {
    let shell_dir = corpus.dir.path().join("shell");
    let port_dir = corpus.dir.path().join("port");
    for side in [&shell_dir, &port_dir] {
        copy_tree(&corpus.dir.path().join("registry"), &side.join("registry"));
    }

    // GitHub's default `run:` shell on Linux is `bash -e {0}`: `-e`
    // on, `-u` and `-o pipefail` off. The pipeline's swallowed `cat`
    // status is the whole point of several scenarios.
    let mut bash = Command::new("bash");
    bash.arg("-e").arg(fixture());
    let shell = once(bash, &shell_dir, seed);

    let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-workflow-guard"));
    ported.arg("migrations-pending");
    let port = once(ported, &port_dir, seed);

    (shell, port)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("the copy's root");
    for entry in fs::read_dir(from).expect("the corpus tree") {
        let entry = entry.expect("a corpus entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a corpus file copy");
        }
    }
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, diagnostics: &Diagnostics) {
    assert!(
        shell.output == port.output,
        "{case}: $GITHUB_OUTPUT\nshell: {}\nport:  {}",
        rendered(shell.output.as_deref()),
        rendered(port.output.as_deref())
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

fn rendered(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "<no $GITHUB_OUTPUT>".to_owned(),
        |bytes| bytes.escape_ascii().to_string(),
    )
}

/// Asserts one side's answer in full: nothing on stdout, and
/// `$GITHUB_OUTPUT` carrying exactly `expected`.
fn wrote(case: &str, outcome: &Outcome, expected: &[u8]) {
    assert!(
        outcome.stdout.is_empty(),
        "{case}: something reached stdout: {}",
        outcome.stdout.escape_ascii()
    );
    assert!(
        outcome.output.as_deref() == Some(expected),
        "{case}: $GITHUB_OUTPUT is {}, expected {}",
        rendered(outcome.output.as_deref()),
        expected.escape_ascii()
    );
}

/// The stamp matches: nothing is written and the step is silent.
#[test]
fn a_current_stamp_writes_nothing() {
    if !ready("a_current_stamp_writes_nothing", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    corpus.applied(&Corpus::stamp_of(b"create table a;\n"));
    let (shell, port) = both(&corpus, Some(b""));
    diff("current", &shell, &port, &Diagnostics::Quiet);
    wrote("current", &shell, b"");
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a current stamp is silent");
}

/// A changed migration answers pending.
#[test]
fn a_changed_migration_answers_pending() {
    if !ready("a_changed_migration_answers_pending", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    corpus.applied(&Corpus::stamp_of(b"create table b;\n"));
    let (shell, port) = both(&corpus, Some(b""));
    diff("changed", &shell, &port, &Diagnostics::Quiet);
    wrote("changed", &shell, PENDING);
    assert_eq!(shell.status, Some(0), "pending is a skip, not a failure");
}

/// The stamp is the digest of the concatenation in glob order: the
/// same two files match in order and mismatch reversed.
#[test]
fn the_stamp_concatenates_in_glob_order() {
    if !ready("the_stamp_concatenates_in_glob_order", &TOOLS) {
        return;
    }
    for (case, concatenation, expected) in [
        ("in order", b"onetwo".as_slice(), b"".as_slice()),
        ("reversed", b"twoone".as_slice(), PENDING),
    ] {
        let corpus = Corpus::new();
        corpus.write("migrations/0001_a.sql", b"one");
        corpus.write("migrations/0002_b.sql", b"two");
        corpus.applied(&Corpus::stamp_of(concatenation));
        let (shell, port) = both(&corpus, Some(b""));
        diff(case, &shell, &port, &Diagnostics::Quiet);
        wrote(case, &shell, expected);
    }
}

/// `B` (0x42) collates before `a` (0x61) on the runner: a stamp over
/// the byte order matches under `LC_ALL=C` on both sides.
#[test]
fn collation_is_byte_order() {
    if !ready("collation_is_byte_order", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0002_B.sql", b"upper");
    corpus.write("migrations/0002_a.sql", b"lower");
    corpus.applied(&Corpus::stamp_of(b"upperlower"));
    let (shell, port) = both(&corpus, Some(b""));
    diff("collation", &shell, &port, &Diagnostics::Quiet);
    wrote("collation", &shell, b"");
}

/// An empty `migrations/` leaves the glob unexpanded: `cat` diagnoses,
/// the pipeline hashes empty input, and the verdict follows whatever
/// the applied stamp says about *that* digest.
#[test]
fn an_empty_migrations_directory_stamps_as_empty_input() {
    if !ready(
        "an_empty_migrations_directory_stamps_as_empty_input",
        &TOOLS,
    ) {
        return;
    }
    for (case, applied, expected) in [
        ("empty, stamped as empty", EMPTY_STAMP, b"".as_slice()),
        ("empty, stamped otherwise", "deadbeef", PENDING),
    ] {
        let corpus = Corpus::new();
        corpus.applied(applied);
        let (shell, port) = both(&corpus, Some(b""));
        diff(case, &shell, &port, &Diagnostics::Tool);
        wrote(case, &shell, expected);
        wrote(case, &port, expected);
        assert_eq!(shell.status, Some(0), "{case}: the pipeline swallowed cat");
    }
}

/// A missing applied stamp compares as the empty substitution: always
/// pending, never an error.
#[test]
fn a_missing_applied_stamp_answers_pending() {
    if !ready("a_missing_applied_stamp_answers_pending", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    let (shell, port) = both(&corpus, Some(b""));
    diff("missing applied", &shell, &port, &Diagnostics::Tool);
    wrote("missing applied", &shell, PENDING);
    wrote("missing applied", &port, PENDING);
    assert_eq!(shell.status, Some(0));
}

/// The applied stamp is read through `$(...)`: no trailing newline and
/// three trailing newlines both match, a CRLF ending does not.
#[test]
fn the_applied_stamp_reads_like_a_command_substitution() {
    if !ready(
        "the_applied_stamp_reads_like_a_command_substitution",
        &TOOLS,
    ) {
        return;
    }
    let sql = b"create table a;\n";
    for (case, suffix, expected) in [
        ("no trailing newline", "", b"".as_slice()),
        ("three trailing newlines", "\n\n\n", b"".as_slice()),
        ("a CRLF ending", "\r\n", PENDING),
    ] {
        let corpus = Corpus::new();
        corpus.write("migrations/0001_init.sql", sql);
        let stamp = Corpus::stamp_of(sql);
        corpus.write("migrations-applied", format!("{stamp}{suffix}").as_bytes());
        let (shell, port) = both(&corpus, Some(b""));
        diff(case, &shell, &port, &Diagnostics::Quiet);
        wrote(case, &shell, expected);
    }
}

/// The glob skips dotfiles: an operator's `.draft.sql` scratch file is
/// outside the stamp on both sides.
#[test]
fn a_dotfile_is_outside_the_stamp() {
    if !ready("a_dotfile_is_outside_the_stamp", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    corpus.write("migrations/.draft.sql", b"drop table a;\n");
    corpus.applied(&Corpus::stamp_of(b"create table a;\n"));
    let (shell, port) = both(&corpus, Some(b""));
    diff("dotfile", &shell, &port, &Diagnostics::Quiet);
    wrote("dotfile", &shell, b"");
}

/// The glob matches bytes, not UTF-8: bash byte-matches an invalid
/// multibyte basename against `*.sql`, so a Latin-1 filename is inside
/// the stamp on both sides - a stamp over only the ASCII file answers
/// pending, which is what says the non-UTF-8 file counted.
#[test]
fn a_non_utf8_name_is_inside_the_stamp() {
    if !ready("a_non_utf8_name_is_inside_the_stamp", &TOOLS) {
        return;
    }
    for (case, concatenation, expected) in [
        ("stamped with it", b"asciilatin1".as_slice(), b"".as_slice()),
        ("stamped without it", b"ascii".as_slice(), PENDING),
    ] {
        let corpus = Corpus::new();
        corpus.write("migrations/0001_a.sql", b"ascii");
        let name = std::ffi::OsString::from_vec(b"0002_legacy\xe9.sql".to_vec());
        if fs::write(
            corpus.dir.path().join("registry/migrations").join(&name),
            b"latin1",
        )
        .is_err()
        {
            // APFS refuses non-UTF-8 names outright; the runner's ext4
            // (where the gate actually runs) does not.
            eprintln!("skipping {case}: this filesystem refuses non-UTF-8 names");
            continue;
        }
        corpus.applied(&Corpus::stamp_of(concatenation));
        let (shell, port) = both(&corpus, Some(b""));
        diff(case, &shell, &port, &Diagnostics::Quiet);
        wrote(case, &shell, expected);
    }
}

/// A directory matching the glob gets `cat`'s diagnostic and
/// contributes nothing; the run still exits 0 and the remaining files
/// decide the verdict.
#[test]
fn a_directory_matching_the_glob_contributes_nothing() {
    if !ready("a_directory_matching_the_glob_contributes_nothing", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    fs::create_dir(corpus.dir.path().join("registry/migrations/0002_dir.sql"))
        .expect("the directory entry");
    corpus.applied(&Corpus::stamp_of(b"create table a;\n"));
    let (shell, port) = both(&corpus, Some(b""));
    diff("a directory entry", &shell, &port, &Diagnostics::Tool);
    wrote("a directory entry", &shell, b"");
    wrote("a directory entry", &port, b"");
    assert_eq!(shell.status, Some(0));
}

/// `$GITHUB_OUTPUT` unset: the current case never reaches the redirect
/// and succeeds; the pending case fails the step on both sides.
#[test]
fn an_unset_github_output_fails_only_the_pending_case() {
    if !ready("an_unset_github_output_fails_only_the_pending_case", &TOOLS) {
        return;
    }
    let current = Corpus::new();
    current.write("migrations/0001_init.sql", b"create table a;\n");
    current.applied(&Corpus::stamp_of(b"create table a;\n"));
    let (shell, port) = both(&current, None);
    diff("current, no output", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.status, Some(0));

    let pending = Corpus::new();
    pending.write("migrations/0001_init.sql", b"create table a;\n");
    pending.applied("deadbeef");
    let (shell, port) = both(&pending, None);
    diff("pending, no output", &shell, &port, &Diagnostics::Tool);
    assert_eq!(shell.status, Some(1), "the shell's `>> \"\"` exits 1");
}

/// `pending=true` is appended: what the runner already wrote stays.
#[test]
fn an_existing_output_file_is_appended_to() {
    if !ready("an_existing_output_file_is_appended_to", &TOOLS) {
        return;
    }
    let corpus = Corpus::new();
    corpus.write("migrations/0001_init.sql", b"create table a;\n");
    corpus.applied("deadbeef");
    let seed = b"written-earlier=kept\n";
    let (shell, port) = both(&corpus, Some(seed));
    diff(
        "an already-written output",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );

    let mut expected = seed.to_vec();
    expected.extend_from_slice(PENDING);
    wrote("an already-written output", &shell, &expected);
    wrote("an already-written output", &port, &expected);
}
