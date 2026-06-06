#[test]
fn build_selection_note_uses_c_and_cxx_target_wording() {
    let docs = include_str!("../../../../docs/workspaces.md");
    assert!(
        docs.contains("plans only the C/C++ targets in the selected"),
        "workspace docs should not describe build selection as C++-only"
    );
}
