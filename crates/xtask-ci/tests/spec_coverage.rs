//! The spec-item coverage check that `proofs.yml`'s "Check spec-item
//! coverage" step runs: every `**L<n>**`/`**T<n>**`/`**C<n>**` item in
//! the standard-compatibility spec needs a same-named `theorem`/`def`
//! declaration in the Lean mechanization. A spec-only change cannot
//! break `lake build` (the Lean sources never read the spec), so this
//! check is what makes spec drift visible.
//!
//! It also runs in every workspace test sweep, not only on
//! proofs-path pushes - deliberate: a pure file read over the
//! checkout, so drift stops waiting for a proofs-path push. A missing
//! or unreadable spec is fail-open (empty id set, one diagnostic); an
//! unscannable Lean directory is fail-closed (every id reports
//! missing).

use std::fs;
use std::path::Path;

/// The id shapes: the letter, then every following digit.
const PREFIXES: [char; 3] = ['L', 'T', 'C'];

fn missing_line(id: &str) -> String {
    format!("spec item {id} has no {id}_* declaration in the Lean mechanization")
}

/// The spec path and Lean directory, relative to one corpus root.
const SPEC: &str = "docs/design/standard-compatibility/spec.md";
const LEAN: &str = "docs/proofs/standard-compatibility/StandardCompatibility";

/// What one run of the check decides.
#[derive(Debug, PartialEq, Eq)]
struct Coverage {
    /// The missing-item lines, in id order.
    missing: Vec<String>,
    /// One diagnostic per path that could not be scanned.
    diagnostics: Vec<String>,
}

impl Coverage {
    fn stdout(&self) -> String {
        self.missing
            .iter()
            .map(|id| missing_line(id) + "\n")
            .collect()
    }
}

/// The whole check against one corpus root.
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
                // Unscannable directory: diagnosed, the id still missing.
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

/// The anchored ids of one spec, deduplicated and byte-sorted. An
/// unreadable spec yields the empty set plus one diagnostic. The
/// decode is lossy rather than rejecting: the id grammar is pure
/// ASCII, so a stray invalid byte elsewhere in the file cannot change
/// what is extracted.
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

/// Whether any file under `lean` (recursively) declares
/// `theorem <id>_` or `def <id>_`. The trailing `_` keeps `L1` from
/// matching `L10_foo`, and `lemma` deliberately does not count. The
/// scan reads bytes and skips symlinks met during the recursion.
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
    // Diagnostics are context, not a verdict: only missing items fail,
    // so a missing spec stays fail-open (an unscannable Lean directory
    // still fails, through the ids it reports missing).
    for line in &answer.diagnostics {
        eprintln!("{line}");
    }
    assert!(
        answer.missing.is_empty(),
        "the spec and the mechanization drifted:\n{}",
        answer.stdout()
    );
}

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

/// One corpus exercises the whole extraction grammar: only the three
/// prefixes count, only at line starts, ids deduplicate and sort in
/// byte order, the trailing underscore separates `L1` from `L10`, a
/// `lemma` is not a declaration, and a nested declaration is found.
#[test]
fn the_port_reports_only_undeclared_ids() {
    let corpus = Corpus::new();
    corpus.spec(
        "**T9** t\n\
         **L5** l\n\
         **C7** c\n\
         **L1** one\n\
         **L1** restated\n\
         **L10** ten\n\
         **D1** not a spec item\n\
         see **T2** mid-line\n",
    );
    corpus.lean("Nested/Deep.lean", "theorem L5_holds : True := trivial\n");
    corpus.lean(
        "Spec.lean",
        "lemma L1_holds : True := trivial\ntheorem L10_holds : True := trivial\n",
    );
    let answer = coverage(corpus.dir.path());
    assert_eq!(answer.missing, ["C7", "L1", "T9"]);
    assert!(answer.diagnostics.is_empty());
}

#[test]
fn a_missing_spec_is_fail_open() {
    let corpus = Corpus::new();
    corpus.lean("Spec.lean", "-- nothing\n");
    let answer = coverage(corpus.dir.path());
    assert!(answer.missing.is_empty());
    assert_eq!(answer.diagnostics.len(), 1, "the failed read is diagnosed");
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
        "one diagnostic per id the directory could not be scanned for"
    );
}
