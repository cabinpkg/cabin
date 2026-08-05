//! The whole-run differential for the registry-deploy wait: the shell
//! it replaces and the port, run over one corpus of canned GitHub API
//! answers and throwaway git repositories, compared on stdout, stderr,
//! exit status and the sequence of requests each side issued.
//!
//! `tests/fixtures/await-deploy.sh.orig` is the original, byte for
//! byte: the `run:` block of the "Wait for a registry deploy containing
//! this SHA" step of `.github/workflows/ports-publish.yml` as it stood
//! on `main` at `a3e9a95f6`, dedented 10 spaces, `sha256`
//! `9ae81f1aad94df0d96d39a42cd12af8f233d2366272cb5ac437952bcb7c98d01`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # The seam: one fake `gh`, two callers
//!
//! `tests/fixtures/fake-bin` goes first on both sides' `PATH`, so the
//! shell's `gh api ...` and the port's spawn of the same argv reach the
//! same stand-in over the same canned service. It answers from
//! `$FAKE_GH_DIR` with text already projected the way `gh --jq` prints
//! it, and it appends every argv it was called with to `$FAKE_GH_LOG`.
//!
//! That log is what makes this a differential of *requests* and not
//! only of output. Each side gets its own log and [`diff`] asserts the
//! two are the identical sequence, so a port that asked for a different
//! URL, reordered its arguments, reworded a `--jq` expression, or
//! skipped a call the shell made fails here even when it happens to
//! reach the same verdict. Several scenarios lean on that directly: the
//! non-descendant candidate is pinned by the *absence* of a jobs
//! request, which is the only observable difference between "checked
//! the ancestry first" and "checked it after asking".
//!
//! `git` is not stubbed. Every scenario builds a bare repository as
//! `origin` and clones it once per side, so `git merge-base
//! --is-ancestor` answers about real commits: `$GITHUB_SHA` is the
//! second of four, and a candidate's head is either a later commit
//! (contains it) or the first (does not). The harness never reads or
//! writes the machine's git configuration, and the identity and dates
//! come from the environment.
//!
//! # What is compared, and where the comparison stops
//!
//! stdout is compared as bytes everywhere - it carries the step's whole
//! account of itself, including the lines a waiting run prints before
//! it succeeds. The exit status is compared exactly wherever the guard
//! chooses it: `git fetch`'s failure is swallowed by `|| true`, and
//! every status but one is an `exit` the script wrote. The exception is
//! the death at the conclusion assignment, where the shell propagates
//! `jq`'s own version-dependent code and the port's documented ceiling
//! collapses to 1 - that scenario compares the refusal alone.
//!
//! stderr is compared byte for byte wherever the guard is the sole
//! writer, which is every scenario but two: the broken `origin`, where
//! `git`'s own `fatal:` is not the guard's wording to reproduce, and
//! the malformed capture, where the noise is `jq`'s and `bash`'s own -
//! both narrow to "this side said something". The refusal in
//! [`a_failed_conclusion_with_nothing_pending_refuses_to_publish`] is
//! compared byte for byte, which is the pin that matters most on that
//! path: the shell writes a bare sentence, so a port that renders it
//! through its error type would prefix it and diverge.
//!
//! # Timing
//!
//! The step's retry is a hard-coded `sleep 40`, and a port that is
//! faithful sleeps for real - so does this suite, on both sides. Every
//! scenario therefore terminates on iteration 1, except four that need
//! a second iteration to show what the retry does; those cost 80
//! seconds each. Two of them run by default and two carry `#[ignore]`
//! to keep the suite near two minutes:
//!
//! ```text
//! cargo test -p xtask-workflow-guard --test await_deploy_differential -- --ignored
//! ```
//!
//! The 90-iteration ceiling is deliberately not here: reaching it costs
//! an hour per side. The port pins it over an injectable clock instead.
//!
//! # Not covered here, and why
//!
//! - **Whether `gh` really answers the way the corpus says.** Both
//!   sides are handed the same stand-in by construction, so a wrong
//!   response shape is one both sides get equally wrong. What this
//!   suite covers is that the two *ask* the same questions and read the
//!   answers the same way.
//! - **A response arriving over more than one iteration boundary.**
//!   Scenarios stop at two iterations; the loop's own bookkeeping past
//!   that is the port's unit tests' problem, not a place the two
//!   implementations can disagree without disagreeing at two.
//! - **The exact status of a death at the conclusion assignment.**
//!   [`a_malformed_runs_capture_kills_the_step_on_both_sides`] pins the
//!   flow - the length check survives its condition, the assignment
//!   dies - but the shell exits with `jq`'s own code, which varies by
//!   jq version, where the port's documented ceiling collapses to 1;
//!   and the diagnostics are bash's and jq's own wording. That scenario
//!   therefore compares refusal, not number or noise.
//!
//! The suite is Unix-only outright. The original is a bash script; a
//! Windows host's `bash` lookalike EXISTS on `PATH` and would pass a
//! presence check while meaning something else. Every test skips rather
//! than fails when a tool it needs is missing, and the harness's own
//! failures panic.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards:
//!
//! - **A one-sided message divergence is caught.** Changing the
//!   fixture's `registry deploy $rid` to `registry deployed $rid`,
//!   which perturbs one side only and is the shape of a port that
//!   reworded a line, failed exactly the three scenarios that print
//!   it (`a successful deploy: stdout / shell: registry deployed 7001
//!   ... / port: registry deploy 7001 ...`) and left the other four
//!   passing. So the compared bytes really are each side's own, and
//!   the catch is specific rather than collateral.
//! - **The request-parity assertion is load-bearing.** Dropping the
//!   port side's last log line in [`World::side`], which is a port
//!   that made one request fewer with its output and status
//!   unchanged, failed all seven on `the two sides asked for
//!   different things` and on nothing else. A skipped request is
//!   therefore caught on its own, rather than only when it happens to
//!   change the verdict.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_fs::TempDir;

/// The repository the run pretends to belong to. It only ever reaches
/// the fake `gh`, which dispatches on the URL's query rather than its
/// owner - but it is part of every logged argv, so both sides must
/// build the same URL from it.
const REPOSITORY: &str = "cabinpkg/cabin";

/// The four commits every scenario's `main` carries. `$GITHUB_SHA` is
/// [`RUN`]; [`AFTER`] and [`LATER`] come after it, so a candidate
/// headed at either passes `git merge-base --is-ancestor`, and
/// [`BEFORE`] is one that does not.
const BEFORE: usize = 0;
const RUN: usize = 1;
const AFTER: usize = 2;
const LATER: usize = 3;

/// What a run that triggered no Registry deploy prints.
const ALREADY_CURRENT: &str =
    "no Registry run for this SHA; the deployed worker is already current\n";

/// Handed to every git call the harness makes and to both sides of
/// every run, so the corpus is reproducible and the machine's own git
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

/// How far stderr can be compared.
enum Diagnostics {
    /// The guard was the only writer: compare byte for byte.
    Quiet,
    /// `git` wrote its own `fatal:`. Assert both sides said something;
    /// the wording is each side's own.
    Git,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// The argv of every `gh` call the side made, in order.
    log: Vec<String>,
}

impl Outcome {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// How many requests carried `fragment`.
    fn requests(&self, fragment: &str) -> usize {
        self.log
            .iter()
            .filter(|call| call.contains(fragment))
            .count()
    }
}

/// One scenario: an `origin` to clone, the commits on it, and the
/// canned answers the fake `gh` serves.
struct World {
    dir: TempDir,
    url: String,
    commits: Vec<String>,
    /// Shared by both sides, because the service is one service.
    responses: PathBuf,
    /// Repoints each side's `origin` at nothing, so the step's
    /// `git fetch` fails.
    origin_gone: bool,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("a scratch directory");
        let bare = dir.path().join("origin.git");
        let publisher = dir.path().join("publisher");
        let url = format!("file://{}", bare.display());
        let responses = dir.path().join("responses");
        fs::create_dir(&responses).expect("the canned responses directory");

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
        for nth in 0..=LATER {
            fs::write(publisher.join("history"), format!("commit {nth}\n"))
                .expect("the commit's file");
            git(&publisher, &["add", "--all"]);
            let message = format!("commit {nth}");
            git(&publisher, &["commit", "--quiet", "-m", &message]);
            commits.push(
                git_output(&publisher, &["rev-parse", "HEAD"])
                    .trim()
                    .to_owned(),
            );
        }
        git(&publisher, &["push", "--quiet", "origin", "main"]);

        Self {
            dir,
            url,
            commits,
            responses,
            origin_gone: false,
        }
    }

    fn sha(&self, nth: usize) -> &str {
        &self.commits[nth]
    }

    /// Cans one answer. `name` is the request kind - `candidates`,
    /// `runs`, `pending`, `deploy-<id>` - optionally suffixed `.<n>` to
    /// answer only that kind's nth call, which is how a scenario makes
    /// iteration 2 differ from iteration 1.
    fn respond(&self, name: &str, code: i32, body: &str) {
        fs::write(self.responses.join(name), format!("{code}\n{body}")).expect("a canned response");
    }

    /// Runs both sides over two byte-identical checkouts of the same
    /// `origin`, each with its own request log.
    fn both(&self) -> (Outcome, Outcome) {
        let gh = fake_bin().join("gh");
        let mode = fs::metadata(&gh).expect("the fake gh").permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "{} lost its executable bit, so PATH would find the real gh",
            show(&gh)
        );

        // GitHub's default `run:` shell on Linux is `bash -e {0}`: `-e`
        // on, `-u` and `-o pipefail` off. Reproduced exactly, because
        // which failures the script survives is most of the corpus.
        let mut bash = Command::new("bash");
        bash.arg("-e").arg(fixture());
        let shell = self.side("shell", bash);

        let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-workflow-guard"));
        ported.arg("await-deploy");
        let port = self.side("port", ported);

        (shell, port)
    }

    fn side(&self, name: &str, mut command: Command) -> Outcome {
        let checkout = self.dir.path().join(name);
        git(
            self.dir.path(),
            &["clone", "--quiet", &self.url, &show(&checkout)],
        );
        git(
            &checkout,
            &["checkout", "--quiet", "--detach", self.sha(RUN)],
        );
        if self.origin_gone {
            let gone = format!("file://{}/not-a-repository.git", self.dir.path().display());
            git(&checkout, &["remote", "set-url", "origin", &gone]);
        }

        let log = self.dir.path().join(format!("{name}.requests"));
        fs::write(&log, b"").expect("the request log");

        command.current_dir(&checkout);
        for (key, value) in GIT_ENVIRONMENT {
            command.env(key, value);
        }
        let produced: Output = command
            .env("GITHUB_REPOSITORY", REPOSITORY)
            .env("GITHUB_SHA", self.sha(RUN))
            .env("GH_TOKEN", "fake-token")
            .env("PATH", path_through_the_fake_gh())
            .env("FAKE_GH_LOG", &log)
            .env("FAKE_GH_DIR", &self.responses)
            .output()
            .expect("running one side of the scenario");

        Outcome {
            stdout: produced.stdout,
            stderr: produced.stderr,
            status: produced.status.code(),
            log: fs::read_to_string(&log)
                .expect("the request log")
                .lines()
                .map(str::to_owned)
                .collect(),
        }
    }
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
    let done = command
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

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/await-deploy.sh.orig")
}

fn fake_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-bin")
}

fn path_through_the_fake_gh() -> std::ffi::OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = vec![fake_bin()];
    directories.extend(std::env::split_paths(&inherited));
    std::env::join_paths(directories).expect("a PATH with the fake gh first")
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn ready(test: &str) -> bool {
    // `jq` is standalone here: the step pipes the run list through it
    // rather than through `gh --jq`.
    for tool in ["bash", "git", "jq"] {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, diagnostics: &Diagnostics) {
    // First, because a scenario missing a canned answer would otherwise
    // report as whatever the two sides did about it.
    for (side, outcome) in [("shell", shell), ("port", port)] {
        assert!(
            !outcome.err().contains("fake gh:"),
            "{case}: the {side}'s fake gh refused a request: {}",
            outcome.err()
        );
    }
    assert!(
        shell.log == port.log,
        "{case}: the two sides asked for different things\nshell: {:#?}\nport:  {:#?}",
        shell.log,
        port.log
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
        Diagnostics::Git => {
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert!(
                    !outcome.stderr.is_empty(),
                    "{case}: the {side} said nothing about a git failure"
                );
            }
        }
    }
}

/// The satisfied case: a successful `main` run whose head contains this
/// SHA and whose `Deploy` step succeeded.
#[test]
fn a_successful_deploy_containing_this_sha_satisfies_the_gate() {
    if !ready("a_successful_deploy_containing_this_sha_satisfies_the_gate") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, &format!("7001 {}\n", world.sha(AFTER)));
    world.respond("deploy-7001", 0, "success\n");

    let (shell, port) = world.both();
    diff("a successful deploy", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        format!(
            "registry deploy 7001 (head {}) contains this SHA\n",
            world.sha(AFTER)
        )
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a satisfied gate is silent");
    assert_eq!(shell.log.len(), 2, "one candidate scan and one jobs call");
}

/// The scan does not stop at the first candidate that contains this
/// SHA: a `Deploy` that was skipped, a run with no `deploy-registry`
/// job at all (the `--jq` selection answers `null`), and a jobs call
/// that failed outright (`|| echo transient`) each leave the loop
/// looking at the next candidate.
#[test]
fn the_scan_walks_past_a_candidate_whose_deploy_did_not_succeed() {
    if !ready("the_scan_walks_past_a_candidate_whose_deploy_did_not_succeed") {
        return;
    }
    for (case, code, body) in [
        ("a skipped Deploy", 0, "skipped\n"),
        ("no deploy-registry job", 0, "null\n"),
        ("a jobs call that failed", 1, ""),
    ] {
        let world = World::new();
        world.respond(
            "candidates",
            0,
            &format!("7001 {}\n7002 {}\n", world.sha(AFTER), world.sha(LATER)),
        );
        world.respond("deploy-7001", code, body);
        world.respond("deploy-7002", 0, "success\n");

        let (shell, port) = world.both();
        diff(case, &shell, &port, &Diagnostics::Quiet);
        assert_eq!(
            shell.out(),
            format!(
                "registry deploy 7002 (head {}) contains this SHA\n",
                world.sha(LATER)
            ),
            "{case}"
        );
        assert_eq!(shell.status, Some(0), "{case}");
        assert_eq!(
            shell.requests("/jobs?"),
            2,
            "{case}: both candidates were inspected"
        );
    }
}

/// The ancestor check runs before the jobs call: a candidate that does
/// not contain this SHA costs no request at all.
#[test]
fn a_candidate_that_does_not_contain_this_sha_is_never_inspected() {
    if !ready("a_candidate_that_does_not_contain_this_sha_is_never_inspected") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, &format!("7001 {}\n", world.sha(BEFORE)));
    world.respond("runs", 0, "[]\n");

    let (shell, port) = world.both();
    diff(
        "a non-descendant candidate",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(shell.out(), ALREADY_CURRENT);
    assert_eq!(shell.status, Some(0));
    assert_eq!(
        shell.requests("/jobs?"),
        0,
        "the ancestry decided it before any jobs call"
    );
}

/// No Registry run for this SHA at all: the commit changed nothing the
/// Worker ships, so the deployed Worker is already current.
#[test]
fn no_registry_run_for_this_sha_leaves_the_worker_alone() {
    if !ready("no_registry_run_for_this_sha_leaves_the_worker_alone") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, &format!("7001 {}\n", world.sha(AFTER)));
    world.respond("deploy-7001", 0, "skipped\n");
    world.respond("runs", 0, "[]\n");

    let (shell, port) = world.both();
    diff("no run for this SHA", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.out(), ALREADY_CURRENT);
    assert_eq!(shell.status, Some(0));
    assert!(
        shell.stderr.is_empty(),
        "an already-current worker is silent"
    );
}

/// A candidate list that yields nothing - the call failed, or it
/// succeeded with no runs to report - is not a failure: the same-SHA
/// branch still decides, and neither shape says anything about itself.
#[test]
fn an_unusable_candidate_list_still_reaches_the_same_sha_branch() {
    if !ready("an_unusable_candidate_list_still_reaches_the_same_sha_branch") {
        return;
    }
    for (case, code) in [("the call failed", 1), ("nothing matched", 0)] {
        let world = World::new();
        world.respond("candidates", code, "");
        world.respond("runs", 0, "[]\n");

        let (shell, port) = world.both();
        diff(case, &shell, &port, &Diagnostics::Quiet);
        assert_eq!(shell.out(), ALREADY_CURRENT, "{case}");
        assert_eq!(shell.status, Some(0), "{case}");
        assert!(shell.stderr.is_empty(), "{case}: neither shape diagnoses");
        assert_eq!(shell.requests("/jobs?"), 0, "{case}: nothing to inspect");
    }
}

/// A failed run for this SHA is not terminal while something pending
/// could still deliver the deploy: the step says so, waits, and the
/// next iteration finds the deploy.
#[test]
fn a_pending_run_containing_this_sha_defers_a_failed_conclusion() {
    if !ready("a_pending_run_containing_this_sha_defers_a_failed_conclusion") {
        return;
    }
    let world = World::new();
    world.respond("candidates.1", 0, "");
    world.respond(
        "runs.1",
        0,
        "[{\"conclusion\":\"failure\",\"status\":\"completed\"}]\n",
    );
    world.respond("pending.1", 0, &format!("{}\n", world.sha(AFTER)));
    world.respond("candidates.2", 0, &format!("7001 {}\n", world.sha(AFTER)));
    world.respond("deploy-7001", 0, "success\n");

    let (shell, port) = world.both();
    diff("a pending descendant", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        format!(
            "Registry run for this SHA concluded 'failure'; a pending run containing it may still \
             deploy; waiting\nregistry deploy 7001 (head {}) contains this SHA\n",
            world.sha(AFTER)
        )
    );
    assert_eq!(shell.status, Some(0));
    assert_eq!(shell.log.len(), 5, "two iterations' worth of requests");
}

/// The one refusal: a failed or cancelled run for this SHA with nothing
/// pending that contains it. The message is the shell's bare sentence
/// on stderr, which a port rendering it through an error type would
/// prefix.
#[test]
fn a_failed_conclusion_with_nothing_pending_refuses_to_publish() {
    if !ready("a_failed_conclusion_with_nothing_pending_refuses_to_publish") {
        return;
    }
    for (case, conclusion, pending) in [
        ("a failure, nothing pending", "failure", None),
        (
            "a failure, a pending run that does not contain this SHA",
            "failure",
            Some(BEFORE),
        ),
        ("a cancelled run", "cancelled", None),
    ] {
        let world = World::new();
        world.respond("candidates", 0, "");
        world.respond(
            "runs",
            0,
            &format!("[{{\"conclusion\":\"{conclusion}\",\"status\":\"completed\"}}]\n"),
        );
        let heads = pending.map_or_else(String::new, |nth| format!("{}\n", world.sha(nth)));
        world.respond("pending", 0, &heads);

        let (shell, port) = world.both();
        diff(case, &shell, &port, &Diagnostics::Quiet);
        assert!(
            shell.stdout.is_empty(),
            "{case}: the refusal belongs on stderr"
        );
        assert_eq!(
            shell.err(),
            format!(
                "Registry run for this SHA concluded '{conclusion}' and no deploy containing it \
                 has landed; not publishing against a stale worker\n"
            ),
            "{case}"
        );
        assert_eq!(shell.status, Some(1), "{case}");
    }
}

/// A runs capture that is not JSON: the length check survives inside
/// its condition (`[ "" -eq 0 ]` diagnoses and answers false), and the
/// conclusion assignment - the block's one unguarded substitution -
/// kills the step under `set -e`. The shell dies with jq's own
/// version-dependent status where the port's documented ceiling
/// collapses to 1, so this compares the refusal and the requests, not
/// the number or the tools' wording.
#[test]
fn a_malformed_runs_capture_kills_the_step_on_both_sides() {
    if !ready("a_malformed_runs_capture_kills_the_step_on_both_sides") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, "");
    world.respond("runs", 0, "not json\n");

    let (shell, port) = world.both();
    for (side, outcome) in [("shell", &shell), ("port", &port)] {
        assert!(
            !outcome.err().contains("fake gh:"),
            "the {side}'s fake gh refused a request: {}",
            outcome.err()
        );
        assert!(
            outcome.stdout.is_empty(),
            "{side}: nothing reaches stdout: {}",
            outcome.out()
        );
        assert_ne!(outcome.status, Some(0), "the {side} must die");
        assert!(
            !outcome.stderr.is_empty(),
            "the {side} died without a diagnostic"
        );
    }
    assert!(
        shell.log == port.log,
        "the two sides asked for different things\nshell: {:#?}\nport:  {:#?}",
        shell.log,
        port.log
    );
    assert_eq!(port.status, Some(1), "the documented collapse");
}

/// A failed list of runs for this SHA is transient: say so, wait, ask
/// again.
#[test]
fn a_transient_runs_call_retries_on_the_next_iteration() {
    if !ready("a_transient_runs_call_retries_on_the_next_iteration") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, "");
    world.respond("runs.1", 1, "");
    world.respond("runs.2", 0, "[]\n");

    let (shell, port) = world.both();
    diff("a transient runs call", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        format!("transient API error listing Registry runs; retrying\n{ALREADY_CURRENT}")
    );
    assert_eq!(shell.status, Some(0));
    assert_eq!(shell.log.len(), 4, "the candidate scan ran twice too");
}

/// The same, one branch deeper: the pending list is what failed, after
/// a conclusion that would otherwise have refused.
///
/// `#[ignore]`: a second iteration costs a real `sleep 40` on each
/// side, and [`a_transient_runs_call_retries_on_the_next_iteration`]
/// already keeps a two-iteration scenario in the default run.
#[test]
#[ignore = "two iterations: 80 seconds of real sleep"]
fn a_transient_pending_call_retries_on_the_next_iteration() {
    if !ready("a_transient_pending_call_retries_on_the_next_iteration") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, "");
    world.respond(
        "runs.1",
        0,
        "[{\"conclusion\":\"failure\",\"status\":\"completed\"}]\n",
    );
    world.respond("pending.1", 1, "");
    world.respond("runs.2", 0, "[]\n");

    let (shell, port) = world.both();
    diff(
        "a transient pending call",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.out(),
        format!("transient API error listing pending Registry runs; retrying\n{ALREADY_CURRENT}")
    );
    assert_eq!(shell.status, Some(0));
}

/// A run for this SHA that has not concluded - `.conclusion` is JSON
/// `null` - is neither satisfied nor terminal: the loop waits without
/// saying anything at all.
///
/// `#[ignore]`: a second iteration costs a real `sleep 40` on each
/// side.
#[test]
#[ignore = "two iterations: 80 seconds of real sleep"]
fn an_unconcluded_run_waits_without_a_word() {
    if !ready("an_unconcluded_run_waits_without_a_word") {
        return;
    }
    let world = World::new();
    world.respond("candidates", 0, "");
    world.respond(
        "runs.1",
        0,
        "[{\"conclusion\":null,\"status\":\"in_progress\"}]\n",
    );
    world.respond("runs.2", 0, "[]\n");

    let (shell, port) = world.both();
    diff("an unconcluded run", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        ALREADY_CURRENT,
        "the waiting iteration printed nothing of its own"
    );
    assert_eq!(shell.status, Some(0));
    assert_eq!(shell.log.len(), 4, "two iterations, no pending call");
}

/// `git fetch --quiet origin main || true`: an `origin` that cannot be
/// reached does not stop the wait. The commits the ancestor check needs
/// are already in the checkout, so the gate is satisfied anyway - only
/// `git`'s own complaint reaches stderr.
#[test]
fn a_broken_origin_does_not_stop_the_wait() {
    if !ready("a_broken_origin_does_not_stop_the_wait") {
        return;
    }
    let mut world = World::new();
    world.origin_gone = true;
    world.respond("candidates", 0, &format!("7001 {}\n", world.sha(AFTER)));
    world.respond("deploy-7001", 0, "success\n");

    let (shell, port) = world.both();
    diff("a broken origin", &shell, &port, &Diagnostics::Git);
    assert_eq!(
        shell.out(),
        format!(
            "registry deploy 7001 (head {}) contains this SHA\n",
            world.sha(AFTER)
        )
    );
    assert_eq!(shell.status, Some(0), "the fetch failure is swallowed");
    assert!(
        shell.err().contains("fatal:"),
        "git did not complain about the origin it could not reach: {:?}",
        shell.err()
    );
}
