//! The generator's offline half.  Packaging itself is covered by the
//! `conformance` job in `registry.yml`, which feeds the real output
//! through the server's publish validation.

/// Every file a fixture manifest names has to exist beside it, or
/// `cabin package` refuses to stage the package: the manifests and the
/// files written next to them are edited independently, and a rename on
/// one side would otherwise only surface in CI.
#[test]
fn every_file_the_manifests_name_is_written() {
    let tree = assert_fs::TempDir::new().unwrap();
    xtask_registry_fixtures::author(tree.path()).unwrap();

    let mut checked = 0;
    for package in ["nodep", "withdep", "withupstream"] {
        let directory = tree.path().join(package);
        let manifest = std::fs::read_to_string(directory.join("cabin.toml")).unwrap();
        let named = manifest.split('"').filter(|piece| {
            [".c", ".cc", ".patch"]
                .iter()
                .any(|kind| piece.ends_with(kind))
        });
        for path in named {
            assert!(
                directory.join(path).is_file(),
                "{package} names {path}, which was not written"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4, "three sources and the declared patch");
}
