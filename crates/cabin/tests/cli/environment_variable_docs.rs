#[test]
fn environment_variables_doc_lists_terminal_env_controls() {
    let docs = include_str!("../../../../docs/environment-variables.md");
    for name in [
        cabin_env::CABIN_TERM_COLOR,
        cabin_env::CABIN_TERM_VERBOSE,
        cabin_env::CABIN_TERM_QUIET,
    ] {
        assert!(
            docs.contains(&format!("`{name}`")),
            "docs/environment-variables.md must list `{name}` because Cabin reads it"
        );
    }
}
