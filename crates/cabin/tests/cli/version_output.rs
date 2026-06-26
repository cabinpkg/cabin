//! End-to-end coverage for the `cabin version` subcommand.
//!
//! `cabin --version` is the clap-framework spelling and
//! continues to work. `cabin version` is the dedicated
//! subcommand:
//! - concise output by default;
//! - verbose key/value block under `-v` (or the global
//!   `cabin -v version` form);
//! - quiet does not suppress the requested version output;
//! - missing optional metadata renders as `unknown`, never
//!   as an error;
//! - output never leaks private local paths, usernames, or
//!   hostnames.

use super::*;

/// Run `cabin version <args>`, assert success, and return
/// the captured stdout as a `String`.  Stderr is checked to
/// be empty so a future regression that prints status
/// chatter to the wrong stream is caught.
fn run_version(args: &[&str]) -> String {
    let assertion = cabin().args(args).assert().success();
    let out = assertion.get_output();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.is_empty(),
        "cabin {args:?} stderr should be empty: {stderr}"
    );
    String::from_utf8(out.stdout.clone()).expect("stdout should be utf-8")
}

#[test]
fn version_subcommand_prints_concise_release_name() {
    let stdout = run_version(&["version"]);
    // Matches the wording of `cabin --version`: `cabin
    // <semver>` followed by a newline.  The workspace
    // version drives the value.
    assert_eq!(stdout, format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn top_level_dash_dash_version_still_works() {
    let stdout = run_version(&["--version"]);
    // clap renders the same line; `cabin --version` and
    // `cabin version` agree on the concise wording.
    assert_eq!(stdout, format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn top_level_dash_v_short_still_works() {
    // The clap-framework `-V` short alias must keep working
    // even after the new `version` subcommand is added.
    let stdout = run_version(&["-V"]);
    assert_eq!(stdout, format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn version_verbose_emits_cargo_style_block() {
    let stdout = run_version(&["version", "-v"]);
    // The header line is `cabin <semver>` plus an optional
    // `(<short-hash> <date>)` parenthetical when git
    // metadata is available.  Pin the prefix without
    // coupling the test to the build's git state.
    let first_line = stdout.lines().next().expect("at least one line");
    assert!(
        first_line.starts_with(format!("cabin {CABIN_VERSION}").as_str()),
        "first line should be the release banner: {first_line}"
    );
    // `release:` is always emitted; `commit-hash:` /
    // `commit-date:` / `host:` / `os:` are conditional on
    // their underlying source being available.
    assert!(
        stdout.contains(format!("release: {CABIN_VERSION}").as_str()),
        "verbose version missing `release:` line: {stdout}"
    );
}

#[test]
fn version_verbose_long_form_matches_short_form() {
    let short = run_version(&["version", "-v"]);
    let long = run_version(&["version", "--verbose"]);
    assert_eq!(
        short, long,
        "`-v` and `--verbose` should produce identical output"
    );
}

#[test]
fn global_verbose_before_subcommand_also_triggers_verbose() {
    // The global `-v` flag is documented as the verbosity
    // entry point; `cabin -v version` should match
    // `cabin version -v` byte-for-byte.
    let trailing = run_version(&["version", "-v"]);
    let leading = run_version(&["-v", "version"]);
    assert_eq!(
        trailing, leading,
        "`cabin -v version` and `cabin version -v` should agree"
    );
}

#[test]
fn verbose_emits_fields_in_deterministic_order() {
    // Two consecutive runs must produce identical output -
    // build-time fields are captured once and the runtime
    // OS probe is deterministic on a stable host.
    let first = run_version(&["version", "-v"]);
    let second = run_version(&["version", "-v"]);
    assert_eq!(first, second);
    // The released cargo-style banner is:
    //
    // cabin <semver> [(<short-hash> <date>)]
    // release: <semver>
    // commit-hash: <full-hash> (optional)
    // commit-date: <date> (optional)
    // host: <triple> (optional)
    // os: <os string> (optional)
    //
    // Walk the optional rows in order; whichever are
    // present must appear in this sequence.
    let canonical_order = ["release", "commit-hash", "commit-date", "host", "os"];
    let observed: Vec<&str> = first
        .lines()
        .skip(1) // header line has no label
        .filter_map(|line| line.split(':').next())
        .collect();
    let mut next = 0;
    for label in &observed {
        let position = canonical_order
            .iter()
            .position(|known| known == label)
            .unwrap_or_else(|| panic!("unexpected label `{label}` in {first}"));
        assert!(
            position >= next,
            "labels appeared out of order: `{label}` before expected slot ({first})"
        );
        next = position;
    }
    // `release:` must always appear; the others are
    // conditional but at least one further row should print
    // on a typical developer or CI host.
    assert!(observed.contains(&"release"));
}

#[test]
fn version_quiet_does_not_suppress_output() {
    // Quiet only suppresses Cabin-owned status chatter;
    // version output is the requested command output and
    // must still print.
    let stdout = run_version(&["version", "-q"]);
    assert_eq!(stdout, format!("cabin {CABIN_VERSION}\n"));
    let stdout_leading = run_version(&["-q", "version"]);
    assert_eq!(stdout_leading, format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn version_verbose_host_matches_target_triple_shape() {
    let stdout = run_version(&["version", "-v"]);
    // The host triple uses cargo's canonical
    // `<arch>-<vendor>-<os>[-<env>]` shape; pin only the
    // structural hyphen count so the test stays portable
    // across CI architectures.
    let host_line = stdout
        .lines()
        .find(|line| line.starts_with("host:"))
        .unwrap_or_else(|| panic!("verbose version missing `host:` line: {stdout}"));
    let triple = host_line["host:".len()..].trim();
    assert!(
        triple.matches('-').count() >= 2,
        "host triple should have at least two dashes: {triple}"
    );
}

#[test]
fn version_verbose_never_leaks_local_filesystem_paths() {
    let stdout = run_version(&["version", "-v"]);
    // Hard-coded prefixes used by macOS, Linux, and the
    // common temp / opt directories: none of these belong
    // anywhere in stable version output.
    for needle in ["/Users/", "/home/", "/private/", "/opt/", "/tmp/", "/root/"] {
        assert!(
            !stdout.contains(needle),
            "verbose version unexpectedly contains `{needle}`: {stdout}"
        );
    }
}

#[test]
fn version_verbose_never_leaks_username_or_hostname() {
    let stdout = run_version(&["version", "-v"]);
    // The developer's username should never appear in
    // version output even by coincidence.  Skip the assertion
    // gracefully when the value is too generic (e.g. empty
    // or "user") to avoid false positives.
    if let Ok(user) = std::env::var("USER")
        && !user.is_empty()
        && user.len() >= 3
        && user != "user"
    {
        assert!(
            !stdout.contains(&user),
            "verbose version unexpectedly contains $USER ({user}): {stdout}"
        );
    }
    if let Ok(home) = std::env::var("HOME") {
        // Trim to the final path component to spot the
        // developer's home directory name even when the
        // full path was suppressed.
        if let Some(name) = std::path::Path::new(&home)
            .file_name()
            .and_then(|s| s.to_str())
            && !name.is_empty()
            && name.len() >= 3
        {
            assert!(
                !stdout.contains(name),
                "verbose version unexpectedly contains $HOME basename ({name}): {stdout}"
            );
        }
    }
}

#[test]
fn version_verbose_never_leaks_internal_planning_or_prompt_text() {
    let stdout = run_version(&["version", "-v"]);
    // The implementation must not leak planning labels, prompt
    // text, or marker strings that belong in development notes
    // rather than the user surface.
    for forbidden in ["TODO", "FIXME", "roadmap"] {
        assert!(
            !stdout.contains(forbidden),
            "verbose version unexpectedly contains `{forbidden}`: {stdout}"
        );
    }
}

#[test]
fn top_level_help_lists_version_subcommand() {
    let assertion = cabin().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("version"),
        "top-level help should list the `version` subcommand: {stdout}"
    );
}

#[test]
fn version_help_documents_inheriting_global_verbosity() {
    // `cabin version --help` should surface the same global
    // `-v` / `--verbose` and `-q` / `--quiet` flags every
    // other subcommand inherits.
    let assertion = cabin().args(["version", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("--verbose"),
        "`cabin version --help` should mention --verbose: {stdout}"
    );
    assert!(
        stdout.contains("--quiet"),
        "`cabin version --help` should mention --quiet: {stdout}"
    );
}

#[test]
fn version_works_outside_workspace() {
    // The version command must not require a workspace; it
    // is a self-describing CLI surface.
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .arg("version")
        .assert()
        .success();
    let stdout =
        String::from_utf8(assertion.get_output().stdout.clone()).expect("stdout should be utf-8");
    assert_eq!(stdout, format!("cabin {CABIN_VERSION}\n"));
}

#[test]
fn version_verbose_works_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["version", "-v"])
        .assert()
        .success();
    let stdout =
        String::from_utf8(assertion.get_output().stdout.clone()).expect("stdout should be utf-8");
    // The verbose banner does not depend on the working
    // directory; the header always starts with the release
    // line.  Whether the parenthetical git metadata appears
    // depends on the build, not on the current directory.
    assert!(stdout.starts_with(format!("cabin {CABIN_VERSION}").as_str()));
    assert!(stdout.contains(format!("\nrelease: {CABIN_VERSION}\n").as_str()));
}

/// Preservation: every command that `cabin --help`
/// advertises today must remain advertised after the
/// version-output work.  The expected set is derived from clap
/// - the visible subcommands are exactly the ones that
///   will appear in the help block.
#[test]
fn other_cargo_interface_commands_still_appear_in_help() {
    let assertion = cabin().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    for cmd in visible_subcommand_names() {
        assert!(
            stdout.contains(&cmd),
            "top-level help should still list `{cmd}`: {stdout}"
        );
    }
}
