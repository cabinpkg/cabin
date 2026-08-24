#[test]
fn installation_docs_list_the_c_compiler_slot() {
    let docs = include_str!("../../../../docs/installation.md");
    assert!(
        docs.contains("C compiler") && docs.contains("`cc`, `clang`, `gcc`"),
        "installation docs must list the separate C compiler requirement for selected `.c` sources"
    );
}

#[test]
fn install_source_docs_do_not_describe_runtime_tools_as_cxx_only() {
    let docs = include_str!("../../../../INSTALL.md");
    assert!(
        docs.contains("C/C++ toolchains"),
        "source install docs must not point users at runtime requirements as C++-only"
    );
}

#[test]
fn toolchain_crate_description_mentions_c_and_cxx() {
    let manifest = include_str!("../../../../crates/cabin-toolchain/Cargo.toml");
    assert!(
        manifest.contains("C/C++ toolchain"),
        "cabin-toolchain crate metadata should not describe the crate as C++-only: {manifest}"
    );
}
