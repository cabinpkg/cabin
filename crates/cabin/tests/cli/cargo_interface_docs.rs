#[test]
fn version_row_matches_verbose_version_fields() {
    let docs = include_str!("../../../../docs/cargo-inspired-interface.md");
    let row = docs
        .lines()
        .find(|line| line.starts_with("| `cabin version`"))
        .expect("cargo-inspired interface docs must list `cabin version`");
    for supported in ["commit", "host", "OS"] {
        assert!(
            row.contains(supported),
            "`cabin version` docs should mention the actual verbose {supported} field: {row}"
        );
    }
    for unsupported in ["rustc", "profile"] {
        assert!(
            !row.contains(unsupported),
            "`cabin version -v` does not print {unsupported}; docs row is misleading: {row}"
        );
    }
}
