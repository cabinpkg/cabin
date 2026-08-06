//! The spec-item coverage check, ported one-to-one from the `run:`
//! body of the "Check spec-item coverage" step of
//! `.github/workflows/proofs.yml` - and the differential that holds
//! the port to it.
//!
//! ```text
//! L1  missing=0
//! L2  for id in $(grep -oE '^\*\*(L[0-9]+|T[0-9]+|C[0-9]+)'
//! L2      docs/design/standard-compatibility/spec.md | tr -d '*' | sort -u); do
//! L3    if ! grep -rqE "(theorem|def) ${id}_"
//! L3        docs/proofs/standard-compatibility/StandardCompatibility/; then
//! L4      echo "spec item ${id} has no ${id}_* declaration in the Lean mechanization"
//! L5      missing=1
//! L6    fi
//! L7  done
//! L8  exit "$missing"
//! ```
//!
//! `tests/fixtures/spec-coverage.sh.orig` is that block, byte for
//! byte, as it stood on `main` at `7826776dd`, dedented 10 spaces,
//! `sha256`
//! `f232779d47d57287312ed5d782dfb2df003d6190b6776625e7de1f3195a8b712`.
//! Nothing is prepended and nothing is edited; the provenance lives
//! here instead. A spec-only change cannot break `lake build` (the
//! Lean sources never read the spec), so this check is what makes the
//! spec trigger meaningful - that rationale is the workflow header's,
//! and it is why the port lives on as a test rather than a tool.
//!
//! Inherited properties, preserved rather than fixed - each pinned by
//! running the original under `bash -e`:
//!
//! - **The accepted keywords are `theorem` and `def` only.** A Lean
//!   `lemma L1_x` counts as missing.
//! - **The trailing `_` is load-bearing:** `L1` does not match
//!   `L10_foo`.
//! - **The spec-side pattern is anchored at `^\*\*`,** so an id
//!   mentioned mid-line is not a spec item, and the id is the letter
//!   plus every following digit whatever comes after.
//! - **Ids are deduplicated and iterated in `sort -u` order** - byte
//!   order, so `C* < L* < T*`.
//! - **An empty id set exits 0** - including the set a missing or
//!   unreadable `spec.md` yields: `grep`'s failure dies inside the
//!   `$( )` word expansion, which under `set -e` is not fatal there,
//!   and the loop simply never runs. Fail-open, faithfully kept, with
//!   the one stderr diagnostic `grep` wrote on the way.
//! - **A missing Lean directory is conversely fail-closed:** `grep -rq`
//!   exits 2 with its own stderr line for every id, each id is
//!   reported missing on stdout, and the step exits 1.
//! - **The scan is `grep -r`'s, on the runner:** bytes, so a binary
//!   file containing the needle matches (GNU `grep` is the ported
//!   truth - a developer's `ugrep` or BSD `grep` treats binaries
//!   differently, which is why that scenario runs only against GNU);
//!   and symlinks met during the recursion are skipped, only a
//!   command-line operand being followed.
//!
//! Stated deviations and ceilings:
//!
//! - **This test also runs in the workspace gate.** The shell ran only
//!   when `proofs.yml`'s path filter matched; as a `#[test]` it now
//!   also runs with every workspace test sweep, which is deliberate: a
//!   pure file read over the checkout, and drift between the spec and
//!   the mechanization stops waiting for a proofs-path push.
//! - **Diagnostic wording on the unreadable-directory path is the
//!   port's own** (the shell's was `grep`'s), one line per id as the
//!   shell produced.
//!
//! # The differential
//!
//! Every scenario builds a synthetic corpus and asserts the port's
//! answer; where `bash` and `grep` are on `PATH` (skipped otherwise,
//! Windows included - the harness's own failures panic) it also runs
//! the vendored block with the corpus as its working directory and
//! compares stdout bytes, stderr emptiness, and the exit status
//! against the port's rendering of the same answer.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards:
//!
//! - Accepting `lemma` as a declaring keyword in the port failed
//!   exactly [`a_lemma_is_not_a_declaration`] on `the two sides
//!   disagree on what is missing`, with every other scenario green -
//!   the keyword ceiling is load-bearing and the catch is specific.
//! - Dropping the trailing `_` from the port's needles failed exactly
//!   [`the_trailing_underscore_separates_l1_from_l10`] the same way.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// L2's id shapes: the letter, then every following digit.
const PREFIXES: [char; 3] = ['L', 'T', 'C'];

/// L4's line for one id.
fn missing_line(id: &str) -> String {
    format!("spec item {id} has no {id}_* declaration in the Lean mechanization")
}

/// The spec path and Lean directory the shell hard-codes, relative to
/// one corpus root.
const SPEC: &str = "docs/design/standard-compatibility/spec.md";
const LEAN: &str = "docs/proofs/standard-compatibility/StandardCompatibility";

/// What one run of the check decides.
#[derive(Debug, PartialEq, Eq)]
struct Coverage {
    /// L4's lines, in id order.
    missing: Vec<String>,
    /// One diagnostic per id the Lean directory could not be scanned
    /// for; the shell's were `grep`'s stderr.
    diagnostics: Vec<String>,
}

impl Coverage {
    /// L8.
    fn exit(&self) -> i32 {
        i32::from(!self.missing.is_empty())
    }

    fn stdout(&self) -> String {
        self.missing
            .iter()
            .map(|id| missing_line(id) + "\n")
            .collect()
    }
}

/// L1..L8 against one corpus root.
fn coverage(root: &Path) -> Coverage {
    let mut missing = Vec::new();
    let mut diagnostics = Vec::new();
    let (ids, spec_diagnostic) = spec_ids(&root.join(SPEC));
    diagnostics.extend(spec_diagnostic);
    for id in ids {
        match declares(&root.join(LEAN), &id) {
            Ok(true) => {}
            Ok(false) => missing.push(id),
            Err(error) => {
                // grep -rq: its own stderr line, the id still missing.
                diagnostics.push(format!("{LEAN}: {error}"));
                missing.push(id);
            }
        }
    }
    Coverage {
        missing,
        diagnostics,
    }
}

/// L2: the anchored ids of one spec, deduplicated and byte-sorted. An
/// unreadable spec yields the empty set the shell's dead `$( )` did,
/// plus the one diagnostic `grep`'s stderr carried through it. The
/// decode is lossy rather than rejecting: the id grammar is pure
/// ASCII, so a stray invalid byte elsewhere in the file cannot change
/// what is extracted - where `grep`'s own binary-file heuristics on a
/// malformed spec vary by implementation and locale, a corner with no
/// single shell truth to reproduce.
fn spec_ids(spec: &Path) -> (Vec<String>, Option<String>) {
    let text = match fs::read(spec) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => return (Vec::new(), Some(format!("{SPEC}: {error}"))),
    };
    let mut ids: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("**")?;
            let mut characters = rest.chars();
            let prefix = characters.next().filter(|c| PREFIXES.contains(c))?;
            let digits: String = characters.take_while(char::is_ascii_digit).collect();
            (!digits.is_empty()).then(|| format!("{prefix}{digits}"))
        })
        .collect();
    ids.sort();
    ids.dedup();
    (ids, None)
}

/// L3: whether any file under `lean` (recursively) declares
/// `theorem <id>_` or `def <id>_`. As `grep -r` does, the scan reads
/// bytes (a binary file can match) and skips symlinks met during the
/// recursion (only a command-line operand is followed).
fn declares(lean: &Path, id: &str) -> std::io::Result<bool> {
    let needles = [format!("theorem {id}_"), format!("def {id}_")];
    let mut pending = vec![lean.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            if needles
                .iter()
                .any(|needle| contains_bytes(&bytes, needle.as_bytes()))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The regression the workflow runs: every numbered spec item has a
/// same-named declaration in the real mechanization.
#[test]
fn every_spec_item_is_mechanized() {
    // Runtime resolution, not `env!("CARGO_MANIFEST_DIR")`: worktrees
    // sharing a target directory reuse this binary, and a compile-time
    // path would inspect whichever checkout built it first. Cargo runs
    // a test with the package root as its working directory.
    let root = std::env::current_dir()
        .expect("the crate root the harness runs from")
        .join("../..");
    let answer = coverage(&root);
    // Diagnostics are context, not a verdict: L8 exits 1 only for
    // missing items, so a missing spec stays as fail-open here as it
    // was in the shell (an unscannable Lean directory still fails,
    // through the ids it reports missing).
    for line in &answer.diagnostics {
        eprintln!("{line}");
    }
    assert!(
        answer.missing.is_empty(),
        "the spec and the mechanization drifted:\n{}",
        answer.stdout()
    );
}

// --- the differential ---------------------------------------------

struct Corpus {
    dir: assert_fs::TempDir,
}

impl Corpus {
    fn new() -> Self {
        let dir = assert_fs::TempDir::new().expect("a scratch directory");
        fs::create_dir_all(dir.path().join(LEAN)).expect("the Lean directory");
        fs::create_dir_all(
            dir.path()
                .join(SPEC)
                .parent()
                .expect("the spec's directory"),
        )
        .expect("the spec's directory");
        Self { dir }
    }

    fn spec(&self, text: &str) -> &Self {
        fs::write(self.dir.path().join(SPEC), text).expect("the spec");
        self
    }

    fn lean(&self, name: &str, text: &str) -> &Self {
        let file = self.dir.path().join(LEAN).join(name);
        fs::create_dir_all(file.parent().expect("a parent")).expect("a nested Lean directory");
        fs::write(file, text).expect("a Lean source");
        self
    }
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn fixture() -> PathBuf {
    // Relative to the runtime working directory (the crate root), for
    // the same worktree-safety reason as the real-corpus test's root.
    Path::new("tests/fixtures/spec-coverage.sh.orig")
        .canonicalize()
        .expect("the vendored fixture beside the running test")
}

/// Asserts the port's whole answer, and holds the vendored shell block
/// to the same answer where the tools to run it exist.
fn check(case: &str, corpus: &Corpus, expected_missing: &[&str]) {
    let answer = coverage(corpus.dir.path());
    assert_eq!(
        answer.missing, expected_missing,
        "{case}: the two sides disagree on what is missing"
    );

    if cfg!(windows) || !have("bash") || !have("grep") {
        eprintln!("skipping {case}'s shell side: bash or grep is not on PATH");
        return;
    }
    let mut bash = Command::new("bash");
    bash.arg("-e").arg(fixture());
    bash.current_dir(corpus.dir.path());
    let produced = bash.output().expect("running the vendored block");

    assert_eq!(
        String::from_utf8_lossy(&produced.stdout),
        answer.stdout(),
        "{case}: stdout"
    );
    assert_eq!(
        produced.status.code(),
        Some(answer.exit()),
        "{case}: exit status"
    );
    assert_eq!(
        produced.stderr.is_empty(),
        answer.diagnostics.is_empty(),
        "{case}: one side diagnosed and the other did not: {}",
        String::from_utf8_lossy(&produced.stderr)
    );
}

#[test]
fn every_id_declared_is_silent() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n**T2** two\n**C3** three\n");
    corpus.lean(
        "Spec.lean",
        "theorem L1_holds : True := trivial\ndef T2_shape := 1\ntheorem C3_x : True := trivial\n",
    );
    check("every id declared", &corpus, &[]);
}

#[test]
fn a_missing_id_is_reported() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n**T2** two\n");
    corpus.lean("Spec.lean", "theorem L1_holds : True := trivial\n");
    check("one missing", &corpus, &["T2"]);
}

#[test]
fn missing_ids_come_out_in_sorted_order() {
    let corpus = Corpus::new();
    // Spec order T, L, C; sort -u order is C, L, T.
    corpus.spec("**T9** t\n**L5** l\n**C7** c\n");
    corpus.lean("Spec.lean", "-- nothing declared\n");
    check("three missing", &corpus, &["C7", "L5", "T9"]);
}

#[test]
fn a_mid_line_mention_is_not_a_spec_item() {
    let corpus = Corpus::new();
    corpus.spec("see **L1** for details\nprefix **T2** mid\n");
    corpus.lean("Spec.lean", "-- nothing\n");
    check("mid-line mentions", &corpus, &[]);
}

#[test]
fn duplicate_ids_are_deduplicated() {
    let corpus = Corpus::new();
    corpus.spec("**L1** first\n**L1** restated\n");
    corpus.lean("Spec.lean", "-- nothing\n");
    check("duplicates", &corpus, &["L1"]);
}

#[test]
fn the_trailing_underscore_separates_l1_from_l10() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n**L10** ten\n");
    corpus.lean("Spec.lean", "theorem L10_holds : True := trivial\n");
    check("the trailing underscore", &corpus, &["L1"]);
}

#[test]
fn a_lemma_is_not_a_declaration() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n");
    corpus.lean("Spec.lean", "lemma L1_holds : True := trivial\n");
    check("a lemma", &corpus, &["L1"]);
}

#[test]
fn a_declaration_in_a_nested_directory_is_found() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n");
    corpus.lean("Nested/Deep.lean", "theorem L1_holds : True := trivial\n");
    check("a nested declaration", &corpus, &[]);
}

#[test]
fn an_empty_spec_exits_zero() {
    let corpus = Corpus::new();
    corpus.spec("prose only\n");
    corpus.lean("Spec.lean", "-- nothing\n");
    check("an empty spec", &corpus, &[]);
}

#[test]
fn a_missing_spec_is_fail_open() {
    let corpus = Corpus::new();
    corpus.lean("Spec.lean", "-- nothing\n");
    // No spec written at all: grep's death inside the word expansion
    // is not fatal there, the loop never runs, the step exits 0 - with
    // the one diagnostic grep wrote on the way, which the port also
    // renders.
    check("a missing spec", &corpus, &[]);
    let answer = coverage(corpus.dir.path());
    assert_eq!(answer.diagnostics.len(), 1, "one line, as grep's stderr");
}

/// `grep -r` matches bytes: a binary file carrying the needle declares
/// the id. GNU `grep` (the runner's) is the ported truth; a
/// developer's `ugrep` or BSD `grep` treats binaries differently, so
/// the shell side runs only against GNU.
#[test]
fn a_binary_lean_file_still_declares() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n");
    fs::write(
        corpus.dir.path().join(LEAN).join("Blob.lean"),
        b"\x00\x01theorem L1_x : True := trivial\x00",
    )
    .expect("the binary Lean file");

    let answer = coverage(corpus.dir.path());
    assert_eq!(answer.missing, Vec::<String>::new(), "bytes match bytes");

    let gnu = Command::new("sh")
        .arg("-c")
        .arg("grep --version 2>/dev/null | head -1 | grep -q 'GNU grep'")
        .status()
        .is_ok_and(|status| status.success());
    if cfg!(windows) || !have("bash") || !gnu {
        eprintln!("skipping the shell side: GNU grep is not on PATH");
        return;
    }
    let mut bash = Command::new("bash");
    bash.arg("-e").arg(fixture());
    bash.current_dir(corpus.dir.path());
    let produced = bash.output().expect("running the vendored block");
    assert_eq!(
        produced.status.code(),
        Some(0),
        "GNU grep matched the bytes"
    );
    assert!(produced.stdout.is_empty());
}

/// A stray invalid byte elsewhere in the spec does not blank the id
/// set: the id grammar is ASCII and the decode is lossy. Port-only -
/// `grep`'s binary-file heuristics on a malformed spec differ between
/// GNU, BSD and ugrep, so there is no one shell truth to hold it to.
#[test]
fn an_invalid_byte_elsewhere_does_not_blank_the_spec() {
    let corpus = Corpus::new();
    fs::write(
        corpus.dir.path().join(SPEC),
        b"**L1** one\nprose with a stray \xe9 byte\n",
    )
    .expect("the malformed spec");
    corpus.lean("Spec.lean", "-- nothing\n");
    let answer = coverage(corpus.dir.path());
    assert_eq!(answer.missing, ["L1"], "the ASCII id grammar is unaffected");
    assert!(answer.diagnostics.is_empty());
}

/// `grep -r` skips a symlink met during the recursion, so a link to a
/// declaring file outside the tree does not satisfy the check.
#[cfg(unix)]
#[test]
fn a_symlink_met_in_the_recursion_is_skipped() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n");
    let outside = corpus.dir.path().join("outside.lean");
    fs::write(&outside, "theorem L1_x : True := trivial\n").expect("the outside declaration");
    std::os::unix::fs::symlink(&outside, corpus.dir.path().join(LEAN).join("Link.lean"))
        .expect("the symlink");
    check("a symlink in the recursion", &corpus, &["L1"]);
}

#[test]
fn other_prefixes_are_ignored() {
    let corpus = Corpus::new();
    corpus.spec("**D1** not a spec item\n**X5** neither\n**L2** is\n");
    corpus.lean("Spec.lean", "theorem L2_holds : True := trivial\n");
    check("other prefixes", &corpus, &[]);
}

#[test]
fn a_missing_lean_directory_is_fail_closed() {
    let corpus = Corpus::new();
    corpus.spec("**L1** one\n**T2** two\n");
    fs::remove_dir(corpus.dir.path().join(LEAN)).expect("removing the Lean directory");
    let answer = coverage(corpus.dir.path());
    assert_eq!(answer.missing, ["L1", "T2"]);
    assert_eq!(
        answer.diagnostics.len(),
        2,
        "one diagnostic per id, as grep wrote one stderr line per call"
    );
    if !cfg!(windows) && have("bash") && have("grep") {
        let mut bash = Command::new("bash");
        bash.arg("-e").arg(fixture());
        bash.current_dir(corpus.dir.path());
        let produced = bash.output().expect("running the vendored block");
        assert_eq!(String::from_utf8_lossy(&produced.stdout), answer.stdout());
        assert_eq!(produced.status.code(), Some(1));
        assert!(!produced.stderr.is_empty(), "grep diagnosed per id");
    }
}
