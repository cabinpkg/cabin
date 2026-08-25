//! Behavioral coverage for the curated help and exhaustive command list.

use super::*;

fn run_ok(args: &[&str]) -> String {
    let assertion = cabin().args(args).assert().success();
    String::from_utf8(assertion.get_output().stdout.clone()).expect("stdout should be utf-8")
}

#[test]
fn help_omits_hidden_subcommands() {
    let listed = parse_help_commands_block(&run_ok(&["--help"]));
    for subcommand in hidden_subcommand_names() {
        assert!(!listed.contains(&subcommand), "help listed {subcommand}");
    }
}

#[test]
fn list_includes_every_subcommand() {
    let output = run_ok(&["--list"]);
    for subcommand in all_subcommand_names() {
        assert!(output.contains(&subcommand), "list omitted {subcommand}");
    }
}

#[test]
fn hidden_subcommands_remain_in_shell_completions() {
    let output = run_ok(&["compgen", "bash"]);
    for subcommand in hidden_subcommand_names() {
        assert!(
            output.contains(&subcommand),
            "completion omitted hidden command {subcommand}"
        );
    }
}

#[test]
fn hidden_subcommands_still_get_man_pages() {
    let dir = TempDir::new().unwrap();
    let output_dir = dir.path().join("man");
    cabin()
        .args(["mangen", "--output-dir"])
        .arg(&output_dir)
        .assert()
        .success();
    for subcommand in hidden_subcommand_names() {
        let page = output_dir.join(format!("cabin-{subcommand}.1"));
        assert!(page.is_file(), "missing man page {}", page.display());
        assert!(page.metadata().unwrap().len() > 0, "empty man page");
    }
}

#[test]
fn no_arguments_prints_curated_help() {
    let output = run_ok(&[]);
    assert!(output.contains("Usage: cabin"));
    let listed = parse_help_commands_block(&output);
    for subcommand in hidden_subcommand_names() {
        assert!(!listed.contains(&subcommand));
    }
}

#[test]
fn help_ends_with_the_command_list_hint() {
    let listed = parse_help_commands_block(&run_ok(&["--help"]));
    assert_eq!(listed.last().map(String::as_str), Some("..."));
}

#[test]
fn dots_are_a_shortcut_for_the_command_list() {
    assert_eq!(run_ok(&["..."]), run_ok(&["--list"]));
}
