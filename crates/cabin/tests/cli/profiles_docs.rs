#[test]
fn built_in_profile_table_lists_c_and_cxx_standard_flags() {
    let docs = include_str!("../../../../docs/profiles.md");
    assert!(
        docs.contains("| Profile   | `debug` | `opt-level` | `assertions` | C compile flags"),
        "profile docs should distinguish C/C++ standard flags"
    );
    assert!(
        docs.contains("`-std=c11 -O0 -g`") && docs.contains("`-std=c++17 -O0 -g`"),
        "profile docs must show both C/C++ built-in standard flags"
    );
}
