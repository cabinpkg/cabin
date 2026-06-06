#[test]
fn config_section_does_not_mark_source_replacement_or_vendoring_out_of_scope() {
    let docs = include_str!("../../../../docs/architecture.md");
    assert!(
        !docs.contains("Auth tokens, source replacement, vendoring, and new registry"),
        "architecture docs still group implemented source replacement and vendoring with unsupported auth/protocol work"
    );
}
