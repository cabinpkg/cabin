//! The whole-run differential for the deploy freshness guard: the shell
//! it replaces and the port, run over one corpus of throwaway git
//! repositories, compared on `$GITHUB_OUTPUT`, stdout, stderr and exit
//! status.
//!
//! `tests/fixtures/superseded.sh.orig` is the original, byte for byte:
//! the `run:` block of the "Skip when superseded by a newer registry
//! commit" step of `.github/workflows/registry.yml` as it stood on
//! `main` at `d454d37a1`, dedented 10 spaces, `sha256`
//! `1e138be99c8a9668c082570cb1207c193657c2381328b75cdf0fddbc368436df`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # The seam is real git, not a mock
//!
//! Every scenario builds a bare repository as `origin`, commits a
//! history into it through a publisher clone, and then clones it once
//! per side. `git` is the only external tool the original drives and it
//! is the real one on both sides, so nothing here is stubbed: what is
//! compared is two runs against two byte-identical checkouts of the same
//! `origin`. The run context is handed over the way Actions hands it
//! over - `GITHUB_SHA` naming a commit, `GITHUB_OUTPUT` naming a file
//! the runner has already created - and the checkout shape mirrors
//! `actions/checkout` with `fetch-depth: 0`: a full clone, detached at
//! `GITHUB_SHA`.
//!
//! The harness never reads or writes the machine's git configuration:
//! `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` point at `/dev/null`, and
//! the identity and the dates come from the environment, which also
//! makes every commit hash in the corpus reproducible - that is what
//! lets the unpushed-commit scenario hand both sides the same
//! `GITHUB_SHA`.
//!
//! # The finding this suite pins: L2 fails OPEN
//!
//! `git fetch` (L1) is a standalone command, so `bash -e` - GitHub's
//! default `run:` shell, `-e` on and `-u`/`-o pipefail` off - kills the
//! step when it fails. `git rev-list` (L2) is not: it runs inside a
//! command substitution, inside `[ -n ... ]`, inside an `if` condition,
//! where `set -e` is suppressed and the enclosing simple command
//! discards the substitution's status. A `rev-list` that errors
//! therefore captures the empty string, answers "not superseded", exits
//! 0, and lets the deploy proceed. Measured, not assumed:
//!
//! | scenario | shell exit | `$GITHUB_OUTPUT` |
//! |---|---|---|
//! | shallow clone, `$GITHUB_SHA` outside the history | **0** | empty |
//! | `$GITHUB_SHA` naming an unknown object | **0** | empty |
//! | `origin` unreachable, so L1 fails | **128** | empty |
//!
//! The port reproduces all three, because a port is not the place to
//! change behavior. Making a broken revision range fatal is a real
//! improvement and a separate change; until then
//! [`a_broken_revision_range_answers_not_superseded`] is the record that
//! today's answer is deliberate rather than accidental, and it fails
//! loudly on whoever changes it on one side only.
//!
//! # What is compared, and where the comparison stops
//!
//! `$GITHUB_OUTPUT` is compared as bytes, including its absence and its
//! emptiness, and every scenario creates it before the run so "wrote
//! nothing" is distinguishable from "wrote and then truncated". stdout
//! is compared as bytes everywhere; neither side ever writes any.
//!
//! stderr is compared byte for byte only where the guard is the sole
//! writer, which is every scenario `git` stays quiet in - all of them
//! but the three above, since L1 passes `--quiet` and a successful L2
//! says nothing. Where `git` wrote its own `fatal:`, its wording is not
//! the guard's to reproduce and the port additionally renders its own
//! diagnostic, so the assertion narrows to "both sides said something".
//!
//! The exit status is compared exactly wherever the guard chose it,
//! which is every 0, the fail-open answers included. It is not compared
//! exactly on the L1 failure, where the shell propagated `git`'s own 128
//! and the port collapses it to 1 (the ceiling the `registry-verify`
//! port set); there the assertion is that both sides refused.
//!
//! # Not covered here, and why
//!
//! - **Whether the guard's path list still matches the workflow's own
//!   `paths:` filter.** A differential cannot answer that: both sides
//!   are handed the same list by construction, so a wrong list is a
//!   list both sides get equally wrong. `tests/path_parity.rs` owns the
//!   question. What this suite covers instead is one scenario per entry
//!   class - a listed file, a listed directory, a nested hit, an
//!   unlisted sibling - which is what says the list is *consulted* the
//!   same way.
//! - **A concurrent push landing between the two runs.** The corpus
//!   fixes `origin` before either side starts and no scenario mutates it
//!   afterwards, so the race the guard exists to lose is out of scope
//!   here by construction.
//! - **An empty path list.** The port refuses one, where the shell had
//!   no such case to refuse: the list was a literal there, so the only
//!   way to reach it is to change the workflow. That is the port's one
//!   deliberate departure and it is covered by a unit test beside the
//!   port; a scenario here would only assert a divergence this suite
//!   exists to say does not otherwise happen.
//!
//! The suite is Unix-only outright. The original is a bash script; a
//! Windows host's `bash` lookalike EXISTS on `PATH` and would pass a
//! presence check while meaning something else. Every test skips rather
//! than fails when a tool it needs is missing.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards:
//!
//! - Changing [`SUPERSEDED`]'s `true` to `TRUE` failed all three tests
//!   that expect a positive answer - `a listed file: $GITHUB_OUTPUT is
//!   superseded=true\n, expected superseded=TRUE\n` - so the file's
//!   bytes really are read rather than assumed.
//! - Dropping `crates/xtask-registry-smoke/` from the port side's
//!   `--path` list alone failed exactly one scenario, `the smoke crate:
//!   $GITHUB_OUTPUT / shell: superseded=true\n / port:` (empty), with
//!   the other seven tests still passing - so the list really does
//!   reach the port, a missing entry really is caught, and the catch is
//!   specific rather than collateral.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_fs::TempDir;

/// The list the port is handed, which is the list baked into the
/// fixture - not the workflow's live one. The two differ by exactly
/// `crates/xtask-workflow-guard/`, which this change adds to the guard
/// and to the trigger filter together; the fixture predates it and, as
/// the original, can never gain it. Handing the port the live list
/// instead would make the smoke-crate scenario's sibling diverge for a
/// reason that is a deliberate addition rather than a port defect, and
/// a differential that reports intended changes as failures stops being
/// read. `tests/path_parity.rs` is what keeps the live list honest.
const PATHS: [&str; 12] = [
    ".github/workflows/registry.yml",
    ".cargo/config.toml",
    "registry/",
    "crates/xtask-registry-guard/",
    "crates/xtask-registry-fixtures/",
    "crates/xtask-registry-admin/",
    "crates/xtask-registry-smoke/",
    "Cargo.toml",
    "crates/cabin-package/",
    "crates/cabin-publish/",
    "crates/cabin-registry-api/",
    "crates/cabin-core/",
];

/// L9's line, which is the whole of what a positive answer writes.
const SUPERSEDED: &[u8] = b"superseded=true\n";

/// A `$GITHUB_SHA` no object can match.
const UNKNOWN_SHA: &str = "0000000000000000000000000000000000000000";

/// Handed to every git call the harness makes and to both sides of every
/// run, so the corpus is reproducible and the machine's own git
/// configuration is neither read nor writable.
const GIT_ENVIRONMENT: [(&str, &str); 9] = [
    ("GIT_AUTHOR_NAME", "cabin differential"),
    ("GIT_AUTHOR_EMAIL", "differential@example.invalid"),
    ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_COMMITTER_NAME", "cabin differential"),
    ("GIT_COMMITTER_EMAIL", "differential@example.invalid"),
    ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
    // An unreachable remote must fail rather than block on a credential
    // prompt: an interactive git would hang the suite, not fail it.
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// How the run's checkout is built, and where `$GITHUB_SHA` points.
enum Run {
    /// `actions/checkout` with `fetch-depth: 0`: a full clone detached
    /// at the numbered commit of the corpus.
    At(usize),
    /// The same, cloned `--depth 1`, so `$GITHUB_SHA` names a commit the
    /// checkout does not contain.
    Shallow(usize),
    /// A full clone whose `origin` is repointed at something that is not
    /// a repository, so L1 fails.
    OriginGone(usize),
    /// A full clone plus one commit that was never pushed, touching the
    /// named path: `origin/main` is behind `$GITHUB_SHA`.
    Ahead(&'static str),
    /// A full clone and a `$GITHUB_SHA` nothing resolves to.
    UnknownSha,
}

/// How far stderr and the exit status can be compared.
enum Diagnostics {
    /// `git` stayed quiet, so the guard was the only writer: compare
    /// both byte for byte.
    Quiet,
    /// `git` wrote its own `fatal:`. Assert both sides said something,
    /// and compare the status as success-or-failure only.
    Git,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// The `$GITHUB_OUTPUT` file's bytes, or `None` if the run removed
    /// it. Every scenario creates the file first, so an empty vector is
    /// "wrote nothing" and not "never existed".
    output: Option<Vec<u8>>,
}

impl Outcome {
    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// One scenario's `origin`: a bare repository carrying a `main` branch,
/// plus the hash of every commit on it, in order.
struct Corpus {
    dir: TempDir,
    url: String,
    commits: Vec<String>,
    /// Distinguishes the two checkouts of one corpus.
    clones: usize,
}

impl Corpus {
    /// Builds `origin` by committing each entry of `history` - one
    /// commit per entry, touching every path it names - through a
    /// publisher clone, then pushing `main` once.
    fn build(history: &[&[&str]]) -> Self {
        let dir = TempDir::new().expect("a scratch directory");
        let bare = dir.path().join("origin.git");
        let publisher = dir.path().join("publisher");
        let url = format!("file://{}", bare.display());

        git(
            dir.path(),
            &["init", "--quiet", "--bare", "-b", "main", &show(&bare)],
        );
        git(
            dir.path(),
            &["init", "--quiet", "-b", "main", &show(&publisher)],
        );
        git(&publisher, &["remote", "add", "origin", &url]);

        let mut commits = Vec::new();
        for (nth, touched) in history.iter().enumerate() {
            for path in *touched {
                write_under(&publisher, path, &format!("commit {nth}\n"));
            }
            git(&publisher, &["add", "--all"]);
            let message = format!("commit {nth}");
            git(&publisher, &["commit", "--quiet", "-m", &message]);
            commits.push(head(&publisher));
        }
        git(&publisher, &["push", "--quiet", "origin", "main"]);

        Self {
            dir,
            url,
            commits,
            clones: 0,
        }
    }

    /// Lays out one side's checkout and answers the `$GITHUB_SHA` it is
    /// to be run with.
    fn checkout(&mut self, run: &Run) -> (PathBuf, String) {
        let into = self.dir.path().join(format!("checkout-{}", self.clones));
        self.clones += 1;
        let root = self.dir.path().to_owned();
        let clone = |args: &[&str]| git(&root, args);

        match *run {
            Run::At(commit) | Run::OriginGone(commit) => {
                clone(&["clone", "--quiet", &self.url, &show(&into)]);
                let sha = self.commits[commit].clone();
                git(&into, &["checkout", "--quiet", "--detach", &sha]);
                if matches!(*run, Run::OriginGone(_)) {
                    let gone = format!("file://{}/not-a-repository.git", root.display());
                    git(&into, &["remote", "set-url", "origin", &gone]);
                }
                (into, sha)
            }
            Run::Shallow(commit) => {
                // `--depth` needs a `file://` URL: git ignores it for a
                // plain local path and hardlinks the whole object store.
                clone(&["clone", "--quiet", "--depth", "1", &self.url, &show(&into)]);
                assert_eq!(
                    git_output(&into, &["rev-parse", "--is-shallow-repository"]).trim(),
                    "true",
                    "the checkout was meant to be shallow"
                );
                (into, self.commits[commit].clone())
            }
            Run::Ahead(path) => {
                clone(&["clone", "--quiet", &self.url, &show(&into)]);
                write_under(&into, path, "unpushed\n");
                git(&into, &["add", "--all"]);
                git(&into, &["commit", "--quiet", "-m", "unpushed"]);
                let sha = head(&into);
                (into, sha)
            }
            Run::UnknownSha => {
                clone(&["clone", "--quiet", &self.url, &show(&into)]);
                (into, UNKNOWN_SHA.to_owned())
            }
        }
    }
}

fn write_under(root: &Path, path: &str, contents: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().expect("a path with a parent"))
        .expect("the commit's directory");
    fs::write(&file, contents).expect("the commit's file");
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn git(dir: &Path, args: &[&str]) {
    let _ = git_output(dir, args);
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.current_dir(dir).args(args);
    for (name, value) in GIT_ENVIRONMENT {
        command.env(name, value);
    }
    let done: Output = command
        .output()
        .expect("running one of the harness's own git commands");
    assert!(
        done.status.success(),
        "the harness's `git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&done.stderr)
    );
    String::from_utf8_lossy(&done.stdout).into_owned()
}

fn head(dir: &Path) -> String {
    git_output(dir, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn ready(test: &str) -> bool {
    for tool in ["bash", "git"] {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/superseded.sh.orig")
}

/// Runs one side and collects everything it produced. `seed` is what the
/// runner had already written to `$GITHUB_OUTPUT`, which is empty in
/// every scenario but the one pinning the append. The file lives beside
/// the checkout rather than inside it, so it cannot reach a commit or a
/// pathspec.
fn once(mut command: Command, dir: &Path, sha: &str, seed: &[u8]) -> Outcome {
    let output_file = dir.with_extension("output");
    fs::write(&output_file, seed).expect("the runner's $GITHUB_OUTPUT");

    command.current_dir(dir);
    for (name, value) in GIT_ENVIRONMENT {
        command.env(name, value);
    }
    let produced = command
        .env("GITHUB_SHA", sha)
        .env("GITHUB_OUTPUT", &output_file)
        .output()
        .expect("running one side of the scenario");

    Outcome {
        stdout: produced.stdout,
        stderr: produced.stderr,
        status: produced.status.code(),
        output: fs::read(&output_file).ok(),
    }
}

/// Runs both sides of one scenario over two byte-identical checkouts of
/// the same `origin`.
fn both(corpus: &mut Corpus, run: &Run) -> (Outcome, Outcome) {
    both_seeded(corpus, run, b"")
}

fn both_seeded(corpus: &mut Corpus, run: &Run, seed: &[u8]) -> (Outcome, Outcome) {
    let (shell_dir, shell_sha) = corpus.checkout(run);
    let (port_dir, port_sha) = corpus.checkout(run);
    assert_eq!(
        shell_sha, port_sha,
        "the two checkouts disagree on $GITHUB_SHA: the corpus is not reproducible"
    );

    // GitHub's default `run:` shell on Linux is `bash -e {0}`: `-e` on,
    // `-u` and `-o pipefail` off. Reproduced exactly, because which of
    // the two git calls the shell aborts on is the whole finding.
    let mut bash = Command::new("bash");
    bash.arg("-e").arg(fixture());
    let shell = once(bash, &shell_dir, &shell_sha, seed);

    let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-workflow-guard"));
    ported.arg("superseded");
    for path in PATHS {
        ported.args(["--path", path]);
    }
    let port = once(ported, &port_dir, &port_sha, seed);

    (shell, port)
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
    match *diagnostics {
        Diagnostics::Quiet => {
            assert!(
                shell.stderr == port.stderr,
                "{case}: stderr\nshell: {}\nport:  {}",
                shell.stderr.escape_ascii(),
                port.stderr.escape_ascii()
            );
            assert_eq!(shell.status, port.status, "{case}: exit status");
        }
        Diagnostics::Git => {
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert!(
                    !outcome.stderr.is_empty(),
                    "{case}: the {side} said nothing about a git failure"
                );
            }
            // The status ceiling: where the shell propagated git's own
            // code the port collapses it to 1, so only the verdict is
            // compared. Where the shell chose 0 - the fail-open answer -
            // that 0 is compared exactly by this same assertion.
            assert_eq!(
                shell.status == Some(0),
                port.status == Some(0),
                "{case}: one side succeeded and the other did not (shell {:?}, port {:?})",
                shell.status,
                port.status
            );
        }
    }
}

fn rendered(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "<the file was removed>".to_owned(),
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

/// `origin/main` is the commit the run is for: there is nothing after
/// it, so nothing is written and the step is silent.
#[test]
fn an_up_to_date_checkout_writes_nothing() {
    if !ready("an_up_to_date_checkout_writes_nothing") {
        return;
    }
    let mut corpus = Corpus::build(&[&["README.md"]]);
    let (shell, port) = both(&mut corpus, &Run::At(0));
    diff("up to date", &shell, &port, &Diagnostics::Quiet);
    wrote("up to date", &shell, b"");
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a successful run is silent");
}

/// One newer commit touching a watched path, reached four ways: a listed
/// file, a listed directory's immediate child, a listed directory's
/// deeply nested child, and the entry the guard's list was missing until
/// `d454d37a1`.
#[test]
fn a_newer_commit_on_a_watched_path_is_superseded() {
    if !ready("a_newer_commit_on_a_watched_path_is_superseded") {
        return;
    }
    let cases = [
        ("a listed file", ".cargo/config.toml"),
        ("a listed directory", "registry/src/lib.rs"),
        (
            "a nested hit under a listed directory",
            "registry/src/a/b/c/deep.rs",
        ),
        ("the smoke crate", "crates/xtask-registry-smoke/src/lib.rs"),
    ];
    for (case, path) in cases {
        let mut corpus = Corpus::build(&[&["README.md"], &[path]]);
        let (shell, port) = both(&mut corpus, &Run::At(0));
        diff(case, &shell, &port, &Diagnostics::Quiet);
        wrote(case, &shell, SUPERSEDED);
        assert_eq!(shell.status, Some(0), "{case}: a skip is not a failure");
        assert!(
            shell.stderr.is_empty(),
            "{case}: a positive answer is silent"
        );
    }
}

/// A newer commit touching nothing the guard watches leaves the deploy
/// alone. `Cargo.lock` is the deliberate omission: it moves on every
/// dependency bump and cannot on its own change what the Worker ships.
#[test]
fn a_newer_commit_off_the_watched_paths_is_not() {
    if !ready("a_newer_commit_off_the_watched_paths_is_not") {
        return;
    }
    for (case, path) in [
        ("the website", "website/src/pages/index.astro"),
        ("the lockfile", "Cargo.lock"),
    ] {
        let mut corpus = Corpus::build(&[&["README.md"], &[path]]);
        let (shell, port) = both(&mut corpus, &Run::At(0));
        diff(case, &shell, &port, &Diagnostics::Quiet);
        wrote(case, &shell, b"");
        assert_eq!(shell.status, Some(0), "{case}");
    }
}

/// `origin/main` behind the run - a commit the checkout has and the
/// remote does not - is the empty range, not a newer commit, however
/// many watched paths that commit touches.
#[test]
fn an_origin_behind_the_run_is_not_superseded() {
    if !ready("an_origin_behind_the_run_is_not_superseded") {
        return;
    }
    let mut corpus = Corpus::build(&[&["README.md"]]);
    let (shell, port) = both(&mut corpus, &Run::Ahead("registry/src/lib.rs"));
    diff("origin is behind", &shell, &port, &Diagnostics::Quiet);
    wrote("origin is behind", &shell, b"");
    assert_eq!(shell.status, Some(0));
}

/// `-n 1` stops at the first match, so a matching commit followed by a
/// non-matching one still supersedes: the answer is "main moved past
/// this run", not "main's tip is newer".
#[test]
fn only_one_of_the_newer_commits_needs_to_match() {
    if !ready("only_one_of_the_newer_commits_needs_to_match") {
        return;
    }
    let mut corpus = Corpus::build(&[
        &["README.md"],
        &["registry/src/lib.rs"],
        &["website/src/pages/index.astro"],
    ]);
    let (shell, port) = both(&mut corpus, &Run::At(0));
    diff(
        "the older of two matches",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    wrote("the older of two matches", &shell, SUPERSEDED);
    assert_eq!(shell.status, Some(0));
}

/// **The fail-open finding.** A revision range `git` cannot resolve - a
/// `$GITHUB_SHA` outside a shallow checkout's history, or one no object
/// matches - is swallowed by the `if` condition: the guard writes
/// nothing, exits 0, and the deploy proceeds. Both corpora carry a newer
/// commit under `registry/`, so a resolvable range would have answered
/// "superseded"; that is what makes this fail-*open* rather than merely
/// quiet. Both sides must agree, and whoever makes this fatal on one
/// side will land here first.
#[test]
fn a_broken_revision_range_answers_not_superseded() {
    if !ready("a_broken_revision_range_answers_not_superseded") {
        return;
    }
    for (case, run) in [
        ("a shallow checkout", Run::Shallow(0)),
        ("an unknown sha", Run::UnknownSha),
    ] {
        let mut corpus = Corpus::build(&[&["README.md"], &["registry/src/lib.rs"]]);
        let (shell, port) = both(&mut corpus, &run);
        diff(case, &shell, &port, &Diagnostics::Git);
        wrote(case, &shell, b"");
        wrote(case, &port, b"");
        assert_eq!(
            shell.status,
            Some(0),
            "{case}: the shell swallowed the failure and succeeded"
        );
        assert!(
            shell.err().contains("fatal:"),
            "{case}: git did not diagnose the range it could not resolve: {:?}",
            shell.err()
        );
    }
}

/// L1 is the one fail-safe half: a standalone command under `set -e`, so
/// an `origin` that cannot be reached stops the step instead of letting
/// a stale run deploy.
#[test]
fn an_unreachable_origin_fails_the_step() {
    if !ready("an_unreachable_origin_fails_the_step") {
        return;
    }
    let mut corpus = Corpus::build(&[&["README.md"]]);
    let (shell, port) = both(&mut corpus, &Run::OriginGone(0));
    diff("origin is gone", &shell, &port, &Diagnostics::Git);
    wrote("origin is gone", &shell, b"");
    wrote("origin is gone", &port, b"");
    assert_ne!(shell.status, Some(0), "the shell must refuse");
    assert_ne!(port.status, Some(0), "the port must refuse");
}

/// L9 appends. `$GITHUB_OUTPUT` is a file the runner may already have
/// written into, so a port that truncated would drop what came before it
/// and still pass every other scenario here, where the file starts
/// empty.
#[test]
fn an_existing_output_file_is_appended_to() {
    if !ready("an_existing_output_file_is_appended_to") {
        return;
    }
    let mut corpus = Corpus::build(&[&["README.md"], &["registry/src/lib.rs"]]);
    let seed = b"written-earlier=kept\n";
    let (shell, port) = both_seeded(&mut corpus, &Run::At(0), seed);
    diff(
        "an already-written output",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );

    let mut expected = seed.to_vec();
    expected.extend_from_slice(SUPERSEDED);
    wrote("an already-written output", &shell, &expected);
    wrote("an already-written output", &port, &expected);
}
