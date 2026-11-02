//! The registry-deploy wait: the "Wait for a registry deploy
//! containing this SHA" step of `.github/workflows/ports-publish.yml`.
//!
//! - A same-SHA Registry run whose `deploy-registry` JOB itself
//!   concluded `skipped` - the workflow's change gate found no
//!   registry-relevant files in the push, so the deploy job's own
//!   `if` skipped it - answers 0 immediately: the gate is a
//!   deterministic property of the commit (a rerun re-answers the
//!   same), such a run can never deploy, and the deployed Worker is
//!   already current for this SHA.
//!   See `SameShaDeploy`.  SAFETY INVARIANT this arm leans on, pinned
//!   here because the gate lives in `.github/path-filters.yml`: every
//!   path that can change the Worker's accepted schema or the publish
//!   client (`registry/`, `crates/`, the root manifests, `.cargo/`,
//!   the filter file itself) must stay in `registry.yml`'s `registry`
//!   filter, so the only pushes that reach this arm - ports-publish
//!   ran, the registry jobs skipped - are ones touching nothing but
//!   the ports tree or ports-publish's own workflow file, neither of
//!   which any Worker or client code reads.  A `ports` filter entry
//!   absent from the `registry` filter (other than `ports/**` and
//!   `.github/workflows/ports-publish.yml`) would let a
//!   client-changing push publish against a Worker whose conformance
//!   never ran for it.
//! - A same-SHA Registry run that succeeded with its Deploy step
//!   *skipped* takes the failure path's pending-descendant exit
//!   instead of idling into the hour ceiling - see
//!   `SameShaDeploy`.  Only the freshness guard skips that step (a
//!   migration awaiting its by-hand apply fails the job instead), so
//!   the stop means the newer run owning the deploy failed; an
//!   operator reruns this workflow once a deploy lands.
//! - A terminal Stop (every arm) requires a CONFIRMED absence: an
//!   iteration only Stops when its own `git fetch` succeeded (a
//!   failed fetch can leave a fresh head unknown, so its ancestor
//!   check reads false) and its candidate scan was not blind, and
//!   `poll` demands two CONSECUTIVE such iterations, so a sampling
//!   race - a deploy landing between the candidate and pending
//!   listings, a head pushed after the fetch - resolves on the second
//!   pass.  Anything less retries.
//! - An empty same-SHA listing waits. `registry.yml` runs on
//!   every main push and the dispatched SHA is a main push, so an
//!   empty same-SHA listing means the push run is not registered
//!   yet (the seconds after a merge-queue fast-forward, or API lag),
//!   never that nothing needs deploying.  The iteration waits;
//!   past `NO_RUN_PATIENCE` waits a verified empty listing Stops
//!   (a run deleted by retention needs a fresh main push before a
//!   re-dispatch).  The listing is also filtered to `branch=main`
//!   and `event=push`: the merge queue's same-SHA `merge_group` run,
//!   and a fork's pull-request run from its own `main` branch, skip
//!   the deploy job by event and would otherwise read as the
//!   change-gate skip above.
//! - **A failing `gh` never fails the step.** A failed candidate
//!   listing skips the scan, and a failed same-SHA or pending listing
//!   takes its transient branch. A failed deploy-step lookup appends
//!   `transient` to whatever it already printed, so a `gh` that
//!   *prints and then fails* captures `success\ntransient`, which
//!   correctly loses the comparison - hence the append rather than a
//!   replace.
//! - **`merge-base --is-ancestor` answers only true or false.** Exit 1
//!   (not an ancestor) and exit 128 (an unknown revision, a shallow
//!   clone) both fold to false, and its stderr is discarded.
//! - **An empty `runs` capture waits rather than exits.** `jq` prints
//!   nothing for empty input, which is not an integer, so the list
//!   reads as non-empty with a diagnostic, and the conclusion then
//!   reads empty - neither `failure` nor `cancelled` - so the
//!   iteration falls through to the wait.
//! - **A failing conclusion query fails the step.** A `jq` that fails
//!   there - the malformed capture the length check just survived, a
//!   missing binary - exits 1 with only its own diagnostic, where
//!   every other tool failure falls into a branch.
//! - **Timing is fixed**: 90 iterations, and every iteration that does
//!   not exit waits 40 seconds exactly once - every transient/retry
//!   branch, the fall-through, and a provisional Stop alike. The
//!   ceiling is 90 waits and then the timeout.
//! - **`$GITHUB_SHA` and `$GITHUB_REPOSITORY` read as empty when
//!   unset** and are spliced into URLs and arguments as-is.
//!
//! Stated ceilings:
//!
//! - **`jq` and `gh` stay child processes.** The `--jq` projections run
//!   inside `gh`, so no JSON is parsed here; the standalone `jq`
//!   invocations spawn the same way. Reimplementing either in Rust
//!   would be a divergence risk for no gain.
//! - **Captured bytes are read as UTF-8, lossily.** `gh` emits
//!   JSON-derived UTF-8; an invalid byte becomes U+FFFD here, in a run
//!   id or head SHA that could not have matched anything anyway. The
//!   `runs` capture is kept as raw bytes, since it is only piped back
//!   into `jq`.
//! - **A failing conclusion query's exit status collapses to 1.**
//!   `jq`'s own code - parse failures vary by jq version, and a
//!   missing binary has none - is not propagated, and nothing
//!   downstream reads more than pass or fail.
//! - **The context is read once**, at startup, rather than at each
//!   splice: nothing this step spawns can change its own environment.

use std::io::Write as _;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// The loop's ceiling and the wait every continuing path shares.
const ITERATIONS: usize = 90;
const WAIT: Duration = Duration::from_secs(40);

const TRANSIENT_RUNS: &str = "transient API error listing Registry runs; retrying";
const TRANSIENT_PENDING: &str = "transient API error listing pending Registry runs; retrying";
// See the terminal confirmation in `iteration`.
const TRANSIENT_SCAN: &str = "could not confirm that no deploy containing this SHA has landed \
     (a transient fetch or API failure); retrying";
// The empty same-SHA listing's wait, with its own ceiling in
// iterations - see the module doc.
const NO_RUN_PATIENCE: usize = 5;
const NO_RUN_YET: &str = "no Registry run listed for this SHA yet (a main push registers one within \
     seconds); waiting";
const NO_RUN_STOP: &str = "no Registry run for this SHA was listed; not publishing against an \
     unverified worker (every main push registers a run - one deleted by retention needs a fresh \
     main push, then a re-dispatch)";
// The change-gated same-SHA run shape - see `SameShaDeploy::JobSkipped`.
const JOB_SKIPPED: &str = "Registry run for this SHA skipped its registry jobs (no \
     registry-relevant changes); the deployed worker is already current";
const TIMED_OUT: &str = "timed out waiting for a registry deploy containing this SHA";

/// Projections that run inside `gh`.
const CANDIDATES_JQ: &str = r#".workflow_runs[] | "\(.id) \(.head_sha)""#;
const DEPLOY_STEP_JQ: &str = r#"[.jobs[] | select(.name == "deploy-registry") | .steps[] | select(.name == "Deploy") | .conclusion][0]"#;
/// The job's own conclusion and its Deploy step's, in one projection:
/// a skipped job has an EMPTY `steps` array, so the step half alone
/// cannot tell "the gate skipped the job" (`skipped null`) from "the
/// job ran and skipped its Deploy step" (`success skipped`).
const DEPLOY_OUTCOME_JQ: &str = r#"[.jobs[] | select(.name == "deploy-registry") | "\(.conclusion) \([.steps[] | select(.name == "Deploy") | .conclusion][0])"][0]"#;
const RUNS_JQ: &str = ".workflow_runs";
const PENDING_JQ: &str = r#".workflow_runs[] | select(.status != "completed") | .head_sha"#;

/// The blanks a captured line is split and trimmed on. A line never
/// carries `\n`.
const IFS: [char; 2] = [' ', '\t'];

/// Wait for a successful `main` Registry deploy whose head contains
/// `$GITHUB_SHA`, or for one of the answers that ends the wait early.
#[must_use]
pub fn run() -> ExitCode {
    ExitCode::from(poll(
        &mut Spawn,
        &mut Console,
        &context("GITHUB_SHA"),
        &context("GITHUB_REPOSITORY"),
    ))
}

/// An unset - or non-UTF-8 - variable reads as the empty string.
fn context(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

trait Output {
    fn stdout(&mut self, line: &str);
    fn stderr(&mut self, line: &str);
}

struct Console;

impl Output for Console {
    fn stdout(&mut self, line: &str) {
        println!("{line}");
    }

    fn stderr(&mut self, line: &str) {
        eprintln!("{line}");
    }
}

/// What one iteration decided: the line it prints and the stream it
/// prints to.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    /// A stdout line, then another iteration.
    Retry(String),
    /// A stdout line, then exit 0.
    Done(String),
    /// A stderr line, then exit 1.
    Stop(String),
    /// A failed conclusion query: the failing tool already wrote its
    /// own diagnostic, and the step says nothing of its own.
    Die,
}

impl Report {
    /// Writes the line and answers with the exit code, or `None` where
    /// the loop goes round again.
    fn emit(&self, output: &mut dyn Output) -> Option<u8> {
        match self {
            Self::Retry(line) => {
                output.stdout(line);
                None
            }
            Self::Done(line) => {
                output.stdout(line);
                Some(0)
            }
            Self::Stop(line) => {
                output.stderr(line);
                Some(1)
            }
            Self::Die => Some(1),
        }
    }
}

/// The loop, its ceiling and the timeout below it.
///
/// A `Stop` is terminal only when two CONSECUTIVE iterations conclude
/// it.  One iteration samples the world piecewise - fetch, candidate
/// listing, same-SHA run, pending listing - so a deploy that lands
/// between two samplings, or a head pushed after the fetch (unknown
/// revisions fold `merge-base` to false), can read as a confirmed
/// absence.  The second iteration re-takes every sample from its own
/// fresh fetch, so any such race resolves to Done or Retry there; the
/// first Stop just waits, as silently as the fall-through.
fn poll(shell: &mut dyn Shell, output: &mut dyn Output, sha: &str, repository: &str) -> u8 {
    let mut provisional_stop = false;
    for n in 0..ITERATIONS {
        let report = iteration(shell, sha, repository, n);
        if matches!(report, Some(Report::Stop(_))) && !provisional_stop {
            provisional_stop = true;
            shell.wait();
            continue;
        }
        provisional_stop = false;
        if let Some(report) = report
            && let Some(code) = report.emit(output)
        {
            return code;
        }
        shell.wait();
    }
    output.stderr(TIMED_OUT);
    1
}

/// The loop's `n`th pass. `None` is the fall-through, which waits
/// without saying anything.
fn iteration(shell: &mut dyn Shell, sha: &str, repository: &str, n: usize) -> Option<Report> {
    // A failed fetch can leave fresh heads unknown to the local clone,
    // so their `merge-base --is-ancestor` answers false (exit 128
    // folds to false) and a landed deploy is indistinguishable from a
    // missing one.  The terminal confirmation at the bottom
    // refuses to Stop on such an iteration.
    let (fetched, _) = shell.capture(&["git", "fetch", "--quiet", "origin", "main"], &[]);

    let scan_blind = match landed_deploy(shell, sha, repository) {
        Landed::Found(report) => return Some(report),
        Landed::Blind => true,
        Landed::Miss => false,
    };

    let (listed, runs) = shell.capture(
        &[
            "gh",
            "api",
            // `branch=main` like the two listings below: the merge
            // queue fast-forwards main to the merge-group head, so the
            // same SHA also has a `merge_group` run whose deploy job is
            // skipped by event, not by the change gate.
            &runs_url(repository, &format!("head_sha={sha}&branch=main")),
            "--jq",
            RUNS_JQ,
        ],
        &[],
    );
    if !listed {
        return Some(Report::Retry(TRANSIENT_RUNS.to_owned()));
    }
    let runs = crate::substitute(runs);

    // An empty listing is a push run not registered yet, never a
    // deploy that is unnecessary (module doc).  Wait for it; past the
    // patience a Stop needs the same verified absence as the arms
    // below, then `poll`'s second consecutive pass.
    if empty_list(shell, &runs) {
        return Some(if n < NO_RUN_PATIENCE {
            Report::Retry(NO_RUN_YET.to_owned())
        } else if !fetched || scan_blind {
            Report::Retry(TRANSIENT_SCAN.to_owned())
        } else {
            Report::Stop(NO_RUN_STOP.to_owned())
        });
    }
    // The one tool failure that fails the step rather than falling
    // into a branch, the malformed capture the length check survived
    // included. jq's own diagnostic passes through on the inherited
    // stderr.
    let (ran, conclusion) = shell.capture(&["jq", "-r", ".[0].conclusion"], &runs);
    if !ran {
        return Some(Report::Die);
    }
    let conclusion = text(conclusion);
    // A same-SHA run that succeeded without deploying exits instead of
    // idling the publish job into its hour ceiling: a job skip exits 0
    // at once, and a Deploy-step skip takes the failure path's
    // pending-descendant exit (`SameShaDeploy`, module doc).  A query
    // failure keeps the safe answer (wait).
    let same_sha = if conclusion == "success" {
        same_sha_deploy(shell, repository, &runs)
    } else {
        SameShaDeploy::Other
    };
    if same_sha == SameShaDeploy::JobSkipped {
        return Some(Report::Done(JOB_SKIPPED.to_owned()));
    }
    let deploy_skipped = same_sha == SameShaDeploy::StepSkipped;
    if !deploy_skipped && conclusion != "failure" && conclusion != "cancelled" {
        return None;
    }

    // Terminal only when nothing pending could still satisfy the
    // ancestor gate: a queued or in-progress run whose head contains
    // this SHA (a rerun, or main already carrying the fix) delivers the
    // deploy on a later iteration, while failing here would strand this
    // run - publishing is dispatch-only, so no push delivers another.
    let (listed, pending) = shell.capture(
        &[
            "gh",
            "api",
            &runs_url(repository, "branch=main&per_page=100"),
            "--jq",
            PENDING_JQ,
        ],
        &[],
    );
    if !listed {
        return Some(Report::Retry(TRANSIENT_PENDING.to_owned()));
    }

    let pending = text(pending);
    let descendant_pending = pending.split('\n').any(|line| {
        let head = line.trim_matches(IFS);
        !head.is_empty() && shell.condition(&["git", "merge-base", "--is-ancestor", sha, head])
    });
    if descendant_pending {
        return Some(Report::Retry(if deploy_skipped {
            "Registry run for this SHA succeeded but skipped its Deploy; a pending run containing it may still deploy; waiting".to_owned()
        } else {
            format!(
                "Registry run for this SHA concluded '{conclusion}'; a pending run containing it may still deploy; waiting"
            )
        }));
    }
    // Never Stop - even provisionally (see `poll`) - from an
    // iteration that could not VERIFY absence: a failed fetch folds
    // every ancestor check to false, and a blind scan may have missed
    // a landed deploy outright.
    if !fetched || scan_blind {
        return Some(Report::Retry(TRANSIENT_SCAN.to_owned()));
    }
    Some(Report::Stop(if deploy_skipped {
        "Registry run for this SHA succeeded but skipped its Deploy (superseded by a newer \
         registry-relevant commit) and no pending run can deliver a deploy containing it; not \
         publishing against a possibly-stale worker - rerun this workflow once the deploy lands \
         (registry/docs/runbook.md)"
            .to_owned()
    } else {
        format!(
            "Registry run for this SHA concluded '{conclusion}' and no deploy containing it has landed; not publishing against a stale worker"
        )
    }))
}

/// What one candidate listing + scan pass concluded.
enum Landed {
    /// A deploy containing the SHA: the wait is over.
    Found(Report),
    /// The listing and every ancestor-gated lookup ran; nothing
    /// qualified.
    Miss,
    /// The listing or an ancestor-gated lookup failed to run: the
    /// absence of a landed deploy is unverified.
    Blind,
}

/// List the successful main runs and scan them for a deploy
/// containing this SHA.
fn landed_deploy(shell: &mut dyn Shell, sha: &str, repository: &str) -> Landed {
    // per_page=100: successive skipped-Deploy successes (a burst of
    // superseded runs) must not push the qualifying run out of the
    // scanned window - a buried qualifying candidate reads as a clean
    // miss, which a confirmed Stop would trust and fast-fail on.
    let (listed, candidates) = shell.capture(
        &[
            "gh",
            "api",
            &runs_url(repository, "branch=main&status=success&per_page=100"),
            "--jq",
            CANDIDATES_JQ,
        ],
        &[],
    );
    if !listed {
        return Landed::Blind;
    }
    let mut blind = false;
    if let Some(report) = scan(shell, sha, repository, &text(candidates), &mut blind) {
        return Landed::Found(report);
    }
    if blind { Landed::Blind } else { Landed::Miss }
}

/// How the same-SHA Registry run left its `deploy-registry` job.
#[derive(Debug, PartialEq, Eq)]
enum SameShaDeploy {
    /// The job itself concluded `skipped`: the workflow's change gate
    /// answered false to its `if`, so this push can never deploy and
    /// the deployed Worker is already current.
    JobSkipped,
    /// The job ran but its Deploy step concluded `skipped`: a newer
    /// registry-relevant commit superseded it.
    StepSkipped,
    /// Anything else, a failed query included: wait.
    Other,
}

/// Reads the run id from the same-SHA listing and asks the jobs API
/// for the `deploy-registry` job's conclusion paired with its Deploy
/// step's ([`DEPLOY_OUTCOME_JQ`]).
fn same_sha_deploy(shell: &mut dyn Shell, repository: &str, runs: &[u8]) -> SameShaDeploy {
    let (ran, id) = shell.capture(&["jq", "-r", ".[0].id"], runs);
    if !ran {
        return SameShaDeploy::Other;
    }
    let id = text(id);
    let id = id.trim_matches(IFS).trim_matches('\n');
    if id.is_empty() || id == "null" {
        return SameShaDeploy::Other;
    }
    let (ran, outcome) = shell.capture(
        &[
            "gh",
            "api",
            &jobs_url(repository, id),
            "--jq",
            DEPLOY_OUTCOME_JQ,
        ],
        &[],
    );
    if !ran {
        return SameShaDeploy::Other;
    }
    // A skipped job has an empty steps array, so its step half is
    // jq's interpolated `null` (probed against the live API,
    // 2026-08-19: run 32216610154 answers exactly `skipped null`).
    match crate::substitute(outcome).as_slice() {
        b"skipped null" => SameShaDeploy::JobSkipped,
        b"success skipped" => SameShaDeploy::StepSkipped,
        _ => SameShaDeploy::Other,
    }
}

/// The successful runs, in the order the API listed them, until one
/// whose head contains this SHA carries a Deploy step that ran and
/// succeeded.  A candidate lookup that failed to run sets
/// `blind` - the caller must not conclude a terminal Stop from an
/// iteration that may have missed a landed deploy.  A lookup that ran
/// and answered non-success is a clean miss, not blindness.
fn scan(
    shell: &mut dyn Shell,
    sha: &str,
    repository: &str,
    candidates: &str,
    blind: &mut bool,
) -> Option<Report> {
    for line in candidates.split('\n') {
        let (run_id, head) = read_two(line);
        if run_id.is_empty() {
            continue;
        }
        if !shell.condition(&["git", "merge-base", "--is-ancestor", sha, head]) {
            continue;
        }
        // `transient` is appended to whatever the call already
        // printed, so a partial answer can never read as `success`.
        let (ran, mut conclusion) = shell.capture(
            &[
                "gh",
                "api",
                &jobs_url(repository, run_id),
                "--jq",
                DEPLOY_STEP_JQ,
            ],
            &[],
        );
        if !ran {
            conclusion.extend_from_slice(b"transient\n");
            *blind = true;
        }
        if crate::substitute(conclusion) == b"success".as_slice() {
            return Some(Report::Done(format!(
                "registry deploy {run_id} (head {head}) contains this SHA"
            )));
        }
    }
    None
}

/// `jq` prints nothing for empty input, which is diagnosed as a
/// non-integer and answers false - the iteration then reads an empty
/// conclusion and waits.
fn empty_list(shell: &mut dyn Shell, runs: &[u8]) -> bool {
    let length = jq(shell, runs, &["jq", "length"]);
    let Ok(count) = length.trim().parse::<i64>() else {
        eprintln!("`jq length` gave `{length}`, not an integer; the run list reads as non-empty");
        return false;
    };
    count == 0
}

/// `runs` piped through `jq`, captured. Nothing reads `jq`'s status,
/// so only the text comes back.
fn jq(shell: &mut dyn Shell, runs: &[u8], argv: &[&str]) -> String {
    let (_, output) = shell.capture(argv, runs);
    text(output)
}

/// A capture as it is compared: `substitute`'s byte rules, then the
/// lossy decode the module documents as a ceiling.
fn text(captured: Vec<u8>) -> String {
    String::from_utf8_lossy(&crate::substitute(captured)).into_owned()
}

/// Every listing is limited to `push` runs, the only event that runs
/// `deploy-registry`: `branch` matches a run's `head_branch`, which a
/// fork's pull request from its own `main` also carries, and such a
/// run - skipped deploy job, or pending forever - would otherwise
/// answer for this SHA.
fn runs_url(repository: &str, query: &str) -> String {
    format!("repos/{repository}/actions/workflows/registry.yml/runs?event=push&{query}")
}

fn jobs_url(repository: &str, run_id: &str) -> String {
    format!("repos/{repository}/actions/runs/{run_id}/jobs?per_page=100")
}

/// Leading and trailing blanks dropped, the first field split off at
/// a run of blanks, and every remaining field left in the second - a
/// line carrying extras keeps them all there.
fn read_two(line: &str) -> (&str, &str) {
    let line = line.trim_matches(IFS);
    match line.split_once(IFS) {
        Some((first, rest)) => (first, rest.trim_start_matches(IFS)),
        None => (line, ""),
    }
}

/// Everything the loop spawns or waits on. The tests script it, which
/// is what lets the 90-iteration ceiling be checked without waiting an
/// hour for it.
trait Shell {
    /// `argv` with `stdin` on the standard input: stdout captured raw,
    /// stderr inherited, and whether it succeeded.
    fn capture(&mut self, argv: &[&str], stdin: &[u8]) -> (bool, Vec<u8>);
    /// `argv` with stderr discarded: the status alone.
    fn condition(&mut self, argv: &[&str]) -> bool;
    /// One `WAIT`.
    fn wait(&mut self);
}

/// The production [`Shell`]: real child processes, found on `PATH`.
struct Spawn;

impl Shell for Spawn {
    fn capture(&mut self, argv: &[&str], stdin: &[u8]) -> (bool, Vec<u8>) {
        let mut child = match Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            // Every caller here reads a spawn failure as a failed call.
            Err(error) => {
                eprintln!("{}: {error}", argv[0]);
                return (false, Vec::new());
            }
        };
        if let Some(mut pipe) = child.stdin.take() {
            // A short write against an early-exiting `jq` is ignored.
            let _ = pipe.write_all(stdin);
        }
        match child.wait_with_output() {
            Ok(output) => (output.status.success(), output.stdout),
            Err(error) => {
                eprintln!("{}: {error}", argv[0]);
                (false, Vec::new())
            }
        }
    }

    fn condition(&mut self, argv: &[&str]) -> bool {
        Command::new(argv[0])
            .args(&argv[1..])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn wait(&mut self) {
        std::thread::sleep(WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted [`Shell`]: each capture is answered by the first
    /// entry whose needle appears in the joined argv - consulting the
    /// consumed-on-use `answers_once` entries first, for tests where
    /// the API's answer changes between two samplings - and
    /// `merge-base` answers true for the heads named in `descendants`.
    #[derive(Default)]
    struct Fake {
        answers_once: Vec<(&'static str, bool, &'static str)>,
        answers: Vec<(&'static str, bool, &'static str)>,
        descendants: Vec<&'static str>,
        calls: Vec<String>,
        waits: usize,
    }

    impl Fake {
        fn answering(answers: &[(&'static str, bool, &'static str)]) -> Self {
            Self {
                answers: answers.to_vec(),
                ..Self::default()
            }
        }

        fn answering_once(mut self, answers: &[(&'static str, bool, &'static str)]) -> Self {
            self.answers_once = answers.to_vec();
            self
        }

        fn descending_from(mut self, heads: &[&'static str]) -> Self {
            self.descendants = heads.to_vec();
            self
        }

        fn called(&self, needle: &str) -> usize {
            self.calls
                .iter()
                .filter(|call| call.contains(needle))
                .count()
        }
    }

    impl Shell for Fake {
        fn capture(&mut self, argv: &[&str], _stdin: &[u8]) -> (bool, Vec<u8>) {
            let joined = argv.join(" ");
            self.calls.push(joined.clone());
            if let Some(position) = self
                .answers_once
                .iter()
                .position(|(needle, _, _)| joined.contains(needle))
            {
                let (_, ran, output) = self.answers_once.remove(position);
                return (ran, output.as_bytes().to_vec());
            }
            for (needle, ran, output) in &self.answers {
                if joined.contains(needle) {
                    return (*ran, output.as_bytes().to_vec());
                }
            }
            (false, Vec::new())
        }

        fn condition(&mut self, argv: &[&str]) -> bool {
            self.calls.push(argv.join(" "));
            let head = argv.last().copied().unwrap_or_default();
            self.descendants.contains(&head)
        }

        fn wait(&mut self) {
            self.waits += 1;
        }
    }

    #[derive(Default)]
    struct FakeOutput {
        stdout: Vec<String>,
        stderr: Vec<String>,
    }

    impl Output for FakeOutput {
        fn stdout(&mut self, line: &str) {
            self.stdout.push(line.to_owned());
        }

        fn stderr(&mut self, line: &str) {
            self.stderr.push(line.to_owned());
        }
    }

    /// The candidate listing answering with one qualifying run.
    const ONE_CANDIDATE: (&str, bool, &str) = ("status=success", true, "77 head77\n");
    /// The iteration's `git fetch` succeeding - a terminal Stop
    /// requires it.
    const FETCH_OK: (&str, bool, &str) = ("git fetch", true, "");

    #[test]
    fn reports_pick_their_stream_and_exit_code() {
        let mut output = FakeOutput::default();
        assert_eq!(Report::Retry("retry".to_owned()).emit(&mut output), None);
        assert_eq!(Report::Done("done".to_owned()).emit(&mut output), Some(0));
        assert_eq!(Report::Stop("stop".to_owned()).emit(&mut output), Some(1));
        assert_eq!(Report::Die.emit(&mut output), Some(1));
        assert_eq!(output.stdout, ["retry", "done"]);
        assert_eq!(output.stderr, ["stop"]);
    }

    #[test]
    fn the_urls_and_projections_are_spelled_exactly() {
        assert_eq!(
            runs_url("o/r", "head_sha=abc&branch=main"),
            "repos/o/r/actions/workflows/registry.yml/runs?event=push&head_sha=abc&branch=main"
        );
        assert_eq!(
            jobs_url("o/r", "77"),
            "repos/o/r/actions/runs/77/jobs?per_page=100"
        );
        assert_eq!(
            CANDIDATES_JQ,
            ".workflow_runs[] | \"\\(.id) \\(.head_sha)\""
        );
        // An unset context reads as empty and is spliced in as-is.
        assert_eq!(
            runs_url("", "head_sha="),
            "repos//actions/workflows/registry.yml/runs?event=push&head_sha="
        );
    }

    #[test]
    fn a_deploy_containing_the_sha_ends_the_wait() {
        let mut shell = Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", true, "success\n")])
            .descending_from(&["head77"]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Done(
                "registry deploy 77 (head head77) contains this SHA".to_owned()
            ))
        );
    }

    #[test]
    fn a_deploy_step_that_prints_then_fails_is_not_a_success() {
        // The appended `transient` makes the capture
        // `success\ntransient`, which loses the comparison.
        let mut shell =
            Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", false, "success\n")])
                .descending_from(&["head77"]);
        // Falls through to the same-SHA listing, which this fake fails.
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_RUNS.to_owned()))
        );
    }

    #[test]
    fn a_candidate_that_is_not_an_ancestor_is_skipped() {
        let mut shell = Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", true, "success\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_RUNS.to_owned()))
        );
        assert_eq!(shell.called("jobs?per_page=100"), 0);
    }

    #[test]
    fn an_empty_candidate_list_runs_one_field_less_iteration() {
        // An empty capture still yields one empty line; the empty
        // run-id guard is what makes it a no-op.
        let mut shell = Fake::answering(&[("status=success", true, "")]);
        iteration(&mut shell, "abc", "o/r", 0);
        assert_eq!(shell.called("merge-base"), 0);
    }

    #[test]
    fn a_failed_candidate_listing_skips_the_scan_without_a_word() {
        let mut shell = Fake::answering(&[("status=success", false, "77 head77\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_RUNS.to_owned()))
        );
        assert_eq!(shell.called("merge-base"), 0);
    }

    #[test]
    fn read_folds_extra_fields_into_the_head() {
        assert_eq!(read_two("77 head77"), ("77", "head77"));
        assert_eq!(read_two("77   head77  tail"), ("77", "head77  tail"));
        assert_eq!(read_two("  77\thead77  "), ("77", "head77"));
        assert_eq!(read_two("77"), ("77", ""));
        assert_eq!(read_two(""), ("", ""));
    }

    /// A main push always registers a Registry run, so an empty
    /// listing is a run not yet registered.
    #[test]
    fn no_registry_run_for_this_sha_waits() {
        let mut shell = Fake::answering(&[("head_sha=", true, "[]"), ("jq length", true, "0\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(NO_RUN_YET.to_owned()))
        );
    }

    /// The registration gap after a merge-queue fast-forward: the
    /// same-SHA listing is empty on the first pass (the `merge_group`
    /// run is filtered out, the push run not yet registered), then
    /// shows the push run in progress, then a deploy containing the
    /// SHA lands.  The wait ends on that deploy, not on the gap.
    #[test]
    fn the_registration_gap_waits_for_the_push_run_and_its_deploy() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, "77 head77\n"),
            ("runs/77/jobs", true, "success\n"),
            ("head_sha=abc&branch=main", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "null"),
        ])
        .answering_once(&[
            ("status=success", true, ""),
            ("head_sha=abc&branch=main", true, "[]"),
            ("jq length", true, "0"),
            ("status=success", true, ""),
        ])
        .descending_from(&["head77"]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 0);
        assert_eq!(shell.waits, 2);
        assert_eq!(
            output.stdout,
            [
                NO_RUN_YET,
                "registry deploy 77 (head head77) contains this SHA"
            ]
        );
    }

    /// A listing that stays empty past the patience fails closed in
    /// minutes rather than idling into the hour ceiling.
    #[test]
    fn a_registry_run_still_missing_past_the_patience_stops() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, ""),
            ("head_sha=abc&branch=main", true, "[]"),
            ("jq length", true, "0"),
        ]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 1);
        // The patience, then the provisional Stop's confirmation wait.
        assert_eq!(shell.waits, NO_RUN_PATIENCE + 1);
        assert_eq!(output.stdout, vec![NO_RUN_YET; NO_RUN_PATIENCE]);
        assert_eq!(output.stderr, [NO_RUN_STOP]);
    }

    /// Past the patience, an iteration that could not verify absence
    /// defers like every other Stop.
    #[test]
    fn a_blind_scan_defers_the_missing_run_stop() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", false, ""),
            ("head_sha=abc&branch=main", true, "[]"),
            ("jq length", true, "0"),
        ]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", NO_RUN_PATIENCE),
            Some(Report::Retry(TRANSIENT_SCAN.to_owned()))
        );
    }

    #[test]
    fn an_unparsable_length_reads_as_non_empty_and_waits() {
        // `jq` prints nothing for empty input, which is not an
        // integer, and the conclusion then reads empty, so the
        // iteration falls through to the wait.
        let mut shell = Fake::answering(&[("head_sha=", true, ""), ("jq ", true, "")]);
        assert_eq!(iteration(&mut shell, "abc", "o/r", 0), None);
    }

    #[test]
    fn a_failing_conclusion_query_kills_the_step() {
        // The one tool failure that fails the step: the same
        // malformed capture the length check just survived.
        let mut shell = Fake::answering(&[
            ("head_sha=", true, "not json"),
            ("jq length", true, "1"),
            ("jq -r", false, ""),
        ]);
        assert_eq!(iteration(&mut shell, "abc", "o/r", 0), Some(Report::Die));
    }

    #[test]
    fn a_conclusion_that_is_neither_failure_nor_cancelled_waits() {
        // The conclusion and id needles are spelled out: a bare
        // "jq -r" would also answer the skip predicate's id query
        // with the conclusion text, and the `success` case would then
        // pass for the wrong reason (a failed jobs lookup).
        for conclusion in ["null", "success", "", "skipped"] {
            let mut shell = Fake::answering(&[
                ("head_sha=", true, "[{}]"),
                ("jq length", true, "1"),
                ("jq -r .[0].conclusion", true, conclusion),
                ("jq -r .[0].id", true, "null"),
            ]);
            assert_eq!(iteration(&mut shell, "abc", "o/r", 0), None, "{conclusion}");
        }
    }

    #[test]
    fn a_failed_run_waits_while_a_pending_descendant_could_still_deploy() {
        for conclusion in ["failure", "cancelled"] {
            let mut shell = Fake::answering(&[
                ("head_sha=", true, "[{}]"),
                ("jq length", true, "1"),
                ("jq -r", true, conclusion),
                ("branch=main&per_page=100", true, "stale\npending77\n"),
            ])
            .descending_from(&["pending77"]);
            assert_eq!(
                iteration(&mut shell, "abc", "o/r", 0),
                Some(Report::Retry(format!(
                    "Registry run for this SHA concluded '{conclusion}'; a pending run containing it may still deploy; waiting"
                )))
            );
        }
    }

    #[test]
    fn a_failed_run_with_no_pending_descendant_stops() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r", true, "failure"),
            ("branch=main&per_page=100", true, "stale\n"),
        ]);
        let stale = Report::Stop(
            "Registry run for this SHA concluded 'failure' and no deploy containing it has landed; not publishing against a stale worker"
                .to_owned(),
        );
        assert_eq!(iteration(&mut shell, "abc", "o/r", 0), Some(stale));
    }

    /// The superseded fast-fail: a same-SHA run that SUCCEEDED with
    /// its Deploy step skipped can never green-light this SHA,
    /// and with nothing pending the wait would only idle into its
    /// hour ceiling - stop immediately, with the reason.
    #[test]
    fn a_skipped_deploy_with_no_pending_descendant_stops_fast() {
        // The candidate scan and the outcome query both ask about run
        // 88, so the needles key on each projection's unique jq text.
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, "88 head88\n"),
            (r#"deploy-registry") | .steps"#, true, "skipped\n"),
            (r"\(.conclusion)", true, "success skipped\n"),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("branch=main&per_page=100", true, "stale\n"),
        ])
        .descending_from(&["head88"]);
        match iteration(&mut shell, "abc", "o/r", 0) {
            Some(Report::Stop(line)) => {
                assert!(line.contains("skipped its Deploy"), "{line}");
                assert!(line.contains("not publishing"), "{line}");
            }
            other => panic!("expected a fast stop, got {other:?}"),
        }
    }

    /// Same skip, but a pending run containing this SHA may still
    /// deliver the deploy: keep waiting.
    #[test]
    fn a_skipped_deploy_with_a_pending_descendant_waits() {
        let mut shell = Fake::answering(&[
            ("status=success", true, "88 head88\n"),
            (r#"deploy-registry") | .steps"#, true, "skipped\n"),
            (r"\(.conclusion)", true, "success skipped\n"),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("branch=main&per_page=100", true, "pending99\n"),
        ])
        .descending_from(&["head88", "pending99"]);
        match iteration(&mut shell, "abc", "o/r", 0) {
            Some(Report::Retry(line)) => {
                assert!(line.contains("skipped its Deploy"), "{line}");
                assert!(line.contains("waiting"), "{line}");
            }
            other => panic!("expected a waiting retry, got {other:?}"),
        }
    }

    /// A transient failure inside the scan hides whether a landed
    /// deploy already contains this SHA, so a terminal answer from
    /// the same iteration is unverified: retry instead of stopping.
    #[test]
    fn a_blind_scan_defers_the_skipped_stop() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, "77 head77\n"),
            ("runs/77/jobs", false, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("runs/88/jobs", true, "success skipped\n"),
            ("branch=main&per_page=100", true, "stale\n"),
        ])
        .descending_from(&["head77", "head88"]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_SCAN.to_owned()))
        );
    }

    /// The same guard covers the failure arm: a failed candidate
    /// listing is a blind scan too.
    #[test]
    fn a_blind_candidate_listing_defers_the_failure_stop() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", false, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r", true, "failure"),
            ("branch=main&per_page=100", true, "stale\n"),
        ]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_SCAN.to_owned()))
        );
    }

    /// A failed `git fetch` leaves superseding heads unknown, so every
    /// ancestor check answers false and a landed deploy is invisible
    /// without any API call failing: never a Stop.
    #[test]
    fn a_failed_fetch_defers_the_stop() {
        let mut shell = Fake::answering(&[
            ("status=success", true, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("runs/88/jobs", true, "success skipped\n"),
            ("branch=main&per_page=100", true, "stale\n"),
        ]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_SCAN.to_owned()))
        );
    }

    /// The negative arm of the outcome predicate: only `success
    /// skipped` is the step-skip and only `skipped null` is the
    /// job-skip.  A Deploy that ran (`success success`, on a listing
    /// that lagged behind the candidate scan), an absent step
    /// (`success null`), an empty answer, or any other pairing falls
    /// through to the wait.
    #[test]
    fn a_success_run_whose_deploy_was_not_skipped_waits() {
        for outcome in [
            "success success\n",
            "success null\n",
            "",
            "success failure\n",
        ] {
            let mut shell = Fake::answering(&[
                FETCH_OK,
                ("status=success", true, ""),
                ("head_sha=", true, "[{}]"),
                ("jq length", true, "1"),
                ("jq -r .[0].conclusion", true, "success"),
                ("jq -r .[0].id", true, "88"),
                ("runs/88/jobs", true, outcome),
            ]);
            assert_eq!(iteration(&mut shell, "abc", "o/r", 0), None, "{outcome:?}");
        }
    }

    /// The change-gated shape: the same-SHA run succeeded with its
    /// `deploy-registry` job itself skipped - an empty steps array,
    /// which the outcome projection reads as `skipped null` (probed
    /// against the live jobs API) - so no deploy can ever come from
    /// this push and the Worker is already current.  Immediate: the
    /// change gate's answer is a deterministic
    /// property of the commit, so there is no sampling race for a
    /// confirmation pass to resolve.
    #[test]
    fn a_change_gated_job_skip_exits_zero_without_waiting() {
        let answers = [
            FETCH_OK,
            ("status=success", true, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("runs/88/jobs", true, "skipped null\n"),
        ];
        let mut shell = Fake::answering(&answers);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Done(JOB_SKIPPED.to_owned()))
        );
        // Only a main-branch run may answer: the same SHA's
        // `merge_group` run skips the deploy job by event.
        assert_eq!(shell.called("head_sha=abc&branch=main"), 1);
        assert_eq!(shell.called("branch=main&per_page=100"), 0);
        let mut shell = Fake::answering(&answers);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 0);
        assert_eq!(shell.waits, 0);
    }

    /// A deploy-step query failure keeps the safe answer: wait.
    #[test]
    fn a_failed_deploy_step_query_on_a_success_run_waits() {
        let mut shell = Fake::answering(&[
            (
                "status=success",
                true,
                "88 head88
",
            ),
            ("jobs?per_page=100", false, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
        ])
        .descending_from(&["head88"]);
        assert_eq!(iteration(&mut shell, "abc", "o/r", 0), None);
    }

    #[test]
    fn a_failed_pending_listing_retries() {
        let mut shell = Fake::answering(&[
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r", true, "failure"),
            ("branch=main&per_page=100", false, ""),
        ]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r", 0),
            Some(Report::Retry(TRANSIENT_PENDING.to_owned()))
        );
    }

    #[test]
    fn a_stop_needs_two_consecutive_iterations() {
        // The superseded shape, stable across iterations: the first Stop
        // is provisional, the second - after a fresh fetch and fresh
        // listings - is terminal.
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, ""),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("runs/88/jobs", true, "success skipped\n"),
            ("branch=main&per_page=100", true, "stale\n"),
        ]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 1);
        assert_eq!(shell.waits, 1);
        assert_eq!(shell.called("git fetch"), 2);
    }

    /// A run pushed or completed during the confirmation wait is
    /// visible to the second iteration's fresh fetch and listings -
    /// the sampling races (a deploy landing between the candidate and
    /// pending listings, a head unknown to the pre-fetch clone) all
    /// resolve here instead of in a terminal Stop.
    #[test]
    fn a_deploy_landing_during_the_confirmation_wait_wins() {
        let mut shell = Fake::answering(&[
            FETCH_OK,
            ("status=success", true, "99 head99\n"),
            ("runs/99/jobs", true, "success\n"),
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r .[0].conclusion", true, "success"),
            ("jq -r .[0].id", true, "88"),
            ("runs/88/jobs", true, "success skipped\n"),
            ("branch=main&per_page=100", true, "stale\n"),
        ])
        .answering_once(&[("status=success", true, "")])
        .descending_from(&["head99"]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 0);
        assert_eq!(shell.waits, 1);
    }

    #[test]
    fn the_ceiling_is_ninety_waits_and_then_the_timeout() {
        // Every call fails, so each iteration takes the transient
        // branch: the loop reaches its ceiling and exits 1.
        let mut shell = Fake::default();
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 1);
        assert_eq!(shell.waits, ITERATIONS);
        assert_eq!(shell.called("git fetch"), ITERATIONS);
        assert_eq!(output.stdout, vec![TRANSIENT_RUNS; ITERATIONS]);
        assert_eq!(output.stderr, [TIMED_OUT]);
    }

    #[test]
    fn an_early_answer_stops_the_loop_without_waiting() {
        let mut shell = Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", true, "success\n")])
            .descending_from(&["head77"]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 0);
        assert_eq!(shell.waits, 0);
    }

    #[test]
    fn a_waiting_answer_costs_exactly_one_wait_per_iteration() {
        let mut shell = Fake::answering(&[
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r", true, "failure"),
            ("branch=main&per_page=100", true, "pending77"),
        ])
        .descending_from(&["pending77"]);
        let mut output = FakeOutput::default();
        assert_eq!(poll(&mut shell, &mut output, "abc", "o/r"), 1);
        assert_eq!(shell.waits, ITERATIONS);
    }
}
