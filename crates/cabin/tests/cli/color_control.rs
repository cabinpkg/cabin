use super::*;

/// ANSI control sequence introducer; presence in the
/// captured bytes is the test's signal for "the renderer
/// emitted styling".
const ESC: char = '\x1b';

/// Drive `cabin metadata` against a non-existent manifest
/// in `dir`. The workspace loader emits the
/// `cabin::workspace::manifest_not_found` typed
/// diagnostic, which the renderer paints when color is on.
/// We use this path because it is the cheapest way to
/// produce a Cabin-owned diagnostic without setting up a
/// build environment.
fn missing_manifest_command(dir: &TempDir) -> Command {
    let mut cmd = cabin();
    cmd.current_dir(dir.path()).arg("metadata");
    cmd
}

#[test]
fn color_value_auto_is_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    // `--color auto` must parse cleanly. Failure is
    // expected (no manifest exists), but it must be a
    // workspace error, not a CLI parse error (clap exits
    // with code 2 on parse error; Cabin returns exit code
    // 1 on workspace failure). miette's fancy renderer
    // prints the diagnostic code on its own line; with
    // `auto` color the code may be wrapped in ANSI
    // escapes, so we match the bare code text rather than
    // a fixed prefix.
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("auto")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("cabin::workspace::manifest_not_found"),
        "expected workspace error reaching the diagnostic renderer, got: {stderr}"
    );
}

#[test]
fn color_value_always_is_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    missing_manifest_command(&dir)
        .arg("--color")
        .arg("always")
        .assert()
        .failure();
}

#[test]
fn color_value_never_is_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    missing_manifest_command(&dir)
        .arg("--color")
        .arg("never")
        .assert()
        .failure();
}

#[test]
fn color_value_unknown_is_rejected_by_clap() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("sometimes")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    // clap's value-enum rejection mentions the offending
    // value and lists the valid set. Pin both substrings
    // rather than the exact message so a clap upgrade
    // does not break the test.
    assert!(
        stderr.contains("'sometimes'") || stderr.contains("\"sometimes\""),
        "expected the invalid value to appear in the error, got: {stderr}"
    );
    assert!(
        stderr.contains("auto") && stderr.contains("always") && stderr.contains("never"),
        "expected the valid set to appear in the error, got: {stderr}"
    );
}

#[test]
fn cabin_term_color_invalid_value_yields_documented_wording() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .env("CABIN_TERM_COLOR", "sometimes")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(
            "invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never"
        ),
        "expected documented env-error wording, got: {stderr}"
    );
}

#[test]
fn color_always_emits_ansi_escapes_for_workspace_diagnostic() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("always")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(ESC),
        "expected ANSI escape with --color always, got: {stderr:?}"
    );
    // miette wraps the bare code line in ANSI; pin the
    // bare code text and the message body.
    assert!(
        stderr.contains("cabin::workspace::manifest_not_found"),
        "expected diagnostic code, got: {stderr}"
    );
    assert!(
        stderr.contains("could not find a Cabin workspace"),
        "expected diagnostic message body, got: {stderr}"
    );
}

#[test]
fn color_never_emits_no_ansi_escapes_for_workspace_diagnostic() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("never")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "expected no ANSI escape with --color never, got: {stderr:?}"
    );
    assert!(
        stderr.contains("cabin::workspace::manifest_not_found"),
        "expected diagnostic body intact, got: {stderr}"
    );
    assert!(
        stderr.contains("could not find a Cabin workspace"),
        "expected diagnostic message body, got: {stderr}"
    );
}

#[test]
fn cli_color_always_overrides_env_never() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("always")
        .env("CABIN_TERM_COLOR", "never")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(ESC),
        "CLI --color always must override env CABIN_TERM_COLOR=never, got: {stderr:?}"
    );
}

#[test]
fn cli_color_never_overrides_env_always() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("never")
        .env("CABIN_TERM_COLOR", "always")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "CLI --color never must override env CABIN_TERM_COLOR=always, got: {stderr:?}"
    );
}

#[test]
fn env_always_applies_when_cli_omitted() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .env("CABIN_TERM_COLOR", "always")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(ESC),
        "env CABIN_TERM_COLOR=always must apply when --color is omitted, got: {stderr:?}"
    );
}

#[test]
fn env_never_applies_when_cli_omitted() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .env("CABIN_TERM_COLOR", "never")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "env CABIN_TERM_COLOR=never must apply when --color is omitted, got: {stderr:?}"
    );
}

#[test]
fn metadata_json_output_remains_uncolored_with_color_always() {
    // Set up a real, parseable workspace so `cabin metadata`
    // emits its JSON document. The JSON path writes only
    // serde-formatted bytes to stdout and never touches the
    // diagnostic renderer, so even with `--color always`
    // the captured stdout must contain no ANSI escape.
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(VALID_MANIFEST)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["metadata", "--color", "always"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        !stdout.contains(ESC),
        "machine-readable JSON must not be colorised, got: {stdout:?}"
    );
    // Sanity-check it actually is JSON.
    assert!(stdout.trim_start().starts_with('{'), "got: {stdout:?}");
}

#[test]
fn top_level_help_advertises_color_option() {
    let assertion = cabin().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("--color"),
        "top-level --help should mention --color, got: {stdout}"
    );
    assert!(
        stdout.contains("Coloring: auto, always, never"),
        "top-level --help should carry our help text, got: {stdout}"
    );
    assert!(
        stdout.contains("auto") && stdout.contains("always") && stdout.contains("never"),
        "top-level --help should list the value set, got: {stdout}"
    );
}

#[test]
fn subcommand_help_inherits_global_color_option() {
    let assertion = cabin().args(["build", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("--color"),
        "subcommand --help should expose the global --color, got: {stdout}"
    );
}

#[test]
fn config_term_color_always_applies_when_cli_and_env_silent() {
    // Drop a user-level `[term] color = "always"` into a
    // throw-away `CABIN_CONFIG_HOME`. The default test
    // helper sets `CABIN_TERM_COLOR=never`, so for this
    // test we explicitly remove the env var (and
    // `CABIN_NO_CONFIG`, which the helper otherwise sets
    // to `1` to isolate other tests from a developer's
    // own config). The expected behavior is that
    // discovery picks up the file, the resolver sees
    // config=always with no env or CLI override, and the
    // diagnostic renderer paints the output.
    let dir = TempDir::new().unwrap();
    let cfg_home = TempDir::new().unwrap();
    cfg_home
        .child("config.toml")
        .write_str("[term]\ncolor = \"always\"\n")
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .arg("metadata")
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_TERM_COLOR")
        .env("CABIN_CONFIG_HOME", cfg_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(ESC),
        "config term.color=always should color output when CLI+env are silent, got: {stderr:?}"
    );
}

#[test]
fn cli_color_overrides_config_term_color_always() {
    // CLI must beat a user-level config that says
    // `always`.
    let dir = TempDir::new().unwrap();
    let cfg_home = TempDir::new().unwrap();
    cfg_home
        .child("config.toml")
        .write_str("[term]\ncolor = \"always\"\n")
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["--color", "never", "metadata"])
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_TERM_COLOR")
        .env("CABIN_CONFIG_HOME", cfg_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "--color never must override config term.color=always, got: {stderr:?}"
    );
}

#[test]
fn env_term_color_overrides_config_term_color() {
    // Config says `always`, env says `never`. Env wins.
    let dir = TempDir::new().unwrap();
    let cfg_home = TempDir::new().unwrap();
    cfg_home
        .child("config.toml")
        .write_str("[term]\ncolor = \"always\"\n")
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .arg("metadata")
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_TERM_COLOR", "never")
        .env("CABIN_CONFIG_HOME", cfg_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "CABIN_TERM_COLOR=never must override config term.color=always, got: {stderr:?}"
    );
}

#[test]
fn metadata_json_stderr_remains_uncolored_with_color_always() {
    // Companion to `metadata_json_output_remains_uncolored…`:
    // the JSON success path should not emit anything
    // colored on stderr either, even when `--color always`
    // forces color for any diagnostics.
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(VALID_MANIFEST)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["metadata", "--color", "always"])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains(ESC),
        "successful metadata run must leave stderr clean, got: {stderr:?}"
    );
}

#[test]
fn cli_color_always_paints_help_lead_in() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .arg("--color")
        .arg("always")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    // The diagnostic carries a `help:` line; under
    // `--color always` it must be wrapped in ANSI styling
    // (escape just before the `help:` token).
    let help_idx = stderr
        .find("help:")
        .expect("manifest_not_found diagnostic should include a help line");
    let leading = &stderr[..help_idx];
    assert!(
        leading.contains(ESC),
        "expected ANSI sequence before `help:`, got: {stderr:?}"
    );
}
