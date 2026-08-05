//! The registry-deploy wait, ported one-to-one from the `run:` body of
//! the "Wait for a registry deploy containing this SHA" step of
//! `.github/workflows/ports-publish.yml`.
//!
//! ```text
//! L1  deploy_step() {
//! L2    gh api "repos/$GITHUB_REPOSITORY/actions/runs/$1/jobs?per_page=100" \
//! L3      --jq '[.jobs[] | select(.name == "deploy-registry") | .steps[] |
//! L3        select(.name == "Deploy") | .conclusion][0]'
//! L4  }
//! L5  for _ in $(seq 1 90); do
//! L6    git fetch --quiet origin main || true
//! L7    if candidates=$(gh api \
//! L8      "<runs>?branch=main&status=success&per_page=100" \
//! L9      --jq '.workflow_runs[] | "\(.id) \(.head_sha)"'); then
//! L10     while read -r rid rsha; do
//! L11       [ -n "$rid" ] || continue
//! L12       git merge-base --is-ancestor "$GITHUB_SHA" "$rsha" 2>/dev/null || continue
//! L13       if [ "$(deploy_step "$rid" || echo transient)" = "success" ]; then
//! L14         echo "registry deploy $rid (head $rsha) contains this SHA"
//! L15         exit 0
//! L16       fi
//! L17     done <<< "$candidates"
//! L18   fi
//! L19   if ! runs=$(gh api \
//! L20     "<runs>?head_sha=$GITHUB_SHA" --jq '.workflow_runs'); then
//! L21     echo "transient API error listing Registry runs; retrying"
//! L22     sleep 40
//! L23     continue
//! L24   fi
//! L25   if [ "$(printf '%s' "$runs" | jq 'length')" -eq 0 ]; then
//! L26     echo "no Registry run for this SHA; the deployed worker is already current"
//! L27     exit 0
//! L28   fi
//! L29   conclusion=$(printf '%s' "$runs" | jq -r '.[0].conclusion')
//! L30   if [ "$conclusion" = "failure" ] || [ "$conclusion" = "cancelled" ]; then
//! L31     if ! pending=$(gh api \
//! L32       "<runs>?branch=main&per_page=100" \
//! L33       --jq '.workflow_runs[] | select(.status != "completed") | .head_sha'); then
//! L34       echo "transient API error listing pending Registry runs; retrying"
//! L35       sleep 40
//! L36       continue
//! L37     fi
//! L38     descendant_pending=false
//! L39     while read -r psha; do
//! L40       [ -n "$psha" ] || continue
//! L41       if git merge-base --is-ancestor "$GITHUB_SHA" "$psha" 2>/dev/null; then
//! L42         descendant_pending=true
//! L43         break
//! L44       fi
//! L45     done <<< "$pending"
//! L46     if [ "$descendant_pending" = true ]; then
//! L47       echo "Registry run ... may still deploy; waiting"
//! L48       sleep 40
//! L49       continue
//! L50     fi
//! L51     echo "Registry run ... against a stale worker" >&2
//! L52     exit 1
//! L53   fi
//! L54   sleep 40
//! L55 done
//! L56 echo "timed out waiting for a registry deploy containing this SHA" >&2
//! L57 exit 1
//! ```
//!
//! `<runs>` abbreviates
//! `repos/$GITHUB_REPOSITORY/actions/workflows/registry.yml/runs`, and
//! the two elided messages are spelled out where the port echoes them.
//! The block's own comments moved to the code they explain; the step's
//! leading comment, which is outside the `run:` body, stays in the
//! workflow.
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior. Each was pinned by running the original
//! under `bash -e`, GitHub's default `run:` shell (`-e` on, `-u` and
//! `-o pipefail` off):
//!
//! - **Every stdout line is the workflow's log contract**, byte for
//!   byte, `echo`'s newline included. The two `>&2` lines are the only
//!   ones the step reads as a failure.
//! - **A failing `gh` never fails the step.** L7 and L19/L31 put the
//!   substitution in a condition, where `set -e` is suppressed: L7
//!   takes the else path with an empty capture, L19/L31 take their
//!   transient branch. L13 folds its own failure into the captured text
//!   through `|| echo transient`, so a `gh` that *prints and then
//!   fails* captures `success\ntransient`, which correctly loses the
//!   comparison - hence the append rather than a replace.
//! - **Fields come from `read`.** The herestring appends a newline, so
//!   an empty capture still runs one iteration with empty fields; the
//!   `[ -n "$rid" ]` guard, not the loop, is what makes an empty list a
//!   no-op. `read -r rid rsha` folds every extra field into `rsha`.
//! - **`merge-base --is-ancestor` answers only true or false.** Exit 1
//!   (not an ancestor) and exit 128 (an unknown revision, a shallow
//!   clone) both fold to false in a condition, and its stderr was
//!   discarded.
//! - **An empty `runs` capture retries rather than exits.** `jq` prints
//!   nothing for empty input, `[ "" -eq 0 ]` writes an
//!   `integer expected` diagnostic and answers false, and the
//!   conclusion then reads empty - neither `failure` nor `cancelled` -
//!   so the iteration falls through to L54. A `[]` list conversely
//!   reads `0` and exits 0, and `jq -r '.[0].conclusion'` over `[]`
//!   prints `null`, which retries.
//! - **L29 is the one substitution outside any condition**, so a `jq`
//!   that fails there - the malformed capture L25 just survived, a
//!   missing binary - killed the step under `set -e` where every other
//!   tool failure fell into a branch.
//! - **Timing is fixed**: 90 iterations, and every path that continues
//!   waits 40 seconds exactly once - the three transient branches and
//!   the fall-through alike - so the wait is hoisted to the loop's tail
//!   here instead of being repeated at each `continue`. The ceiling is
//!   90 waits and then L56.
//! - **`$GITHUB_SHA` and `$GITHUB_REPOSITORY` read as empty when
//!   unset** (`-u` is off) and are spliced into URLs and arguments
//!   as-is.
//!
//! Stated ceilings:
//!
//! - **`jq` and `gh` stay child processes.** The `--jq` projections run
//!   inside `gh` and this port passes the same argv, so no JSON is
//!   parsed here; the two standalone `jq` invocations spawn the same
//!   way. Reimplementing either in Rust would be a divergence risk for
//!   no gain.
//! - **Diagnostic wording.** Where the original's own stderr came from
//!   `bash` - the `integer expected` complaint, a `command not found`
//!   for a missing `gh`, `jq` or `git` - this port writes its own
//!   wording. The control flow is identical in each case; the workflow
//!   reads the step's status, and its log carries the `echo`'d lines
//!   unchanged.
//! - **Captured bytes are read as UTF-8, lossily.** `gh` emits
//!   JSON-derived UTF-8; a byte the shell would have passed through
//!   becomes U+FFFD here, in a run id or head SHA that could not have
//!   matched anything anyway. The `runs` capture is kept as raw bytes,
//!   since it is only piped back into `jq`.
//! - **`[ x -eq 0 ]` narrows to a decimal parse.** `bash` evaluates
//!   both sides as arithmetic, which also accepts `0x10`, `1+1` and
//!   variable names; `jq 'length'` emits a plain decimal count or
//!   nothing at all.
//! - **L29's exit status collapses to 1.** The shell died with `jq`'s
//!   own code - parse failures vary by jq version, a missing binary is
//!   127 - and nothing downstream reads more than pass or fail.
//! - **The context is read once**, at startup, rather than at each
//!   splice: nothing this step spawns can change its own environment.

use std::io::Write as _;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// L5 and the `sleep 40` every continuing path shares.
const ITERATIONS: usize = 90;
const WAIT: Duration = Duration::from_secs(40);

/// L21, L34, L26 and L56, byte for byte; `echo` adds the newline.
const TRANSIENT_RUNS: &str = "transient API error listing Registry runs; retrying";
const TRANSIENT_PENDING: &str = "transient API error listing pending Registry runs; retrying";
const NO_RUN: &str = "no Registry run for this SHA; the deployed worker is already current";
const TIMED_OUT: &str = "timed out waiting for a registry deploy containing this SHA";

/// L9, L3, L20 and L33's projections, which run inside `gh`.
const CANDIDATES_JQ: &str = r#".workflow_runs[] | "\(.id) \(.head_sha)""#;
const DEPLOY_STEP_JQ: &str = r#"[.jobs[] | select(.name == "deploy-registry") | .steps[] | select(.name == "Deploy") | .conclusion][0]"#;
const RUNS_JQ: &str = ".workflow_runs";
const PENDING_JQ: &str = r#".workflow_runs[] | select(.status != "completed") | .head_sha"#;

/// The default `IFS` blanks `read` splits and trims on. A line never
/// carries the third one, `\n`.
const IFS: [char; 2] = [' ', '\t'];

/// Wait for a successful `main` Registry deploy whose head contains
/// `$GITHUB_SHA`, or for one of the answers that ends the wait early.
#[must_use]
pub fn run() -> ExitCode {
    ExitCode::from(poll(
        &mut Spawn,
        &context("GITHUB_SHA"),
        &context("GITHUB_REPOSITORY"),
    ))
}

/// `${VAR}` with `-u` off: an unset - or, here, a non-UTF-8 - name
/// reads as the empty string the original spliced in.
fn context(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// What one iteration decided, in the original's terms: the line it
/// echoed and the stream it echoed to.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    /// `echo ...` then `continue` (L21, L34, L47).
    Retry(String),
    /// `echo ...; exit 0` (L14, L26).
    Done(String),
    /// `echo ... >&2; exit 1` (L51).
    Stop(String),
    /// L29's death under `set -e`: the failing tool already wrote its
    /// own diagnostic, and the step said nothing of its own.
    Die,
}

impl Report {
    /// Writes the line and answers with the exit the original took, or
    /// `None` where it went round again.
    fn emit(&self) -> Option<u8> {
        match self {
            Self::Retry(line) => {
                println!("{line}");
                None
            }
            Self::Done(line) => {
                println!("{line}");
                Some(0)
            }
            Self::Stop(line) => {
                eprintln!("{line}");
                Some(1)
            }
            Self::Die => Some(1),
        }
    }
}

/// L5..L57: the loop, its ceiling and the timeout below it.
fn poll(shell: &mut dyn Shell, sha: &str, repository: &str) -> u8 {
    for _ in 0..ITERATIONS {
        if let Some(report) = iteration(shell, sha, repository)
            && let Some(code) = report.emit()
        {
            return code;
        }
        shell.wait();
    }
    eprintln!("{TIMED_OUT}");
    1
}

/// One pass of L6..L54. `None` is the fall-through at L54, which waits
/// without saying anything.
fn iteration(shell: &mut dyn Shell, sha: &str, repository: &str) -> Option<Report> {
    shell.discard(&["git", "fetch", "--quiet", "origin", "main"]);

    // L7. per_page=100: successive skipped-Deploy successes (a
    // migration awaiting its by-hand apply green-lights every
    // subsequent run) must not push the qualifying run out of the
    // scanned window - the same-SHA fallback below cannot break the tie
    // for a merely-successful run, so a buried candidate would idle the
    // loop into its ceiling.
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
    if listed && let Some(report) = scan(shell, sha, repository, &text(candidates)) {
        return Some(report);
    }

    // L19.
    let (listed, runs) = shell.capture(
        &[
            "gh",
            "api",
            &runs_url(repository, &format!("head_sha={sha}")),
            "--jq",
            RUNS_JQ,
        ],
        &[],
    );
    if !listed {
        return Some(Report::Retry(TRANSIENT_RUNS.to_owned()));
    }
    let runs = crate::substitute(runs);

    if empty_list(shell, &runs) {
        return Some(Report::Done(NO_RUN.to_owned()));
    }
    // L29: the block's one substitution outside any condition, so a jq
    // that fails here killed the step under `set -e` (measured: the
    // same malformed capture that L25 survived in its condition). jq's
    // own diagnostic passes through on the inherited stderr.
    let (ran, conclusion) = shell.capture(&["jq", "-r", ".[0].conclusion"], &runs);
    if !ran {
        return Some(Report::Die);
    }
    let conclusion = text(conclusion);
    if conclusion != "failure" && conclusion != "cancelled" {
        return None;
    }

    // L31. Terminal only when nothing pending could still satisfy the
    // ancestor gate: a queued or in-progress run whose head contains
    // this SHA (a rerun, or main already carrying the fix) delivers the
    // deploy on a later iteration, while failing here would strand this
    // run - registry-only pushes never retrigger ports-publish.
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

    // L39..L45. `any` stops at the first match, where the original
    // broke out of the loop.
    let pending = text(pending);
    let descendant_pending = pending.split('\n').any(|line| {
        let head = line.trim_matches(IFS);
        !head.is_empty() && shell.condition(&["git", "merge-base", "--is-ancestor", sha, head])
    });
    if descendant_pending {
        return Some(Report::Retry(format!(
            "Registry run for this SHA concluded '{conclusion}'; a pending run containing it may still deploy; waiting"
        )));
    }
    Some(Report::Stop(format!(
        "Registry run for this SHA concluded '{conclusion}' and no deploy containing it has landed; not publishing against a stale worker"
    )))
}

/// L10..L17: the successful runs, in the order the API listed them,
/// until one whose head contains this SHA carries a Deploy step that
/// ran and succeeded.
fn scan(shell: &mut dyn Shell, sha: &str, repository: &str, candidates: &str) -> Option<Report> {
    for line in candidates.split('\n') {
        let (run_id, head) = read_two(line);
        if run_id.is_empty() {
            continue;
        }
        if !shell.condition(&["git", "merge-base", "--is-ancestor", sha, head]) {
            continue;
        }
        // L13's `|| echo transient` appends to whatever the call
        // already printed, so a partial answer can never read as
        // `success`.
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
        }
        if crate::substitute(conclusion) == b"success".as_slice() {
            return Some(Report::Done(format!(
                "registry deploy {run_id} (head {head}) contains this SHA"
            )));
        }
    }
    None
}

/// L25. `jq` prints nothing for empty input, where `[` diagnoses a
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

/// `printf '%s' "$runs" | jq ...`, captured. The pipeline's status is
/// `jq`'s and nothing reads it (`pipefail` is off), so only the text
/// comes back.
fn jq(shell: &mut dyn Shell, runs: &[u8], argv: &[&str]) -> String {
    let (_, output) = shell.capture(argv, runs);
    text(output)
}

/// A capture as the port compares it: `$(...)`'s byte rules, then the
/// lossy decode the module documents as a ceiling.
fn text(captured: Vec<u8>) -> String {
    String::from_utf8_lossy(&crate::substitute(captured)).into_owned()
}

fn runs_url(repository: &str, query: &str) -> String {
    format!("repos/{repository}/actions/workflows/registry.yml/runs?{query}")
}

fn jobs_url(repository: &str, run_id: &str) -> String {
    format!("repos/{repository}/actions/runs/{run_id}/jobs?per_page=100")
}

/// `read -r rid rsha`: leading and trailing blanks dropped, the first
/// field split off at a run of blanks, and every remaining field left
/// in the second - a line carrying extras keeps them all there.
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
    /// `$(argv)` with `stdin` on the standard input: stdout captured
    /// raw, stderr inherited, and the status the original's `if` and
    /// `||` read.
    fn capture(&mut self, argv: &[&str], stdin: &[u8]) -> (bool, Vec<u8>);
    /// `argv 2>/dev/null` in a condition: the status alone.
    fn condition(&mut self, argv: &[&str]) -> bool;
    /// `argv || true`: output inherited, status ignored.
    fn discard(&mut self, argv: &[&str]);
    /// `sleep 40`.
    fn wait(&mut self);
}

/// The production shell: real child processes, found on `PATH` exactly
/// as the workflow's `gh`, `git` and `jq` were.
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
            // The shell's status 127, which every caller here reads as
            // a failed call.
            Err(error) => {
                eprintln!("{}: {error}", argv[0]);
                return (false, Vec::new());
            }
        };
        if let Some(mut pipe) = child.stdin.take() {
            // The pipeline discards `printf`'s status, a short write
            // against an early-exiting `jq` included.
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

    fn discard(&mut self, argv: &[&str]) {
        // `|| true` swallowed the status alone; a spawn failure still
        // wrote its `command not found` to stderr.
        if let Err(error) = Command::new(argv[0]).args(&argv[1..]).status() {
            eprintln!("{}: {error}", argv[0]);
        }
    }

    fn wait(&mut self) {
        std::thread::sleep(WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted [`Shell`]: each capture is answered by the first
    /// entry whose needle appears in the joined argv, and `merge-base`
    /// answers true for the heads named in `descendants`.
    #[derive(Default)]
    struct Fake {
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

        fn discard(&mut self, argv: &[&str]) {
            self.calls.push(argv.join(" "));
        }

        fn wait(&mut self) {
            self.waits += 1;
        }
    }

    /// The candidate listing answering with one qualifying run.
    const ONE_CANDIDATE: (&str, bool, &str) = ("status=success", true, "77 head77\n");

    #[test]
    fn the_messages_match_the_shells_echoes() {
        assert_eq!(
            TRANSIENT_RUNS,
            "transient API error listing Registry runs; retrying"
        );
        assert_eq!(
            TRANSIENT_PENDING,
            "transient API error listing pending Registry runs; retrying"
        );
        assert_eq!(
            NO_RUN,
            "no Registry run for this SHA; the deployed worker is already current"
        );
        assert_eq!(
            TIMED_OUT,
            "timed out waiting for a registry deploy containing this SHA"
        );
    }

    #[test]
    fn the_urls_and_projections_match_the_originals() {
        assert_eq!(
            runs_url("o/r", "head_sha=abc"),
            "repos/o/r/actions/workflows/registry.yml/runs?head_sha=abc"
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
            "repos//actions/workflows/registry.yml/runs?head_sha="
        );
    }

    #[test]
    fn a_deploy_containing_the_sha_ends_the_wait() {
        let mut shell = Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", true, "success\n")])
            .descending_from(&["head77"]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r"),
            Some(Report::Done(
                "registry deploy 77 (head head77) contains this SHA".to_owned()
            ))
        );
        assert_eq!(Report::Done(String::new()).emit(), Some(0));
    }

    #[test]
    fn a_deploy_step_that_prints_then_fails_is_not_a_success() {
        // `$(deploy_step ... || echo transient)` concatenates, so the
        // capture is `success\ntransient` and loses the comparison.
        let mut shell =
            Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", false, "success\n")])
                .descending_from(&["head77"]);
        // Falls through to the same-SHA listing, which this fake fails.
        assert_eq!(
            iteration(&mut shell, "abc", "o/r"),
            Some(Report::Retry(TRANSIENT_RUNS.to_owned()))
        );
    }

    #[test]
    fn a_candidate_that_is_not_an_ancestor_is_skipped() {
        let mut shell = Fake::answering(&[ONE_CANDIDATE, ("jobs?per_page=100", true, "success\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r"),
            Some(Report::Retry(TRANSIENT_RUNS.to_owned()))
        );
        assert_eq!(shell.called("jobs?per_page=100"), 0);
    }

    #[test]
    fn an_empty_candidate_list_runs_one_field_less_iteration() {
        // The herestring's newline gives `read` one pass with empty
        // fields; the `[ -n "$rid" ]` guard is what makes it a no-op.
        let mut shell = Fake::answering(&[("status=success", true, "")]);
        iteration(&mut shell, "abc", "o/r");
        assert_eq!(shell.called("merge-base"), 0);
    }

    #[test]
    fn a_failed_candidate_listing_skips_the_scan_without_a_word() {
        let mut shell = Fake::answering(&[("status=success", false, "77 head77\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r"),
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

    #[test]
    fn no_registry_run_for_this_sha_exits_zero() {
        let mut shell = Fake::answering(&[("head_sha=", true, "[]"), ("jq length", true, "0\n")]);
        assert_eq!(
            iteration(&mut shell, "abc", "o/r"),
            Some(Report::Done(NO_RUN.to_owned()))
        );
    }

    #[test]
    fn an_unparsable_length_reads_as_non_empty_and_waits() {
        // `jq` prints nothing for empty input; `[ "" -eq 0 ]` answers
        // false, and the conclusion then reads empty, so the iteration
        // falls through to the wait.
        let mut shell = Fake::answering(&[("head_sha=", true, ""), ("jq ", true, "")]);
        assert_eq!(iteration(&mut shell, "abc", "o/r"), None);
    }

    #[test]
    fn a_failing_conclusion_query_kills_the_step() {
        // L29 is the one substitution `set -e` still guards: the same
        // malformed capture the length check just survived.
        let mut shell = Fake::answering(&[
            ("head_sha=", true, "not json"),
            ("jq length", true, "1"),
            ("jq -r", false, ""),
        ]);
        assert_eq!(iteration(&mut shell, "abc", "o/r"), Some(Report::Die));
        assert_eq!(Report::Die.emit(), Some(1));
    }

    #[test]
    fn a_conclusion_that_is_neither_failure_nor_cancelled_waits() {
        for conclusion in ["null", "success", "", "skipped"] {
            let mut shell = Fake::answering(&[
                ("head_sha=", true, "[{}]"),
                ("jq length", true, "1"),
                ("jq -r", true, conclusion),
            ]);
            assert_eq!(iteration(&mut shell, "abc", "o/r"), None, "{conclusion}");
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
                iteration(&mut shell, "abc", "o/r"),
                Some(Report::Retry(format!(
                    "Registry run for this SHA concluded '{conclusion}'; a pending run containing it may still deploy; waiting"
                )))
            );
        }
    }

    #[test]
    fn a_failed_run_with_no_pending_descendant_stops() {
        let mut shell = Fake::answering(&[
            ("head_sha=", true, "[{}]"),
            ("jq length", true, "1"),
            ("jq -r", true, "failure"),
            ("branch=main&per_page=100", true, "stale\n"),
        ]);
        let stale = Report::Stop(
            "Registry run for this SHA concluded 'failure' and no deploy containing it has landed; not publishing against a stale worker"
                .to_owned(),
        );
        assert_eq!(iteration(&mut shell, "abc", "o/r"), Some(stale));
        assert_eq!(Report::Stop(String::new()).emit(), Some(1));
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
            iteration(&mut shell, "abc", "o/r"),
            Some(Report::Retry(TRANSIENT_PENDING.to_owned()))
        );
    }

    #[test]
    fn the_ceiling_is_ninety_waits_and_then_the_timeout() {
        // Every call fails, so each iteration takes the transient
        // branch: the loop reaches its ceiling and exits 1.
        let mut shell = Fake::default();
        assert_eq!(poll(&mut shell, "abc", "o/r"), 1);
        assert_eq!(shell.waits, ITERATIONS);
        assert_eq!(shell.called("git fetch"), ITERATIONS);
    }

    #[test]
    fn an_early_answer_stops_the_loop_without_waiting() {
        let mut shell = Fake::answering(&[("head_sha=", true, "[]"), ("jq length", true, "0")]);
        assert_eq!(poll(&mut shell, "abc", "o/r"), 0);
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
        assert_eq!(poll(&mut shell, "abc", "o/r"), 1);
        assert_eq!(shell.waits, ITERATIONS);
    }
}
