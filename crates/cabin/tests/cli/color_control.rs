use super::*;

const ESC: char = '\x1b';

fn missing_manifest_command(dir: &TempDir) -> Command {
    let mut cmd = cabin();
    cmd.current_dir(dir.path()).arg("metadata");
    cmd
}

fn stderr_with_color(value: &str) -> String {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .args(["--color", value])
        .assert()
        .failure();
    String::from_utf8_lossy(&assertion.get_output().stderr).into_owned()
}

#[test]
fn unknown_cli_color_is_rejected_before_dispatch() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .args(["--color", "sometimes"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--color") && stderr.contains("sometimes"),
        "invalid color was not diagnosed: {stderr}"
    );
    assert!(
        !stderr.contains("cabin::workspace::manifest_not_found"),
        "invalid CLI color reached command dispatch: {stderr}"
    );
}

#[test]
fn invalid_environment_color_is_reported_before_dispatch() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .env("CABIN_TERM_COLOR", "sometimes")
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("CABIN_TERM_COLOR") && stderr.contains("sometimes"),
        "invalid environment color was not diagnosed: {stderr}"
    );
    assert!(
        !stderr.contains("cabin::workspace::manifest_not_found"),
        "invalid environment color reached command dispatch: {stderr}"
    );
}

#[test]
fn color_always_styles_diagnostics() {
    let stderr = stderr_with_color("always");
    assert!(
        stderr.contains(ESC),
        "expected styled diagnostic: {stderr:?}"
    );
    assert!(stderr.contains("cabin::workspace::manifest_not_found"));
}

#[test]
fn color_never_keeps_diagnostics_plain() {
    let stderr = stderr_with_color("never");
    assert!(
        !stderr.contains(ESC),
        "expected plain diagnostic: {stderr:?}"
    );
    assert!(stderr.contains("cabin::workspace::manifest_not_found"));
}

#[test]
fn cli_color_overrides_the_environment() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .args(["--color", "always"])
        .env("CABIN_TERM_COLOR", "never")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(stderr.contains(ESC), "CLI color did not win: {stderr:?}");
}

#[test]
fn environment_color_applies_without_a_cli_override() {
    let dir = TempDir::new().unwrap();
    let assertion = missing_manifest_command(&dir)
        .env("CABIN_TERM_COLOR", "always")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains(ESC),
        "environment color did not apply: {stderr:?}"
    );
}

#[test]
fn discovered_config_color_applies_without_stronger_inputs() {
    let dir = TempDir::new().unwrap();
    let config_home = TempDir::new().unwrap();
    config_home
        .child("config.toml")
        .write_str("[term]\ncolor = \"always\"\n")
        .unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .arg("metadata")
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_TERM_COLOR")
        .env("CABIN_CONFIG_HOME", config_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains(ESC),
        "config color did not apply: {stderr:?}"
    );
}

#[test]
fn machine_readable_output_stays_clean_when_color_is_forced() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["metadata", "--color", "always"])
        .assert()
        .success();
    let output = assertion.get_output();
    assert!(output.stderr.is_empty(), "metadata wrote to stderr");
    assert!(!output.stdout.contains(&(ESC as u8)));
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .expect("metadata stdout should remain valid JSON");
}
