//! Contract pinning the curation between `cabin --help` and
//! `cabin --list`.
//!
//! `cabin --help` is the curated, day-to-day surface: it
//! advertises the commands aimed at normal users.  The
//! advanced and distribution-helper subcommands are hidden from
//! `--help` so the block stays short and skimmable, and
//! surface through `cabin --list` instead.  Hidden commands still
//! run normally, still
//! produce shell completions, and still ship per-command
//! man pages - only the help listing is curated.

use super::*;

fn run_ok(args: &[&str]) -> String {
    let assertion = cabin().args(args).assert().success();
    String::from_utf8(assertion.get_output().stdout.clone()).expect("stdout should be utf-8")
}

#[test]
fn help_does_not_list_hidden_subcommands_in_commands_block() {
    let out = run_ok(&["--help"]);
    // Words like "package" and "version" also appear in
    // command descriptions ("Create a new cabin package
    // …"), so a naive `contains` check is too loose.
    // Parse the Commands block into row names and assert
    // against the structured set.
    let listed = parse_help_commands_block(&out);
    for sub in hidden_subcommand_names() {
        assert!(
            !listed.contains(&sub),
            "`--help` Commands block must not list hidden `{sub}`. Listed: {listed:?}"
        );
    }
}

#[test]
fn list_includes_every_subcommand_including_hidden() {
    let out = run_ok(&["--list"]);
    // Every documented subcommand, hidden or not, appears
    // in `cabin --list`.  The expected set comes from clap
    // so a new subcommand is covered automatically.  Order
    // is alphabetical and deterministic - see the
    // dedicated test below.
    for sub in all_subcommand_names() {
        assert!(out.contains(&sub), "`--list` missing `{sub}`: {out}");
    }
}

#[test]
fn list_heading_is_present() {
    let out = run_ok(&["--list"]);
    assert!(
        out.starts_with("Installed Commands:\n"),
        "`--list` must lead with the Installed Commands heading: {out}"
    );
}

#[test]
fn list_output_is_deterministic_across_runs() {
    // Two consecutive runs must produce byte-identical
    // output.  Every input is captured at compile time so
    // there is no legitimate source of non-determinism.
    let first = run_ok(&["--list"]);
    let second = run_ok(&["--list"]);
    assert_eq!(first, second);
}

#[test]
fn list_rows_are_alphabetically_sorted() {
    let out = run_ok(&["--list"]);
    let names: Vec<&str> = out
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "`--list` rows must be sorted: {names:?}");
}

#[test]
fn list_includes_help_subcommand() {
    // `cabin --list` is exhaustive and surfaces `help` so
    // users discover `cabin help <command>`; this matches
    // cargo's `cargo --list` behavior even though
    // `cabin --help` hides the `help` row to keep its
    // Commands block short.
    let out = run_ok(&["--list"]);
    let names: Vec<&str> = out
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(
        names.contains(&"help"),
        "`--list` should surface the `help` subcommand: {names:?}"
    );
}

#[test]
fn list_does_not_expose_future_or_internal_commands() {
    let out = run_ok(&["--list"]);
    // Sanity: nothing prefixed with `internal-`, `debug-`,
    // or `experimental-` belongs in user-visible surface.
    // No such command exists today; the assertion guards
    // against drift.
    for forbidden in ["internal-", "debug-", "experimental-", "step-", "TODO"] {
        assert!(
            !out.contains(forbidden),
            "`--list` exposed unexpected `{forbidden}`: {out}"
        );
    }
}

#[test]
fn hidden_subcommands_still_run_normally() {
    // Hiding is purely a help-display concern; the actual
    // subcommands still parse and execute.
    for sub in hidden_subcommand_names() {
        let _ = cabin().args([sub.as_str(), "--help"]).assert().success();
    }
}

#[test]
fn hidden_subcommands_still_appear_in_shell_completions() {
    // clap_complete walks the canonical command tree and
    // emits hidden subcommands so completions still work
    // for users who learn the name.
    let out = run_ok(&["compgen", "bash"]);
    for sub in hidden_subcommand_names() {
        assert!(
            out.contains(&sub),
            "bash completion must still know about hidden `{sub}`: {out}"
        );
    }
}

#[test]
fn hidden_subcommands_still_get_individual_man_pages() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("man");
    cabin()
        .args(["mangen", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    for sub in hidden_subcommand_names() {
        let path = out.join(format!("cabin-{sub}.1"));
        assert!(
            path.is_file(),
            "expected per-subcommand man page for hidden `{sub}` at {path:?}"
        );
        let body = std::fs::read(&path).unwrap();
        assert!(!body.is_empty(), "cabin-{sub}.1 must be non-empty");
    }
}

#[test]
fn cabin_no_args_prints_curated_help_and_exits_zero() {
    // The dispatcher hands the help printing to clap; the
    // exit code remains 0 so a CI script that runs `cabin`
    // standalone does not flap.
    let out = run_ok(&[]);
    assert!(
        out.contains("Usage: cabin"),
        "no-arg cabin should print help: {out}"
    );
    // The curated help still omits the hidden subcommands.
    let listed = parse_help_commands_block(&out);
    for sub in hidden_subcommand_names() {
        assert!(
            !listed.contains(&sub),
            "no-arg cabin help must not list hidden `{sub}`. Listed: {listed:?}"
        );
    }
}

#[test]
fn cabin_dash_dash_list_documented_in_top_level_help() {
    let out = run_ok(&["--help"]);
    assert!(
        out.contains("--list"),
        "`--help` must mention the `--list` flag: {out}"
    );
}

#[test]
fn help_renders_visible_aliases_in_cargo_style() {
    // Cargo's `cargo --help` shows `build, b` rather than
    // clap's default `[aliases: b]`.  Each visible-aliased
    // subcommand should advertise its alias inline.
    let out = run_ok(&["--help"]);
    for (canonical, alias) in [("build", "b"), ("run", "r"), ("test", "t")] {
        let pattern = format!("{canonical}, {alias}");
        assert!(
            out.contains(&pattern),
            "`--help` should advertise `{pattern}`: {out}"
        );
    }
    assert!(
        !out.contains("[aliases:"),
        "`--help` must not use clap's `[aliases: ...]` form: {out}"
    );
}

#[test]
fn aliases_route_to_their_canonical_subcommand() {
    // `cabin b`, `cabin t`, `cabin r` should parse and
    // dispatch to the canonical build / test / run
    // subcommands; `--help` against each alias should
    // therefore print the canonical command's help.
    for (alias, expected_summary) in [
        ("b", "Compile a local package and all of its dependencies"),
        ("t", "Run the tests of a local package"),
        ("r", "Run a binary of the local package"),
    ] {
        let out = run_ok(&[alias, "--help"]);
        assert!(
            out.contains(expected_summary),
            "`cabin {alias} --help` should mirror the canonical help: {out}"
        );
    }
}

#[test]
fn list_renders_visible_aliases_in_cargo_style() {
    let out = run_ok(&["--list"]);
    for (canonical, alias) in [("build", "b"), ("run", "r"), ("test", "t")] {
        let pattern = format!("{canonical}, {alias}");
        assert!(
            out.contains(&pattern),
            "`--list` should advertise `{pattern}`: {out}"
        );
    }
}

#[test]
fn help_ends_with_cargo_style_see_more_hint() {
    // The curated Commands block ends with a cargo-style
    // `... See all commands with --list` row so users
    // immediately know how to find every subcommand.  The
    // row must be the *last* visible entry - clap's
    // auto-injected `help` subcommand is hidden from the
    // listing so the cargo-style ordering is preserved.
    let out = run_ok(&["--help"]);
    let listed = parse_help_commands_block(&out);
    assert!(
        listed.contains(&"...".to_owned()),
        "`--help` Commands block must include the `...` hint row: {listed:?}"
    );
    assert_eq!(
        listed.last().map(String::as_str),
        Some("..."),
        "`...` must be the last row in the Commands block: {listed:?}"
    );
    assert!(
        !listed.contains(&"help".to_owned()),
        "auto-injected `help` row must be hidden from the curated Commands block: {listed:?}"
    );
    assert!(
        out.contains("See all commands with --list"),
        "`--help` must spell out the `--list` hint next to the `...` row: {out}"
    );
}

#[test]
fn help_subcommand_still_works_even_when_hidden() {
    // Hiding the `help` row is purely cosmetic - `cabin
    // help <subcommand>` should still surface the same
    // long help that `cabin <subcommand> --help` shows.
    let from_help_sub = run_ok(&["help", "build"]);
    let from_long_flag = run_ok(&["build", "--help"]);
    assert_eq!(
        from_help_sub, from_long_flag,
        "`cabin help build` should mirror `cabin build --help`"
    );
}

#[test]
fn cabin_dot_dot_dot_is_a_shortcut_for_list() {
    // Typing `cabin ...` should land the user on the same
    // page `cabin --list` does, so the help-block row is
    // also a working command and not a footgun.
    let from_dots = run_ok(&["..."]);
    let from_list = run_ok(&["--list"]);
    assert_eq!(
        from_dots, from_list,
        "`cabin ...` should mirror `cabin --list` output"
    );
}

#[test]
fn dots_hint_does_not_leak_into_list() {
    // The `...` row is purely a help-view affordance; it
    // must not appear in `cabin --list`.
    let out = run_ok(&["--list"]);
    let names: Vec<&str> = out
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(
        !names.contains(&"..."),
        "`--list` must not list the `...` help-only hint: {names:?}"
    );
}

#[test]
fn dots_hint_does_not_leak_into_shell_completions() {
    // Shell completions are derived from the canonical
    // clap tree (no `...` injection), so the completion
    // script must not advertise `...` as a subcommand.
    let out = run_ok(&["compgen", "bash"]);
    // The bash completion script uses
    // `tool__subcmd__<name>` markers for each subcommand;
    // assert no `...`-flavored marker appears.
    assert!(
        !out.contains("cabin__subcmd________"),
        "bash completion must not expose `...` as a subcommand: {out}"
    );
}

#[test]
fn dots_hint_does_not_get_its_own_man_page() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("man");
    cabin()
        .args(["mangen", "--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let dots_path = out.join("cabin-....1");
    assert!(
        !dots_path.exists(),
        "expected no man page for the `...` hint at {dots_path:?}"
    );
}

#[test]
fn subcommand_help_never_leaks_rustdoc_intra_doc_links() {
    // clap renders `///` doc comments verbatim, so an
    // intra-doc link like `[`BuildArgs::offline`]` appears
    // in the user-facing `--help` output as literal
    // `` [`BuildArgs::offline`] `` text.  That looks like
    // source code, not documentation.  Scan every visible
    // subcommand's `--help` plus the top-level `--help` for
    // the `[\`` opener so a future doc-comment regression
    // surfaces here rather than in the user's terminal.
    let mut surfaces: Vec<Vec<&str>> = vec![vec!["--help"]];
    let owned_help_args: Vec<[String; 2]> = visible_subcommand_names()
        .into_iter()
        .map(|name| [name, "--help".to_owned()])
        .collect();
    for pair in &owned_help_args {
        surfaces.push(vec![pair[0].as_str(), pair[1].as_str()]);
    }
    for args in surfaces {
        let assertion = cabin().args(&args).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            !stdout.contains("[`"),
            "`cabin {args:?} --help` leaks a rustdoc intra-doc link \
                 (pattern `[\\`...\\``): {stdout}"
        );
    }
}
