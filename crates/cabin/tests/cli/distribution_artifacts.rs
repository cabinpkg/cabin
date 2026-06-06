use super::*;

#[test]
fn compgen_bash_writes_completion_script_to_stdout() {
    let output = cabin()
        .args(["compgen", "bash"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.is_empty(), "bash completion should be non-empty");
    assert!(stdout.contains("cabin"), "bash completion mentions cabin");
    // Generated completions list every visible subcommand; pick a
    // few stable ones so the assertion is not brittle.
    assert!(stdout.contains("build"));
    assert!(stdout.contains("package"));
}

#[test]
fn compgen_zsh_writes_completion_script_to_stdout() {
    let output = cabin()
        .args(["compgen", "zsh"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.is_empty(), "zsh completion should be non-empty");
    // Zsh completion either mentions the binary or the standard
    // `_cabin` function name.
    assert!(
        stdout.contains("cabin") || stdout.contains("_cabin"),
        "expected zsh script to reference cabin or _cabin"
    );
}

#[test]
fn compgen_all_writes_files_for_every_supported_shell() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("completions");
    cabin()
        .args(["compgen", "--all", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    for filename in &[
        "cabin.bash",
        "_cabin",
        "cabin.fish",
        "cabin.ps1",
        "cabin.elv",
    ] {
        let path = out.join(filename);
        assert!(path.is_file(), "expected {filename} to exist at {path:?}");
        let body = fs::read(&path).unwrap();
        assert!(!body.is_empty(), "{filename} must be non-empty");
    }
}

#[test]
fn compgen_all_without_output_dir_fails_clearly() {
    cabin()
        .args(["compgen", "--all"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output-dir"))
        .stderr(predicate::str::contains("--all"));
}

#[test]
fn compgen_invalid_shell_fails() {
    cabin()
        .args(["compgen", "klingon"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("klingon"));
}

#[test]
fn compgen_writes_single_shell_to_output_dir() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("completions");
    cabin()
        .args(["compgen", "fish", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let path = out.join("cabin.fish");
    assert!(path.is_file());
    let body = fs::read(&path).unwrap();
    assert!(!body.is_empty());
    // Other shells should NOT have been written for a single-shell
    // invocation.
    assert!(!out.join("cabin.bash").exists());
}

#[test]
fn mangen_writes_root_man_page_to_stdout() {
    let output = cabin()
        .args(["mangen"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.is_empty(), "man page should be non-empty");
    assert!(
        stdout.contains(".TH"),
        "stdout should look like a ROFF man page"
    );
    assert!(stdout.contains("cabin"));
}

#[test]
fn mangen_writes_root_and_subcommand_pages_to_output_dir() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("man");
    cabin()
        .args(["mangen", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let root = out.join("cabin.1");
    assert!(root.is_file(), "expected cabin.1 at {root:?}");
    let root_body = fs::read_to_string(&root).unwrap();
    assert!(!root_body.is_empty());

    // Every top-level subcommand — including the ones
    // hidden from `cabin --help` — gets its own
    // `cabin-<sub>.1` page. The expected list is derived
    // from clap so adding a subcommand updates this test
    // automatically.
    for sub in all_subcommand_names() {
        let path = out.join(format!("cabin-{sub}.1"));
        assert!(path.is_file(), "expected cabin-{sub}.1");
        let body = fs::read(&path).unwrap();
        assert!(!body.is_empty(), "cabin-{sub}.1 must be non-empty");
    }
}

#[test]
fn mangen_root_page_mentions_known_subcommands() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("man");
    cabin()
        .args(["mangen", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let body = fs::read_to_string(out.join("cabin.1")).unwrap();
    assert!(body.contains("build"));
    assert!(body.contains("package"));
}
