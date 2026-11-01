//! Command-line shim for the local CI gate.
//!
//!   `cargo ci`          run the checks; exits non-zero on failure
//!   `cargo ci --hook`   agent Stop-hook adapter: reads the hook JSON
//!                       on stdin, always exits 0, and prints `{}` on
//!                       success or a "block" decision naming the
//!                       failed step

use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use clap::Parser;
use xtask_ci::{Gate, arm_teardown, cores, hook, repo_root, scope, teardown_exits_zero};

/// The hidden flag the website phase re-executes this binary with.
const WEBSITE_STEPS: &str = "website-steps";

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long)]
    hook: bool,
    #[arg(long = WEBSITE_STEPS, hide = true)]
    website_steps: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.hook {
        return run_hook();
    }
    if cli.website_steps {
        return website_steps();
    }
    match run(Box::new(std::io::stdout()), false) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// The website phase's body, re-executed as its own child so the
/// whole npm sequence stays one phase: one process group, one log,
/// one status.  The npm commands are spawned directly rather than
/// through `bash -c` - a supported Windows host has npm but no bash
/// (crates/AGENTS.md portability), and the commands themselves are
/// plain npm invocations.
fn website_steps() -> ExitCode {
    let website = match repo_root() {
        Ok(root) => root.join("website"),
        Err(err) => {
            eprintln!("error: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    // `npm.cmd`: on Windows npm is a cmd shim, which `Command::new`
    // does not resolve from `npm`.
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    for step in [&["ci"][..], &["run", "lint"], &["test"], &["run", "build"]] {
        match Command::new(npm).args(step).current_dir(&website).status() {
            Ok(status) if status.success() => {}
            Ok(_) => return ExitCode::FAILURE,
            Err(err) => {
                eprintln!("error: run npm {}: {err}", step.join(" "));
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// The Stop-hook adapter.  Every path exits 0 - including a panic,
/// which would otherwise exit 101 and read as "the hook crashed"
/// rather than as a decision.
fn run_hook() -> ExitCode {
    teardown_exits_zero();
    let input = hook::read_stdin();
    // Everything the gate says is captured: stdout here carries only
    // the JSON body, and the failed-step marker has to be recoverable
    // from the same text.
    let sink = Shared::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(Box::new(sink.clone()), true)
    }));
    let passed = matches!(outcome, Ok(Ok(())));
    let mut bytes = sink.bytes();
    match &outcome {
        Ok(Err(error)) => bytes.extend_from_slice(format!("error: {error:#}\n").as_bytes()),
        Err(_) => bytes.extend_from_slice(b"error: the gate panicked\n"),
        Ok(Ok(())) => {}
    }
    // Replayed as bytes: a stray invalid byte in compiler output must
    // not corrupt - let alone drop - everything around it.  The lossy
    // copy below is for the line-oriented marker scan only.
    {
        use std::io::Write as _;
        let _ = std::io::stderr().write_all(&bytes);
    }
    let text = String::from_utf8_lossy(&bytes);
    println!(
        "{}",
        hook::decision(
            passed,
            &hook::failed_step(&text),
            hook::already_blocked(&input)
        )
    );
    ExitCode::SUCCESS
}

/// A sink the hook can read back after the gate has written to it.
#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::io::Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the gate, writing its progress to `out`.
fn run(out: Box<dyn std::io::Write + Send>, capture: bool) -> Result<()> {
    arm_teardown();
    let root = repo_root()?
        .canonicalize()
        .context("resolve the repository root")?;
    // Resolved here, before the first `==>` marker: failing later,
    // between phases, would let the hook attribute the error to
    // whichever step's marker came last.
    let gate_binary = std::env::current_exe().context("locate the gate binary")?;

    let base = merge_base(&root);
    let mut commits = 0;
    let surfaces = match &base {
        Some(base) => {
            let changed = changed_paths(&root, base)?;
            commits = commits_since(&root, base)?;
            if changed.is_empty() && commits == 0 {
                let mut out = out;
                writeln!(
                    out,
                    "no changes since {}; nothing to check",
                    short(&root, base)
                )?;
                return Ok(());
            }
            scope::surfaces(&changed)
        }
        None => scope::Surfaces::all(),
    };

    let cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let jobs = cores::split(u32::try_from(cpus).unwrap_or(u32::MAX));
    let mut gate = Gate::new(root.clone(), jobs.parallel, out, capture);

    gate.step(
        "cargo fmt --all --verbose -- --check",
        Command::new("cargo").args(["fmt", "--all", "--verbose", "--", "--check"]),
    )?;
    gate.step(
        "taplo fmt --check",
        Command::new("taplo").args(["fmt", "--check"]),
    )?;
    gate.step("typos", &mut Command::new("typos"))?;

    if let Some(base) = &base
        && commits > 0
    {
        // `npm.cmd`/`npx.cmd`: the same Windows cmd shims as in
        // `website_steps`, which `Command::new` cannot resolve from
        // the bare names.  Running from `.github/commitlint` picks up
        // the committed config and the `npm ci`-installed lockfile
        // versions; git discovery still resolves the repository from
        // that subdirectory.
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
        let tooling = root.join(".github").join("commitlint");
        gate.step(
            "npm ci (commitlint)",
            Command::new(npm)
                .args(["ci", "--ignore-scripts"])
                .current_dir(&tooling),
        )?;
        gate.step(
            "commitlint",
            Command::new(npx)
                .args(["commitlint", "--from", base, "--to", "HEAD", "--verbose"])
                .current_dir(&tooling),
        )?;
    }

    checks(&mut gate, &root, &surfaces, &jobs, &gate_binary)?;

    gate.finish()?;
    gate.say("local CI green")?;
    Ok(())
}

/// The expensive half of the gate: everything whose cost is worth
/// scoping to a surface.
fn checks(
    gate: &mut Gate,
    root: &std::path::Path,
    surfaces: &scope::Surfaces,
    jobs: &cores::Jobs,
    gate_binary: &std::path::Path,
) -> Result<()> {
    let nextest = which("cargo-nextest", root);
    let target = |name: &str| root.join("target").join(name);
    let test = jobs.test.to_string();

    if surfaces.rust {
        gate.launch(
            "cargo clippy (workspace, all targets, all features)",
            Command::new("cargo")
                .env("CARGO_TARGET_DIR", target("ci-clippy"))
                .args(["clippy", "--workspace", "--all-targets", "--all-features"])
                .args(["--locked", "--verbose", "--jobs", &jobs.clippy.to_string()])
                .args(["--", "-D", "warnings"]),
        )?;
        gate.launch(
            "cargo check (workspace, all targets, -D warnings)",
            Command::new("cargo")
                .env("CARGO_TARGET_DIR", target("ci-check"))
                .env("RUSTFLAGS", "-D warnings")
                .args([
                    "check",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--verbose",
                ])
                .args(["--jobs", &jobs.check.to_string()]),
        )?;
        let flavor = if nextest { "nextest" } else { "test" };
        gate.launch(
            &format!("cargo {flavor} (workspace, all targets, all features)"),
            &mut test_phase(nextest, target("ci-test"), &test),
        )?;
        gate.launch(
            "cargo doc (workspace, no deps, -D warnings)",
            Command::new("cargo")
                .env("CARGO_TARGET_DIR", target("ci-doc"))
                .env("RUSTDOCFLAGS", "-D warnings")
                .args(["doc", "--workspace", "--all-features", "--no-deps"])
                .args(["--locked", "--verbose", "--jobs", &jobs.doc.to_string()]),
        )?;
    } else {
        gate.say("skipping clippy/check/test/doc: no Rust changes since main")?;
    }
    // Unscoped: a filter-file or registry-manifest edit alone can break it.
    gate.launch(
        "cargo workflow-filter-coverage",
        Command::new("cargo").arg("workflow-filter-coverage"),
    )?;

    website(gate, surfaces, gate_binary)
}

/// The test phase's command over the whole workspace.
/// `cargo-nextest` runs the same test set (the phase excludes
/// doctests either way, via `--all-targets`) but schedules each test
/// in its own process instead of one binary at a time, which is the
/// bulk of this phase's wall clock.  It is an optional accelerator:
/// without it the phase runs CI's command plus the `--jobs` and
/// `--test-threads` caps.
fn test_phase(nextest: bool, target: std::path::PathBuf, jobs: &str) -> Command {
    let mut command = Command::new("cargo");
    command
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTFLAGS", "-D warnings");
    if nextest {
        command
            .args(["nextest", "run", "--workspace", "--all-targets"])
            .args([
                "--all-features",
                "--locked",
                "--no-fail-fast",
                "--cargo-verbose",
            ])
            .args(["--build-jobs", jobs, "--test-threads", jobs]);
    } else {
        command
            .args(["test", "--workspace", "--all-targets"])
            .args(["--all-features", "--locked", "--no-fail-fast", "--verbose"])
            .args(["--jobs", jobs])
            .args(["--", "--show-output", "--test-threads", jobs]);
    }
    command
}

/// The website leg, which mirrors `website.yml` exactly.
fn website(
    gate: &mut Gate,
    surfaces: &scope::Surfaces,
    gate_binary: &std::path::Path,
) -> Result<()> {
    if surfaces.website {
        gate.launch(
            "npm ci && npm run lint && npm test && npm run build (website/)",
            Command::new(gate_binary).arg(format!("--{WEBSITE_STEPS}")),
        )?;
    } else {
        gate.say(
            "skipping website lint/test/build: no website/, docs/ or ports/ changes since main",
        )?;
    }

    Ok(())
}

/// `git merge-base HEAD origin/main`, falling back to `main`.  No base
/// means the gate runs whole rather than guessing what changed.
fn merge_base(root: &std::path::Path) -> Option<String> {
    for reference in ["origin/main", "main"] {
        let out = Command::new("git")
            .args(["merge-base", "HEAD", reference])
            .current_dir(root)
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if out.status.success() {
            let base = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !base.is_empty() {
                return Some(base);
            }
        }
    }
    None
}

/// Tracked changes since the base, plus untracked files, which CI sees
/// once they are committed.  A rename lists both paths, so moving files
/// out of a surface selects it.
fn changed_paths(root: &std::path::Path, base: &str) -> Result<Vec<String>> {
    let mut paths = git(root, &["diff", "--no-renames", "--name-only", base, "--"])?;
    paths.extend(git(root, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(paths)
}

fn commits_since(root: &std::path::Path, base: &str) -> Result<usize> {
    Ok(git(root, &["rev-list", &format!("{base}..HEAD")])?.len())
}

fn short(root: &std::path::Path, base: &str) -> String {
    git(root, &["rev-parse", "--short", base])
        .ok()
        .and_then(|lines| lines.into_iter().next())
        .unwrap_or_else(|| base.to_owned())
}

fn git(root: &std::path::Path, args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Whether any regular file named `program` is on PATH (directories
/// never count).  Executability must NOT be required here: a broken -
/// even non-executable - installation must attempt `cargo nextest`
/// and fail red rather than silently falling back to `cargo test`.
/// Deliberately not "runs `--version` successfully" either: probing
/// by execution would let a transient failure downgrade a red gate
/// the same way.
///
/// Relative and empty PATH entries resolve against `root`: the gate's
/// answer must not depend on which subdirectory it was invoked from.
fn which(program: &str, root: &std::path::Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| root.join(directory).join(program).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(command: &Command) -> Vec<String> {
        assert_eq!(command.get_program(), "cargo");
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_folded_test_phase_builds_the_exact_argv() {
        assert_eq!(
            argv(&test_phase(true, "t".into(), "8")),
            [
                "nextest",
                "run",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
                "--no-fail-fast",
                "--cargo-verbose",
                "--build-jobs",
                "8",
                "--test-threads",
                "8"
            ]
        );
        assert_eq!(
            argv(&test_phase(false, "t".into(), "8")),
            [
                "test",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
                "--no-fail-fast",
                "--verbose",
                "--jobs",
                "8",
                "--",
                "--show-output",
                "--test-threads",
                "8"
            ]
        );
    }

    #[test]
    fn a_committed_rename_lists_its_source_path() {
        let scratch = assert_fs::TempDir::new().unwrap();
        let repo = scratch.path();
        // The machine's own git configuration (signing, identity) must
        // not reach the fixture's commits, and a hook's exported
        // `GIT_DIR` must not redirect them into the caller's repository.
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_TEMPLATE_DIR")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "cabin test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "cabin test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        git(&["init", "-q", "."]);
        std::fs::create_dir(repo.join("docs")).unwrap();
        std::fs::write(repo.join("docs/page.md"), "page\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        git(&["mv", "docs", "elsewhere"]);
        git(&["commit", "-q", "-m", "move"]);

        assert_eq!(
            changed_paths(repo, &base).unwrap(),
            ["docs/page.md", "elsewhere/page.md"]
        );
    }
}
