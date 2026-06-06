#[test]
fn deferred_section_does_not_mark_implemented_surfaces_out_of_scope() {
    let docs = include_str!("../../../../docs/toolchains.md");
    let deferred = docs
        .split("## Deferred / out of scope")
        .nth(1)
        .expect("toolchains docs should keep a deferred section");
    for implemented in [
        "Config files (`~/.cabin/config.toml`-style overrides)",
        "patch / override / source replacement, vendoring",
    ] {
        assert!(
            !deferred.contains(implemented),
            "toolchains deferred section still lists implemented surface `{implemented}`: {deferred}"
        );
    }
}
