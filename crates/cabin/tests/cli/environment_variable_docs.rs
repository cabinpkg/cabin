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

#[test]
fn environment_variables_doc_lists_build_time_version_metadata() {
    let docs = include_str!("../../../../docs/environment-variables.md");
    for name in [
        "CABIN_BUILD_COMMIT",
        "CABIN_BUILD_COMMIT_DATE",
        "CABIN_BUILD_HOST",
    ] {
        assert!(
            docs.contains(&format!("`{name}`")),
            "docs/environment-variables.md claims to list every CABIN_* variable Cabin reads, so it must list build-time `{name}`"
        );
    }
    assert!(
        docs.contains("not runtime controls"),
        "build-time metadata docs must make clear these variables are not runtime controls"
    );
}
