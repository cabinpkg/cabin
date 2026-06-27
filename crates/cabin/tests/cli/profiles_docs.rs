#[test]
fn built_in_profile_table_lists_c_and_cxx_standard_flags() {
    let docs = include_str!("../../../../docs/profiles.md");
    assert!(
        docs.contains("| Profile   | `debug` | `opt-level` | `assertions` | C compile flags"),
        "profile docs should distinguish C/C++ standard flags"
    );
    assert!(
        docs.contains("`-std=<c-standard> -O0 -g`")
            && docs.contains("`-std=<cxx-standard> -O0 -g`"),
        "profile docs must show both C/C++ standard-flag slots"
    );
    assert!(
        docs.contains("language-standards.md"),
        "profile docs must point at the language-standards layer"
    );
}

#[test]
fn named_target_profile_overlays_are_documented_with_boundaries() {
    let profiles = include_str!("../../../../docs/profiles.md");
    assert!(
        profiles.contains(r#"[target.'cfg(os = "linux")'.profile.release]"#),
        "profile docs must show the Linux release overlay",
    );
    assert!(
        profiles.contains("does not define a profile"),
        "profile docs must distinguish overlays from definitions",
    );
    for rejected in [
        "`inherits`",
        "`debug`",
        "`opt-level`",
        "`assertions`",
        "`toolchain`",
    ] {
        assert!(
            profiles.contains(rejected),
            "profile docs must list rejected field {rejected}",
        );
    }
    for layer in [
        "[profile]",
        "[target.'cfg(...)'.profile]",
        "[profile.<root>]",
        "[target.'cfg(...)'.profile.<root>]",
        "[profile.<child>]",
        "[target.'cfg(...)'.profile.<child>]",
    ] {
        assert!(
            profiles.contains(layer),
            "profile docs must include ordering layer {layer}",
        );
    }

    let target_dependencies = include_str!("../../../../docs/target-dependencies.md");
    assert!(
        target_dependencies.contains("`profile` is not a `cfg(...)` key"),
        "target-dependency docs must keep profile selection outside cfg predicates",
    );
}
