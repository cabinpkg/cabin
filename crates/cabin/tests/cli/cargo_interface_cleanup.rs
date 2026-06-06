use super::*;

/// Capture `cabin <args> --help` stdout for a focused
/// substring assertion. Any non-zero exit fails the test.
fn help_text(args: &[&str]) -> String {
    let mut full: Vec<&str> = args.to_vec();
    full.push("--help");
    let output = cabin().args(&full).assert().success().get_output().clone();
    String::from_utf8(output.stdout).expect("help output should be utf-8")
}

#[test]
fn top_level_help_lists_day_to_day_subcommands() {
    let out = help_text(&[]);
    // `cabin --help` is curated to match cargo's `--help`
    // pattern: only commands users type day-to-day.  The
    // inspection, offline, scripting, packaging, and
    // distribution helpers live under `cabin --list`.  See
    // the dedicated `curated_help_and_list` module for the
    // full curation contract.  The visible set is derived
    // from clap (`Cli::command()`); we never hard-code it.
    for cmd in visible_subcommand_names() {
        assert!(out.contains(&cmd), "top-level help missing `{cmd}`:\n{out}");
    }
    // Cabin describes itself for C/C++, not Rust.
    assert!(
        out.contains("C/C++") || out.contains("C/C++"),
        "top-level help should mention C/C++:\n{out}"
    );
}

#[test]
fn cabin_run_help_mentions_documented_flags() {
    let out = help_text(&["run"]);
    for needle in [
        "--bin",
        "--manifest-path",
        "--build-dir",
        "--profile",
        "--release",
        "--features",
        "--all-features",
        "--no-default-features",
        "--locked",
        "--frozen",
        "--offline",
        "--no-patches",
    ] {
        assert!(
            out.contains(needle),
            "`cabin run --help` missing `{needle}`:\n{out}"
        );
    }
}

#[test]
fn cabin_build_help_uses_build_dir_not_target_dir() {
    let out = help_text(&["build"]);
    assert!(out.contains("--build-dir"), "build help: {out}");
    assert!(
        !out.contains("--target-dir"),
        "Cabin must not surface `--target-dir`:\n{out}"
    );
}

#[test]
fn cabin_build_and_test_help_do_not_advertise_target_flag() {
    // `--target` is reserved for a future platform/toolchain
    // target. The historic manifest-target selector overload
    // is gone, so neither help screen should advertise the
    // flag.
    for sub in ["build", "test"] {
        let out = help_text(&[sub]);
        assert!(
            !out.contains("--target"),
            "`cabin {sub} --help` must not advertise `--target`:\n{out}"
        );
    }
}

#[test]
fn cabin_metadata_help_lists_format_flag() {
    let out = help_text(&["metadata"]);
    assert!(out.contains("--format"), "metadata help: {out}");
}

#[test]
fn cabin_tree_help_lists_kind_filter() {
    let out = help_text(&["tree"]);
    assert!(out.contains("--kind"), "tree help: {out}");
    assert!(out.contains("--format"), "tree help: {out}");
}

#[test]
fn cabin_explain_help_lists_subcommand_set() {
    let out = help_text(&["explain"]);
    for sub in ["package", "target", "source", "feature", "build-config"] {
        assert!(out.contains(sub), "explain help missing `{sub}`:\n{out}");
    }
}

#[test]
fn cabin_vendor_help_mentions_vendor_dir() {
    let out = help_text(&["vendor"]);
    assert!(
        out.contains("external versioned dependencies"),
        "vendor help should not claim to vendor local path deps: {out}"
    );
    assert!(out.contains("--vendor-dir"), "vendor help: {out}");
    assert!(out.contains("--offline"), "vendor help: {out}");
}

#[test]
fn cabin_version_reports_workspace_release_version() {
    let output = cabin()
        .arg("--version")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("--version should be utf-8");
    // The workspace's `[workspace.package] version` drives
    // every crate's `--version`; if a future release bumps
    // it, this test must be updated alongside the bump.
    assert!(
        stdout.contains(CABIN_VERSION),
        "expected `cabin --version` to mention the {CABIN_VERSION} release, got: {stdout}"
    );
}

#[test]
fn cabin_about_describes_c_and_cpp_not_just_cpp() {
    let out = help_text(&[]);
    // Cabin's about text must describe the package as
    // serving both C/C++; the older "for C++" branding
    // is gone.
    assert!(
        out.contains("for C/C++"),
        "expected `--help` to describe Cabin for C/C++; got: {out}"
    );
}
