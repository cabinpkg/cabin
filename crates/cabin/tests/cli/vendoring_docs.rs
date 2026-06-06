#[test]
fn quickstart_does_not_advertise_generic_offline_test_after_vendor() {
    let docs = include_str!("../../../../docs/vendoring-offline.md");
    let quickstart = docs
        .split("## What `cabin vendor` produces")
        .next()
        .expect("vendoring docs should have a quickstart section");
    assert!(
        !quickstart.contains("cabin test   --offline --index-path ./vendor"),
        "the top-level vendor workflow must not imply that dev-dependency test closures are vendored: {quickstart}"
    );

    let glue = include_str!("../../src/vendor_glue.rs");
    assert!(
        !glue.contains("cabin test   --offline --index-path ./vendor"),
        "vendor_glue module docs must not advertise a generic offline test workflow"
    );
}
