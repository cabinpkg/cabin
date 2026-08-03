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
        .chain(scripts::pinned_workflows())
        .chain([".cargo/config.toml"])
        .map(|path| {
            let contents = fs::read_to_string(repo_root().join(path))
                .unwrap_or_else(|err| panic!("read {path}: {err}"));
            (path.to_owned(), contents)
        })
        .collect();
    // Each product-source root must hold something, or the guard reports
    // it stale like any other declaration that stopped binding.
    let roots: Vec<String> = scripts::source_roots()
        .into_iter()
        .map(|root| format!("{root}placeholder.ts"))
        .collect();
    // Every aliased tool must sit at crates/<name>, so a scratch tree
    // carries a stub of each: the guard reads the location, not the
    // code, and a tree without them is not this repository.
    let stubs: Vec<(String, String)> = scripts::aliased_packages(&repo_root())
        .expect("read the aliases")
        .into_iter()
        .map(|name| {
            (
                format!("crates/{name}/Cargo.toml"),
                format!("[package]\nname = \"{name}\"\npublish = false\n"),
            )
        })
        .collect();
    let mut all: Vec<(&str, &str)> =
        vec![("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n")];
    all.extend(real.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    all.extend(roots.iter().map(|path| (path.as_str(), "export {};\n")));
    all.extend(stubs.iter().map(|(p, c)| (p.as_str(), c.as_str())));
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

/// Stage `contents` at `path` in the index alone, leaving the working
/// tree as it is: the guard reads the index, and some paths cannot be
/// written next to what they collide with.
fn stage_blob(dir: &assert_fs::TempDir, path: &str, contents: &str) {
    let scratch_file = dir.path().join(".staged-blob");
    fs::write(&scratch_file, contents).expect("write the blob source");
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["hash-object", "-w", "--"])
        .arg(&scratch_file)
        .output()
        .expect("run git hash-object");
    assert!(output.status.success(), "git hash-object failed");
    fs::remove_file(&scratch_file).expect("drop the blob source");
    let oid = String::from_utf8(output.stdout).expect("git output");
    git(
        dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{},{path}", oid.trim()),
        ],
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
        ("website/src/scripts/home-stats.ts", "export {};\n"),
        // Source languages are source INSIDE the website source root.
        ("website/src/lib/helper.mjs", "export const x = 1;\n"),
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
        // Windows Script Host: `cscript tools/release.vbs` runs it with
        // nothing installed.
        ("vbscript", "tools/release.vbs", "WScript.Echo 1\n"),
        ("vbscript_encoded", "tools/release.vbe", "#@~^AAAA==\n"),
        ("windows_script_file", "tools/release.wsf", "<job/>\n"),
        ("windows_script_host", "tools/release.wsh", "[ScriptFile]\n"),
        ("jscript_encoded", "tools/release.jse", "#@~^AAAA==\n"),
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
        // TypeScript is a source language, but only inside the
        // website source root; a build tool written in it is a tool.
        ("typescript", "tools/deploy.ts", "export {};\n"),
        ("typescript_jsx", "tools/deploy.tsx", "export {};\n"),
        ("typescript_module", "tools/deploy.mts", "export {};\n"),
        // ...and a shell script is a script even inside it.
        (
            "shell_in_the_source_root",
            "website/src/build.sh",
            "echo hi\n",
        ),
        (
            "node_outside_the_website_list",
            "website/scripts/release-tag.mjs",
            "console.log(1);\n",
        ),
        // A local action is automation whose entry point is whatever its
        // metadata says - `runs.main` can name a file with no telling
        // extension at all, so the metadata is what gets refused.
        (
            "local_action",
            ".github/actions/deploy/action.yml",
            "runs:\n  using: node20\n  main: deploy.data\n",
        ),
        (
            "local_action_yaml",
            ".github/actions/deploy/action.yaml",
            "runs:\n  using: composite\n",
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

/// The pinned wiring must sit where YAML would read it: copying the
/// pinned job into a block scalar while deleting the real one satisfies
/// a substring match but leaves the workflow with no guard job.
#[test]
fn the_pinned_job_must_be_a_real_job() {
    let real = fs::read_to_string(repo_root().join(".github/workflows/rust.yml"))
        .expect("read the rust workflow")
        .replace("\r\n", "\n");
    let job_start = real.find("  automation:\n").expect("the automation job");
    let terminator = "\n  clippy:";
    let job_end = real.find(terminator).expect("the next job") + terminator.len();
    let job = real[job_start..job_end].to_owned();

    // Delete the real job, and park its exact text as inert content of a
    // top-level block scalar - at the same indent, so a substring match
    // still finds it.
    let without = format!(
        "{}{}",
        &real[..job_start],
        &real[job_end - terminator.len() + 1..]
    );
    let mutated = without.replace("jobs:\n", &format!("x-note: |\n{job}\n\njobs:\n"));
    assert!(
        mutated.contains(&job),
        "the smuggled copy should still satisfy a substring match"
    );
    assert!(
        !mutated.contains("\njobs:\n  automation:"),
        "the real job should be gone"
    );

    let dir = scratch(&[]);
    write(&dir, ".github/workflows/rust.yml", &mutated);
    restage(&dir);
    let caught = scripts::check(dir.path()).expect("run the guard");
    assert!(
        caught
            .iter()
            .any(|line| line.contains("automation job is not the pinned one")),
        "{caught:?}"
    );
}

/// A cargo runner would make `cargo run` execute something else - the
/// guard's own CI job included - so the config stays alias-only.
#[test]
fn a_non_alias_cargo_config_section_is_caught() {
    let cases: &[(&str, &str)] = &[
        (
            "runner",
            "[alias]\ndemo = \"run -p xtask-demo -- demo\"\n\n[target.'cfg(all())']\nrunner = \"true\"\n",
        ),
        (
            "build",
            "[alias]\ndemo = \"run -p xtask-demo -- demo\"\n\n[build]\nrustflags = [\"-C\", \"panic=abort\"]\n",
        ),
    ];
    let escaped: Vec<&str> = cases
        .iter()
        .filter(|(_, config)| {
            let dir = scratch(&[]);
            write(&dir, ".cargo/config.toml", config);
            restage(&dir);
            !scripts::check(dir.path())
                .expect("run the guard")
                .iter()
                .any(|line| line.contains("must stay [alias]-only"))
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(escaped.is_empty(), "the guard accepted {escaped:?}");
}

/// A workflow that runs an alias has to keep the alias file and the
/// tool's crate in its filters. Whether it still does is a question
/// about GitHub's ordered pattern matching, so the block is pinned
/// instead: every edit to it is a re-pin, wherever it is spelled.
#[test]
fn an_alias_consumer_off_its_pinned_triggers_is_caught() {
    let path = ".github/workflows/registry.yml";
    let real = fs::read_to_string(repo_root().join(path))
        .expect("read the registry workflow")
        .replace("\r\n", "\n");
    let mutations: &[(&str, &str, &str)] = &[
        // The two inputs the rule is about, dropped from one filter.
        (
            "alias_file_dropped",
            "      # On the config search path of the registry workspace too.\n      \
             - \".cargo/config.toml\"\n",
            "",
        ),
        (
            "crate_dropped",
            "      - \"crates/xtask-registry-guard/**\"\n",
            "",
        ),
        // Ordered patterns: a later negation cancels an earlier entry.
        (
            "crate_negated",
            "      - \"crates/xtask-registry-guard/**\"\n",
            "      - \"crates/xtask-registry-guard/**\"\n      \
             - \"!crates/xtask-registry-guard/**\"\n",
        ),
        // Flow style says the same thing in text that shares nothing.
        (
            "flow_style",
            "on:\n  push:\n    branches: [ main ]\n",
            "on: { push: { branches: [main] },\n",
        ),
    ];
    let escaped: Vec<&str> = mutations
        .iter()
        .filter(|(name, from, to)| {
            assert!(real.contains(from), "{name}: mutation target not in {path}");
            let dir = scratch(&[]);
            write(&dir, path, &real.replacen(from, to, 1));
            restage(&dir);
            !scripts::check(dir.path())
                .expect("run the guard")
                .iter()
                .any(|line| line.contains("trigger block is not the pinned one"))
        })
        .map(|(name, ..)| *name)
        .collect();
    assert!(escaped.is_empty(), "the guard accepted {escaped:?}");

    // A pin is taken for the tools the workflow ran when it was taken:
    // adding a call to another one leaves the block itself untouched.
    let dir = scratch(&[]);
    let called = "        run: cargo check-sql";
    assert!(real.contains(called), "mutation target not in {path}");
    write(
        &dir,
        path,
        &real.replacen(
            called,
            "        run: |\n          cargo check-sql\n          cargo port-publish",
            1,
        ),
    );
    restage(&dir);
    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(caught[0].contains("was pinned for"), "{caught:?}");

    // A new consumer nobody pinned is the case the pins exist for, in
    // each spelling a shell reads as the same command.
    let head = "on:\n  pull_request:\n    paths:\n      - \"docs/**\"\n\njobs:\n  \
                guard:\n    steps:\n";
    for call in [
        "      - run: cargo check-sql;\n",
        "      - run: cargo --locked check-sql\n",
        // A backslash continuation, and a folded block scalar: both put
        // the command and its argument on different lines.
        "      - run: |\n          cargo \\\n            check-sql\n",
        "      - run: >\n          cargo\n          check-sql\n",
        // Redirection needs no space around it.
        "      - run: cargo check-sql>/dev/null\n",
        // Assembled from a variable: nothing literal follows `cargo`.
        "    env:\n      CMD: check-sql\n    steps:\n      - run: cargo \"$CMD\"\n",
    ] {
        let caught = violations(&[(".github/workflows/consumer.yml", &format!("{head}{call}"))]);
        assert_eq!(caught.len(), 1, "{call}: {caught:?}");
        assert!(caught[0].contains("no pinned trigger block"), "{caught:?}");
    }
}

/// The alias-only check is worth only as much as the guarantee that the
/// checked file is the config cargo reads: cargo prefers the
/// extensionless name, and reads one per directory on the way up.
#[test]
fn a_second_cargo_config_is_caught() {
    let hostile = "[target.'cfg(all())']\nrunner = \"true\"\n";
    for path in [
        ".cargo/config",
        "registry/.cargo/config.toml",
        "registry/.cargo/config",
        "crates/xtask-ci/.cargo/config.toml",
    ] {
        let caught = violations(&[(path, hostile)]);
        assert_eq!(caught.len(), 1, "{path}: {caught:?}");
        assert!(caught[0].contains("a second cargo config"), "{caught:?}");
    }
    // Windows and macOS resolve these to the paths cargo reads, while
    // git records the name as it was typed. Staged straight into the
    // index: on those same filesystems the file cannot be WRITTEN
    // beside its sibling, which is the point.
    for path in [".Cargo/config", ".cargo/Config.toml"] {
        let dir = scratch(&[]);
        stage_blob(&dir, path, hostile);
        let caught = scripts::check(dir.path()).expect("run the guard");
        assert_eq!(caught.len(), 1, "{path}: {caught:?}");
        assert!(caught[0].contains("a second cargo config"), "{caught:?}");
    }
}

/// The aliases are the repository's tool surface, so they are checked
/// from their own side too: a tool added as an ordinary package with an
/// alias pointed at it never lands under `crates/xtask-*` for the crate
/// scan to see.
#[test]
fn an_alias_onto_a_non_xtask_package_is_caught() {
    let real =
        fs::read_to_string(repo_root().join(".cargo/config.toml")).expect("read the cargo config");
    let cases: &[(&str, &str)] = &[
        ("plain", "repo-task = \"run --locked -p repo-task --\"\n"),
        ("long_flag", "repo-task = \"run --package repo-task --\"\n"),
        ("joined", "repo-task = \"run -prepo-task --\"\n"),
        ("equals", "repo-task = \"run --package=repo-task --\"\n"),
        (
            "array",
            "repo-task = [\"run\", \"-p\", \"repo-task\", \"--\"]\n",
        ),
        // The convention is a name AND a place: a package may be called
        // `xtask-anything` while living outside crates/.
        (
            "xtask_name_elsewhere",
            "repo-task = \"run --locked -p xtask-bypass --\"\n",
        ),
        // ...and a crates/xtask-* directory may declare any name at
        // all, leaving the alias naming a package nothing provides.
        (
            "crate_declares_another_name",
            "repo-task = \"run --locked -p xtask-misnamed --\"\n",
        ),
        // An alias that selects no package at all runs whatever the
        // working directory resolves to.
        ("no_package", "repo-task = \"run --bin repo-task --\"\n"),
        // An array alias is not the same declaration: cargo joins array
        // values across config layers instead of overriding them.
        (
            "array_value",
            "repo-task = [\"run\", \"--locked\", \"-p\", \"xtask-ci\", \"--\"]\n",
        ),
    ];
    let escaped: Vec<&str> = cases
        .iter()
        .filter(|(_, alias)| {
            let dir = scratch(&[]);
            write(&dir, ".cargo/config.toml", &format!("{real}{alias}"));
            // A crate at the conventional path that declares another
            // name: everything about it is right except what it is.
            write(
                &dir,
                "crates/xtask-misnamed/Cargo.toml",
                "[package]\nname = \"ordinary-tool\"\npublish = false\n",
            );
            restage(&dir);
            !scripts::check(dir.path())
                .expect("run the guard")
                .iter()
                .any(|line| line.contains("`cargo repo-task` alias"))
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(escaped.is_empty(), "the guard accepted {escaped:?}");
}

/// A source root that names nothing is a rule that stopped binding.
#[test]
fn the_source_roots_hold_tracked_source() {
    assert!(violations(&[]).is_empty(), "a seeded scratch tree is clean");
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/rust.yml"))
        .expect("read the rust workflow");
    let bare = bare_scratch(&[(".github/workflows/rust.yml", &workflow)]);
    let caught = scripts::check(bare.path()).expect("run the guard");
    assert!(
        caught
            .iter()
            .any(|line| line.contains("declared a product-source root but tracks nothing")),
        "a root that holds nothing should report stale: {caught:?}"
    );
}

/// A symlink would give an excepted script a second path the list does
/// not carry.
#[cfg(unix)]
#[test]
fn a_symlink_to_an_excepted_script_is_caught() {
    let dir = scratch(&[]);
    fs::create_dir_all(dir.path().join("tools")).expect("create tools/");
    std::os::unix::fs::symlink("../scripts/ci.sh", dir.path().join("tools/ci")).expect("symlink");
    restage(&dir);
    let caught = scripts::check(dir.path()).expect("run the guard");
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(caught[0].contains("a tracked symlink"), "{caught:?}");
}

/// An exception covers a path, not every kind of thing that path could
/// be: swapping an excepted file for a symlink would alias whatever it
/// points at into the tree under a name the guard clears.
#[cfg(unix)]
#[test]
fn an_exception_swapped_for_a_symlink_is_caught() {
    for path in ["website/astro.config.ts", "scripts/ci.sh"] {
        let dir = scratch(&[]);
        fs::remove_file(dir.path().join(path)).expect("drop the excepted file");
        std::os::unix::fs::symlink("../scripts/ci.sh", dir.path().join(path)).expect("symlink");
        // Stage just this path: a full restage would try to chmod +x a
        // symlink, which git refuses.
        git(&dir, &["add", "-A", path]);
        let caught = scripts::check(dir.path()).expect("run the guard");
        assert_eq!(caught.len(), 1, "{path}: {caught:?}");
        assert!(caught[0].starts_with(path), "{caught:?}");
    }
}

/// The same for the executable bit, which the website list does not pin
/// the way the legacy list pins a mode alongside its blob id.
#[cfg(unix)]
#[test]
fn a_website_exception_made_executable_is_caught() {
    let dir = scratch(&[]);
    git(
        &dir,
        &["update-index", "--chmod=+x", "website/astro.config.ts"],
    );
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
        // Every exception, every source root, and the missing alias file.
        scripts::exceptions().len() + scripts::source_roots().len() + 1,
        "every exception and source root should report stale in an empty tree: {stale:?}"
    );
    assert_eq!(
        stale
            .iter()
            .filter(|line| line.contains("delete its exception"))
            .count(),
        scripts::exceptions().len(),
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
    // Windows checkouts normalize to CRLF; the mutations below are
    // written with the line endings the repository stores.
    let real = fs::read_to_string(repo_root().join(".github/workflows/rust.yml"))
        .expect("read the rust workflow")
        .replace("\r\n", "\n");
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
            "        run: ./target/debug/xtask-ci check-scripts\n",
            "        run: echo skip # ./target/debug/xtask-ci check-scripts\n",
        ),
        (
            // `cargo run` would honor a [target] runner; the direct exec
            // is what makes the alias-only check trustworthy.
            "back_to_cargo_run",
            "        run: ./target/debug/xtask-ci check-scripts\n",
            "        run: cargo check-scripts\n",
        ),
        (
            // A custom shell is handed the step's script as an argument
            // and may ignore it. Unpinning the shell re-opens that to a
            // workflow-level `defaults.run.shell`.
            "shell_reinterpreted",
            "      - name: Repository automation guard\n        shell: bash\n",
            "      - name: Repository automation guard\n        shell: \"true {0}\"\n",
        ),
        (
            // Bash sources $BASH_ENV before the script it was handed, so
            // a workflow-level variable can end the step green.
            "bash_env_injected",
            "env:\n  CARGO_TERM_COLOR: always\n",
            "env:\n  BASH_ENV: docs/preamble\n  CARGO_TERM_COLOR: always\n",
        ),
        (
            // `./target/debug/xtask-ci` is a different file under a
            // different directory, and a workflow-level
            // `defaults.run.working-directory` supplies one to any step
            // that does not pin its own.
            "working_directory_unpinned",
            "      - name: Repository automation guard\n        shell: bash\n        \
             working-directory: .\n",
            "      - name: Repository automation guard\n        shell: bash\n",
        ),
        (
            "shell_unpinned",
            "      - name: Repository automation guard\n        shell: bash\n",
            "      - name: Repository automation guard\n",
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
    // The repository's own aliases stay: the workflows copied into the
    // scratch tree run them, and a tree where they do not exist is not
    // the tree this test is about.
    let real =
        fs::read_to_string(repo_root().join(".cargo/config.toml")).expect("read the cargo config");
    let base = |manifest: &str, root: &str, alias: &str| {
        let dir = scratch(&[]);
        write(&dir, "crates/xtask-demo/Cargo.toml", manifest);
        write(&dir, "Cargo.toml", root);
        write(&dir, ".cargo/config.toml", &format!("{real}{alias}"));
        restage(&dir);
        scripts::check(dir.path()).expect("run the guard")
    };
    let good_manifest = "[package]\nname = \"xtask-demo\"\npublish = false\n";
    let good_root = "[workspace]\nmembers = [\"crates/*\"]\n";
    let good_alias = "demo = \"run -p xtask-demo -- demo\"\n";
    assert!(base(good_manifest, good_root, good_alias).is_empty());
    // `-p` and `--package` select the same package, so both are the
    // alias this crate is reached through.
    let long_form = "demo = \"run --package xtask-demo -- demo\"\n";
    assert!(base(good_manifest, good_root, long_form).is_empty());

    // `exclude` takes back what the glob swept in.
    let excluded = base(
        good_manifest,
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/xtask-demo\"]\n",
        good_alias,
    );
    assert!(
        excluded
            .iter()
            .any(|line| line.contains("not a member of the root workspace")),
        "{excluded:?}"
    );

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
    let unaliased = base(good_manifest, good_root, "");
    assert!(
        unaliased.iter().any(|line| line.contains("no cargo alias")),
        "{unaliased:?}"
    );
}

/// `publish = false` keeps a tool off the registry, not out of the
/// binary: what ships must not depend on one.
#[test]
fn a_shipped_crate_depending_on_a_tool_is_caught() {
    let cases: &[(&str, &str)] = &[
        (
            "dependency",
            "[package]\nname = \"cabin\"\n\n[dependencies]\nxtask-ci = { path = \"../xtask-ci\" }\n",
        ),
        (
            "build_dependency",
            "[package]\nname = \"cabin\"\n\n[build-dependencies]\n\
             xtask-ci = { path = \"../xtask-ci\" }\n",
        ),
        (
            "per_target_dependency",
            "[package]\nname = \"cabin\"\n\n[target.'cfg(unix)'.dependencies]\n\
             xtask-ci = { path = \"../xtask-ci\" }\n",
        ),
    ];
    for (name, manifest) in cases {
        let caught = violations(&[("crates/cabin/Cargo.toml", manifest)]);
        assert_eq!(caught.len(), 1, "{name}: {caught:?}");
        assert!(caught[0].contains("never part of what ships"), "{caught:?}");
    }
    // A rename changes the key, not what is depended on.
    let renamed = "[package]\nname = \"cabin\"\n\n[dependencies]\n\
                   guard = { package = \"xtask-ci\", path = \"../xtask-ci\" }\n";
    let caught = violations(&[("crates/cabin/Cargo.toml", renamed)]);
    assert_eq!(caught.len(), 1, "{caught:?}");
    assert!(caught[0].contains("never part of what ships"), "{caught:?}");

    // A rename can also live in the workspace table the crate inherits.
    let inherited = violations(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n\n\
             [workspace.dependencies]\nguard = { package = \"xtask-ci\", path = \"crates/xtask-ci\" }\n",
        ),
        (
            "crates/cabin/Cargo.toml",
            "[package]\nname = \"cabin\"\n\n[dependencies]\nguard.workspace = true\n",
        ),
    ]);
    assert_eq!(inherited.len(), 1, "{inherited:?}");
    assert!(
        inherited[0].contains("never part of what ships"),
        "{inherited:?}"
    );

    // A dev-dependency is test-only, which is how crates/cabin reaches
    // the publisher for its registry fixtures.
    let dev = "[package]\nname = \"cabin\"\n\n[dev-dependencies]\n\
               xtask-ci = { path = \"../xtask-ci\" }\n";
    assert!(violations(&[("crates/cabin/Cargo.toml", dev)]).is_empty());
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
