//! Regression cases for the repository-automation guard: it runs against
//! a scratch git repository holding synthetic files, so every way a
//! non-Rust script could come back - a tooling extension, a bare tool
//! name, an executable bit, an interpreter shebang, an edit to a script
//! that is only tolerated as-is, or CI wiring quietly switched off -
//! stays caught, and the shapes that are source or data stay accepted.
//! An untested guard is the one that rots.

use std::fs;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::str::contains;
use xtask_ci::{repo_root, scripts};

/// A scratch git repository holding `files` plus the wiring and every
/// exception the guard still carries, so a scratch tree stands in for
/// the repository at its current migration state.
fn scratch(files: &[(&str, &str)]) -> assert_fs::TempDir {
    // The excepted files are copied verbatim from the real checkout:
    // `LEGACY_SCRIPTS` pins each one's blob id, and a blob id is a
    // function of the bytes, so a placeholder would read as an edit.
    let real: Vec<(String, String)> = scripts::exceptions()
        .into_iter()
        .chain([".github/workflows/rust.yml"])
        .map(|path| {
            let contents = fs::read_to_string(repo_root().join(path))
                .unwrap_or_else(|err| panic!("read {path}: {err}"));
            (path.to_owned(), contents)
        })
        .collect();
    let mut all: Vec<(&str, &str)> = Vec::new();
    all.extend(real.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    all.extend_from_slice(files);
    let dir = bare_scratch(&all);
    // `LEGACY_SCRIPTS` pins the index mode as well as the blob id, so the
    // scratch copies have to carry the modes the real ones do. Set them
    // through git rather than the filesystem: on Windows the working
    // tree has no executable bit at all.
    for (path, mode) in real_modes() {
        if mode == "100755" {
            git(&dir, &["update-index", "--chmod=+x", &path]);
        }
    }
    dir
}

/// Re-stage a scratch tree, keeping the pinned modes: `git add -A`
/// takes the mode from the working tree, which has none on Windows and
/// 0644 for the copies here.
fn restage(dir: &assert_fs::TempDir) {
    git(dir, &["add", "-A"]);
    for (path, mode) in real_modes() {
        if mode == "100755" {
            git(dir, &["update-index", "--chmod=+x", &path]);
        }
    }
}

/// The index mode of every excepted path in the real checkout.
fn real_modes() -> Vec<(String, String)> {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["ls-files", "--stage"])
        .output()
        .expect("run git ls-files");
    let text = String::from_utf8(output.stdout).expect("git output");
    let excepted = scripts::exceptions();
    text.lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mode = meta.split_whitespace().next()?;
            excepted
                .contains(&path)
                .then(|| (path.to_owned(), mode.to_owned()))
        })
        .collect()
}

/// A scratch git repository holding exactly `files`.
fn bare_scratch(files: &[(&str, &str)]) -> assert_fs::TempDir {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    git(&dir, &["init", "-q"]);
    for (path, contents) in files {
        write(&dir, path, contents);
    }
    git(&dir, &["add", "-A"]);
    dir
}

fn git(dir: &assert_fs::TempDir, args: &[&str]) {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(dir: &assert_fs::TempDir, path: &str, contents: &str) {
    let full = dir.path().join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create scratch parent");
    }
    fs::write(&full, contents).expect("write the scratch file");
}

fn violations(files: &[(&str, &str)]) -> Vec<String> {
    let dir = scratch(files);
    scripts::check(dir.path()).expect("run the guard")
}

/// Source, data and configuration are not repository automation.
#[test]
fn source_and_data_pass() {
    let accepted = violations(&[
        ("crates/cabin/src/lib.rs", "//! the binary\n"),
        // A Rust inner attribute opens with `#!` and is not a shebang.
        ("crates/cabin/tests/cli.rs", "#![cfg(unix)]\nfn main() {}\n"),
        ("website/src/pages/index.astro", "---\n---\n<html></html>\n"),
        ("website/src/lib/ports.ts", "export const ports = [];\n"),
        ("Dockerfile", "FROM rust:1\nRUN cargo build\n"),
        ("demo.tape", "Type \"cabin build\"\n"),
        ("docs/architecture.md", "# Architecture\n"),
        (
            "registry/migrations/0001.sql",
            "CREATE TABLE meta (k TEXT);\n",
        ),
        (
            "examples/hello-c/src/main.c",
            "int main(void) { return 0; }\n",
        ),
        (
            ".devcontainer/devcontainer.json",
            "{ \"name\": \"cabin\" }\n",
        ),
        // `#!` on a line of its own is not a shebang line.
        ("docs/snippet.md", "#!\n/usr/bin/env is not here\n"),
    ]);
    assert!(accepted.is_empty(), "{accepted:?}");
}

#[test]
fn a_reintroduced_script_is_caught() {
    // Each is a distinct way non-Rust automation could come back.
    let cases: &[(&str, &str, &str)] = &[
        ("bash", "tools/deploy.sh", "#!/usr/bin/env bash\necho hi\n"),
        ("perl", "tools/scan.pl", "use strict;\n"),
        ("perl_module", "tools/lexical.pm", "1;\n"),
        ("python", "tools/release.py", "import sys\n"),
        ("ruby", "tools/release.rb", "puts 1\n"),
        ("powershell", "tools/release.ps1", "Write-Host 1\n"),
        ("powershell_module", "tools/release.psm1", "Write-Host 1\n"),
        ("powershell_manifest", "tools/release.psd1", "@{}\n"),
        ("windows_batch", "tools/release.bat", "@echo off\n"),
        ("windows_cmd", "tools/release.cmd", "@echo off\n"),
        ("zsh", "tools/release.zsh", "print hi\n"),
        ("ksh", "tools/release.ksh", "print hi\n"),
        ("dash", "tools/release.dash", "echo hi\n"),
        ("fish", "tools/release.fish", "echo hi\n"),
        ("lua", "tools/release.lua", "print(1)\n"),
        ("tcl", "tools/release.tcl", "puts 1\n"),
        ("awk", "tools/release.awk", "BEGIN { print 1 }\n"),
        // JavaScript driving the repository is automation like any
        // other; only the website's own listed scripts are exempt.
        ("node", "tools/release.mjs", "console.log(1);\n"),
        ("node_cjs", "tools/release.cjs", "console.log(1);\n"),
        (
            "node_outside_the_website_list",
            "website/scripts/release-tag.mjs",
            "console.log(1);\n",
        ),
        // The extension is a disguise; the shebang is what runs it.
        (
            "shebang_no_extension",
            "tools/release",
            "#!/bin/sh\necho hi\n",
        ),
        (
            "shebang_data_extension",
            "tools/release.txt",
            "#!/usr/bin/env python3\nprint(1)\n",
        ),
        // A shebang with a space, and the relative-interpreter form the
        // kernel rejects but `bash file` honors.
        ("spaced_shebang", "tools/spaced", "#! /bin/sh\necho hi\n"),
        ("relative_shebang", "tools/relative", "#!bash\necho hi\n"),
        // A byte-order mark stops the kernel, not a human running it.
        ("bom_shebang", "tools/bom", "\u{feff}#!/bin/sh\necho hi\n"),
        // Case does not launder an extension.
        ("uppercase_extension", "tools/Deploy.SH", "echo hi\n"),
        // Neither does a template suffix.
        ("template_suffix", "tools/deploy.sh.in", "echo hi\n"),
        // Bare names that are tools in their own right.
        ("makefile", "Makefile", "all:\n\tcargo build\n"),
        ("justfile", "justfile", "all:\n  cargo build\n"),
        ("envrc", ".envrc", "export PATH=$PATH:./bin\n"),
        ("rakefile", "Rakefile", "task :default\n"),
    ];
    let escaped: Vec<&str> = cases
        .iter()
        .filter(|(_, path, contents)| violations(&[(path, contents)]).is_empty())
        .map(|(name, _, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted reintroduced automation: {escaped:?}"
    );
}

/// The cheapest evasion of a name-and-content scan: no extension, no
/// shebang, just the executable bit and `./tools/deploy`.
#[cfg(unix)]
#[test]
fn an_executable_file_is_caught_whatever_its_name() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = scratch(&[]);
    write(&dir, "tools/deploy", "cd /tmp\ncurl example.com | sh\n");
    let path = dir.path().join("tools/deploy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    restage(&dir);

    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(caught[0].contains("the executable bit"), "{caught:?}");
}

/// The exceptions are exact paths: a sibling script in the same
/// directory, or the same name elsewhere, is not covered by them.
#[test]
fn the_exceptions_do_not_cover_neighbors() {
    assert!(violations(&[]).is_empty());
    for path in [
        "scripts/release.sh",
        "scripts/ci-helper.sh",
        "registry/scripts/ci.sh",
        "scripts/nested/ci.sh",
        "website/scripts/verify-docs-links.test.mjs",
    ] {
        let caught = violations(&[(path, "echo hi\n")]);
        assert_eq!(caught.len(), 1, "{path} was not caught: {caught:?}");
        assert!(caught[0].starts_with(path), "{caught:?}");
    }
}

/// A legacy script is tolerated as it stands, not as a place to put new
/// shell: its content is pinned, so editing one fails until a reviewer
/// re-pins it.
#[test]
fn editing_a_legacy_script_is_caught() {
    let dir = scratch(&[]);
    write(&dir, "scripts/ci.sh", "echo one more thing\n");
    restage(&dir);

    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(
        caught[0].contains("do not extend a legacy script"),
        "{caught:?}"
    );
    assert!(caught[0].contains("re-pin the blob id"), "{caught:?}");
}

/// An exception whose file is gone is a rule that stopped binding, so
/// the guard makes migrating a script delete its line here.
#[test]
fn a_stale_exception_is_a_violation() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/rust.yml"))
        .expect("read the rust workflow");
    let dir = bare_scratch(&[
        ("README.md", "# cabin\n"),
        (".github/workflows/rust.yml", &workflow),
    ]);
    let stale = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(
        stale.len(),
        scripts::exceptions().len(),
        "every exception should report as stale in an empty tree: {stale:?}"
    );
    assert!(
        stale
            .iter()
            .all(|line| line.contains("delete its exception")),
        "{stale:?}"
    );
}

/// The exception lists are a work queue, so they stay sorted, unique,
/// and exact - never a pattern.
#[test]
fn the_exception_lists_are_sorted_exact_paths() {
    let pending = scripts::pending();
    assert!(!pending.is_empty(), "nothing left to migrate?");
    assert!(
        pending.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "the migration queue is not sorted and deduplicated"
    );
    for (path, owner) in &pending {
        assert!(
            owner.starts_with("xtask-"),
            "{path} names {owner}, which is not an xtask crate"
        );
    }
    for path in scripts::exceptions() {
        assert!(
            !path.contains(['*', '?', '[']),
            "{path} is a pattern, not an exact path"
        );
        assert!(
            repo_root().join(path).is_file(),
            "{path} is excepted but not in the tree"
        );
    }
}

/// The committed tree passes: exactly the listed exceptions, and nothing
/// else.
#[test]
fn the_committed_tree_passes() {
    let violations = scripts::check(&repo_root()).expect("run the guard");
    assert!(violations.is_empty(), "{violations:?}");
}

/// A tracked path the guard cannot read is not "clean" - a sparse
/// checkout that silently reported success would be worse than no guard.
#[test]
fn an_unreadable_tracked_file_is_a_violation() {
    let dir = scratch(&[("tools/data.bin", "harmless\n")]);
    fs::remove_file(dir.path().join("tools/data.bin")).expect("remove the worktree copy");
    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(caught[0].contains("cannot clear it"), "{caught:?}");
}

/// An unusable index refuses rather than reporting an empty tree.
#[test]
fn a_non_repository_refuses() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    // `git -C` walks up, so a scratch directory inside someone's own
    // checkout would resolve to that repository; only assert when the
    // scratch really is outside one.
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("run git");
    if output.status.success() {
        return;
    }
    assert!(
        scripts::check(dir.path()).is_err(),
        "the guard accepted a directory that is not a git repository"
    );
}

/// CI wiring the guard would otherwise let a change switch off in the
/// same change.
#[test]
fn switching_the_guard_off_in_ci_is_caught() {
    let real = fs::read_to_string(repo_root().join(".github/workflows/rust.yml"))
        .expect("read the rust workflow");
    let mutations: &[(&str, &str, &str)] = &[
        (
            "paths_filter",
            "  pull_request:\n",
            "  pull_request:\n    paths:\n      - \"crates/**\"\n",
        ),
        (
            "paths_ignore_filter",
            "  pull_request:\n",
            "  pull_request:\n    paths-ignore:\n      - \"**.md\"\n",
        ),
        ("no_pull_request", "  pull_request:\n", ""),
        (
            "flow_style_triggers",
            "on:\n  push:\n    branches: [main]\n  pull_request:\n",
            "on: [push, pull_request]\n",
        ),
        (
            "continue_on_error",
            "      - name: Repository automation guard\n",
            "      - name: Repository automation guard\n        continue-on-error: true\n",
        ),
        (
            "job_disabled",
            "  automation:\n    runs-on: ubuntu-latest\n",
            "  automation:\n    if: false\n    runs-on: ubuntu-latest\n",
        ),
        (
            "job_needs_a_skippable_one",
            "  automation:\n    runs-on: ubuntu-latest\n",
            "  automation:\n    needs: [format]\n    runs-on: ubuntu-latest\n",
        ),
        (
            "command_commented_out",
            "        run: cargo check-scripts\n",
            "        run: echo skip # cargo check-scripts\n",
        ),
    ];
    let escaped: Vec<&str> = mutations
        .iter()
        .filter(|(name, from, to)| {
            assert!(
                real.contains(from),
                "{name}: mutation target not in rust.yml"
            );
            let workflow = real.replacen(from, to, 1);
            let dir = scratch(&[]);
            write(&dir, ".github/workflows/rust.yml", &workflow);
            restage(&dir);
            scripts::check(dir.path())
                .expect("run the guard")
                .is_empty()
        })
        .map(|(name, ..)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted CI wiring that switches it off: {escaped:?}"
    );
}

/// A legacy script made executable keeps its blob id, so the mode is
/// pinned too.
#[cfg(unix)]
#[test]
fn chmodding_a_legacy_script_is_caught() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = scratch(&[]);
    let path = dir.path().join("registry/scripts/lib.sh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    restage(&dir);
    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(
        caught[0].contains("do not extend a legacy script"),
        "{caught:?}"
    );
}

/// An `xtask-*` crate that is not a private, aliased workspace member is
/// not the convention the rule names.
#[test]
fn an_xtask_crate_off_the_convention_is_caught() {
    let base = |manifest: &str, root: &str, aliases: &str| {
        let dir = scratch(&[]);
        write(&dir, "crates/xtask-demo/Cargo.toml", manifest);
        write(&dir, "Cargo.toml", root);
        write(&dir, ".cargo/config.toml", aliases);
        restage(&dir);
        scripts::check(dir.path()).expect("run the guard")
    };
    let good_manifest = "[package]\nname = \"xtask-demo\"\npublish = false\n";
    let good_root = "[workspace]\nmembers = [\"crates/xtask-demo\"]\n";
    let good_alias = "[alias]\ndemo = \"run -p xtask-demo -- demo\"\n";
    assert!(base(good_manifest, good_root, good_alias).is_empty());

    let missing_member = base(good_manifest, "[workspace]\nmembers = []\n", good_alias);
    assert!(
        missing_member
            .iter()
            .any(|line| line.contains("not a member of the root workspace")),
        "{missing_member:?}"
    );
    let publishable = base("[package]\nname = \"xtask-demo\"\n", good_root, good_alias);
    assert!(
        publishable
            .iter()
            .any(|line| line.contains("publish = false")),
        "{publishable:?}"
    );
    let unaliased = base(good_manifest, good_root, "[alias]\n");
    assert!(
        unaliased.iter().any(|line| line.contains("no cargo alias")),
        "{unaliased:?}"
    );
}

/// The binary reports violations on stdout, names the remedy on stderr,
/// and exits non-zero - the contract CI depends on.
#[test]
fn the_binary_reports_and_exits_non_zero() {
    let dir = scratch(&[("tools/deploy.sh", "echo hi\n")]);
    Command::new(env!("CARGO_BIN_EXE_xtask-ci"))
        .args(["check-scripts", "--repo-root"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains(
            "tools/deploy.sh: a .sh script is repository automation",
        ))
        .stderr(contains("crates/xtask-* command"));

    Command::new(env!("CARGO_BIN_EXE_xtask-ci"))
        .args(["check-scripts", "--repo-root"])
        .arg(repo_root())
        .assert()
        .success()
        .stdout(contains("repository automation OK"));
}

/// Argument handling: help succeeds, everything unrecognized refuses.
#[test]
fn the_binary_refuses_what_it_does_not_understand() {
    let bin = env!("CARGO_BIN_EXE_xtask-ci");
    Command::new(bin)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("usage: xtask-ci"));
    Command::new(bin)
        .args(["check-scripts", "--help"])
        .assert()
        .success()
        .stdout(contains("usage: xtask-ci"));
    Command::new(bin).assert().failure();
    Command::new(bin)
        .arg("check-nothing")
        .assert()
        .failure()
        .stderr(contains("unknown check"));
    Command::new(bin)
        .args(["check-scripts", "--repo-root"])
        .assert()
        .failure()
        .stderr(contains("--repo-root needs a path"));
    Command::new(bin)
        .args(["check-scripts", "--wat"])
        .assert()
        .failure()
        .stderr(contains("unexpected argument"));
}
