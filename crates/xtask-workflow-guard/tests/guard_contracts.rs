//! The workflow guards' live contracts: each case spawns the real
//! binary in a scratch working directory and asserts the exit status
//! and what it left in `$GITHUB_OUTPUT`, the two channels
//! `registry.yml` reads.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use sha2::{Digest as _, Sha256};

/// Handed to every git call and to the guard itself, so the corpus is
/// reproducible and the machine's own git configuration is neither
/// read nor writable. `GIT_TERMINAL_PROMPT=0` makes an unreachable
/// remote fail rather than block on a credential prompt.
const GIT_ENVIRONMENT: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "cabin test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_COMMITTER_NAME", "cabin test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// Inherited redirections a caller's environment could carry: each
/// would point the corpus or the guard at shared state the pinned
/// values above do not cover.
const GIT_REDIRECTIONS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_CONFIG_COUNT",
    "GIT_TEMPLATE_DIR",
];

fn pin_git_environment(command: &mut Command) {
    for &(name, value) in GIT_ENVIRONMENT {
        command.env(name, value);
    }
    for name in GIT_REDIRECTIONS {
        command.env_remove(name);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.current_dir(dir).args(args);
    pin_git_environment(&mut command);
    let done = command.output().expect("git is runnable");
    assert!(
        done.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    String::from_utf8_lossy(&done.stdout).trim().to_owned()
}

/// Runs the guard binary in `dir` with the given subcommand and
/// environment on top of the pinned git environment.
fn guard(dir: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask-workflow-guard"));
    command.current_dir(dir).args(arguments);
    pin_git_environment(&mut command);
    command.env_remove("GITHUB_OUTPUT");
    for &(name, value) in environment {
        command.env(name, value);
    }
    command.output().expect("the guard is runnable")
}

/// A work tree pushing to a bare `origin.git`, both under `scratch`.
fn cloned_pair(scratch: &Path) -> std::path::PathBuf {
    let origin = scratch.join("origin.git");
    fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "--bare", "-b", "main", "."]);
    let work = scratch.join("work");
    fs::create_dir(&work).unwrap();
    git(&work, &["init", "-b", "main", "."]);
    git(&work, &["remote", "add", "origin", "../origin.git"]);
    work
}

/// Writes `name`, commits everything, pushes main, and returns the
/// new head.
fn push_commit(work: &Path, name: &str, message: &str) -> String {
    let file = work.join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, message).unwrap();
    git(work, &["add", "."]);
    git(work, &["commit", "-q", "-m", message]);
    git(work, &["push", "-q", "origin", "main"]);
    git(work, &["rev-parse", "HEAD"])
}

/// `superseded` answers exclusively through `$GITHUB_OUTPUT`: any
/// later origin/main commit appends `superseded=true`, the newest
/// commit appends nothing, and an unreachable origin fails the step
/// without writing.
#[test]
fn superseded_answers_through_github_output() {
    let scratch = assert_fs::TempDir::new().unwrap();
    let work = cloned_pair(scratch.path());
    let first = push_commit(&work, "registry/src/lib.rs", "first");
    push_commit(&work, "registry/src/lib.rs", "second");
    let newest = push_commit(&work, "website/page.astro", "third");

    let output = scratch.path().join("overtaken");
    let run = guard(
        &work,
        &["superseded"],
        &[
            ("GITHUB_SHA", &first),
            ("GITHUB_OUTPUT", &output.to_string_lossy()),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert_eq!(fs::read_to_string(&output).unwrap(), "superseded=true\n");

    let quiet = scratch.path().join("newest");
    let run = guard(
        &work,
        &["superseded"],
        &[
            ("GITHUB_SHA", &newest),
            ("GITHUB_OUTPUT", &quiet.to_string_lossy()),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert!(!quiet.exists(), "nothing follows the newest commit");

    git(
        &work,
        &["remote", "set-url", "origin", "../not-a-repository.git"],
    );
    let unwritten = scratch.path().join("unreachable");
    let run = guard(
        &work,
        &["superseded"],
        &[
            ("GITHUB_SHA", &first),
            ("GITHUB_OUTPUT", &unwritten.to_string_lossy()),
        ],
    );
    assert!(
        !run.status.success(),
        "an unreachable origin fails the step"
    );
    assert!(!unwritten.exists(), "and writes nothing");
}

/// With `--relevant-to`, only newer commits matching that filter's
/// list in `.github/path-filters.yml` supersede: an irrelevant newer
/// commit writes nothing, a relevant one writes `superseded=true`,
/// and a list the file does not carry falls back to unscoped
/// supersession with a warning.
#[test]
fn superseded_scoping_reads_the_shared_filter_file() {
    let scratch = assert_fs::TempDir::new().unwrap();
    let work = cloned_pair(scratch.path());
    fs::create_dir_all(work.join(".github")).unwrap();
    fs::write(
        work.join(".github/path-filters.yml"),
        "# fixture copy of the shared filter file\nregistry:\n  - \"registry/**\"\n  - \"Cargo.toml\"\n",
    )
    .unwrap();
    let first = push_commit(&work, "registry/src/lib.rs", "first");
    push_commit(&work, "website/page.astro", "website only");

    let ignored = scratch.path().join("ignored");
    let run = guard(
        &work,
        &["superseded", "--relevant-to", "registry"],
        &[
            ("GITHUB_SHA", &first),
            ("GITHUB_OUTPUT", &ignored.to_string_lossy()),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert!(
        !ignored.exists(),
        "a website-only commit does not supersede"
    );
    assert!(run.stderr.is_empty(), "no fallback warning: {run:?}");

    push_commit(&work, "registry/src/main.rs", "registry again");
    let output = scratch.path().join("overtaken");
    let run = guard(
        &work,
        &["superseded", "--relevant-to", "registry"],
        &[
            ("GITHUB_SHA", &first),
            ("GITHUB_OUTPUT", &output.to_string_lossy()),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert_eq!(fs::read_to_string(&output).unwrap(), "superseded=true\n");

    let fallback = scratch.path().join("fallback");
    let run = guard(
        &work,
        &["superseded", "--relevant-to", "no-such-filter"],
        &[
            ("GITHUB_SHA", &first),
            ("GITHUB_OUTPUT", &fallback.to_string_lossy()),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert_eq!(
        fs::read_to_string(&fallback).unwrap(),
        "superseded=true\n",
        "an unusable list falls back to any-newer-commit"
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("no usable `no-such-filter` list"),
        "{run:?}"
    );
}

fn stamp_corpus(root: &Path, applied: &str) {
    let migrations = root.join("registry/migrations");
    fs::create_dir_all(&migrations).unwrap();
    fs::write(migrations.join("0001_init.sql"), SQL).unwrap();
    fs::write(root.join("registry/migrations-applied"), applied).unwrap();
}

const SQL: &str = "create table t (x);\n";

fn stamp() -> String {
    Sha256::digest(SQL.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The one `::error` annotation a blocked gate prints, or `None`.
fn blocked_annotation(run: &Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&run.stdout);
    let mut lines = stdout.lines();
    let line = lines
        .next()?
        .strip_prefix("::error title=Registry deploy blocked::")?;
    assert_eq!(lines.next(), None, "one annotation line: {run:?}");
    Some(line.to_owned())
}

/// Runs `migrations-pending` in `root` with a writable `$GITHUB_OUTPUT`,
/// so a gate that answered there instead of failing would exit 0, and
/// returns the run and whether it wrote that file.
fn migrations_gate(root: &Path) -> (Output, bool) {
    let output = root.join("out");
    let run = guard(
        root,
        &["migrations-pending"],
        &[("GITHUB_OUTPUT", &output.to_string_lossy())],
    );
    (run, output.exists())
}

/// `migrations-pending` passes silently when the committed migrations
/// match the stamp, and otherwise fails the step with one error
/// annotation naming the cause and the by-hand recovery, so a blocked
/// deploy job goes red instead of reading as a deploy that happened.
#[test]
fn migrations_pending_blocks_the_deploy_until_the_stamp_matches() {
    let current = assert_fs::TempDir::new().unwrap();
    stamp_corpus(current.path(), &format!("{}\n", stamp()));
    let (run, wrote) = migrations_gate(current.path());
    assert!(run.status.success(), "{run:?}");
    assert!(run.stdout.is_empty() && !wrote, "{run:?}");

    let pending = assert_fs::TempDir::new().unwrap();
    stamp_corpus(pending.path(), "not-the-stamp\n");
    let (run, wrote) = migrations_gate(pending.path());
    assert!(!run.status.success() && !wrote, "{run:?}");
    let annotation = blocked_annotation(&run).expect("a blocked-deploy annotation");
    assert!(annotation.contains("no longer matches"), "{annotation}");
    assert!(
        annotation.contains("cargo registry-migrate --remote"),
        "{annotation}"
    );
}

/// A stamp file that cannot be read blocks the deploy too, naming the
/// read failure and a repair rather than a migrate run that would
/// refuse the same state.
#[test]
fn an_unreadable_stamp_blocks_the_deploy() {
    let scratch = assert_fs::TempDir::new().unwrap();
    stamp_corpus(scratch.path(), "");
    fs::remove_file(scratch.path().join("registry/migrations-applied")).unwrap();
    let (run, wrote) = migrations_gate(scratch.path());
    assert!(!run.status.success() && !wrote, "{run:?}");
    let annotation = blocked_annotation(&run).expect("a blocked-deploy annotation");
    assert!(annotation.contains("cannot compare"), "{annotation}");
    assert!(
        annotation.contains("Repair what the error names"),
        "{annotation}"
    );
}
