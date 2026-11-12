use std::sync::mpsc;
use std::time::Duration;

use assert_fs::TempDir;
use assert_fs::prelude::*;

use super::*;

fn package(name: &str) -> Package {
    Package {
        name: name.to_owned(),
        version: "1.0.0".to_owned(),
        source: Source::Path("src".into()),
        features: Vec::new(),
        profile: "release".to_owned(),
        toolchain: Toolchain {
            compiler: "clang".to_owned(),
            version: None,
        },
        dependencies: Vec::new(),
        binaries: Vec::new(),
    }
}

/// A built file holding `contents`, to install as `name`.
fn built(dir: &TempDir, name: &str, contents: &str) -> (String, PathBuf) {
    let file = dir.child("built").child(contents);
    file.write_str(contents).unwrap();
    (name.to_owned(), file.to_path_buf())
}

fn never_waits() {
    panic!("nothing else holds the root");
}

/// Each package with the file names it owns.
fn owners(root: &Root) -> Vec<(String, Vec<String>)> {
    root.packages()
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.binaries.into_iter().map(|b| b.name).collect()))
        .collect()
}

fn read(root: &Root, name: &str) -> String {
    fs::read_to_string(root.bin().join(name)).unwrap()
}

/// `Tool` and `tool` are one file where the file system ignores case,
/// so no root gets both.
#[test]
fn a_name_differing_only_in_case_is_refused() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    let install = |package: &str, names: &[&str]| {
        let files: Vec<_> = names.iter().map(|name| built(&dir, name, name)).collect();
        root.install(self::package(package), &files, true, never_waits)
    };
    install("a", &["Tool"]).unwrap();
    for (package, names) in [("a", &["tool"][..]), ("b", &["TOOL"]), ("c", &["x", "X"])] {
        let refused = install(package, names).unwrap_err();
        assert!(matches!(refused, Error::Case { .. }), "{refused}");
    }
    assert_eq!(owners(&root), [("a".to_owned(), vec!["Tool".to_owned()])]);
    assert_eq!(read(&root, "Tool"), "Tool");
}

#[test]
fn a_directory_in_the_way_is_never_replaced() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    let kept = root.bin().join("tool/settings");
    fs::create_dir_all(kept.parent().unwrap()).unwrap();
    fs::write(&kept, "the user's").unwrap();

    root.install(
        package("a"),
        &[built(&dir, "tool", "a's")],
        true,
        never_waits,
    )
    .unwrap_err();
    assert_eq!(fs::read_to_string(kept).unwrap(), "the user's");
}

#[test]
fn a_name_in_use_is_refused_unless_forced() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    root.install(
        package("a"),
        &[built(&dir, "tool", "a's")],
        false,
        never_waits,
    )
    .unwrap();
    fs::write(root.bin().join("stray"), "the user's").unwrap();

    let files = [
        built(&dir, "tool", "b's tool"),
        built(&dir, "stray", "b's stray"),
        built(&dir, "new", "b's new"),
    ];
    let refused = root
        .install(package("b"), &files, false, never_waits)
        .unwrap_err();
    let Error::Collision { files: taken, .. } = refused else {
        panic!("{refused}");
    };
    assert_eq!(
        taken,
        [
            ("tool".to_owned(), Some("a".to_owned())),
            ("stray".to_owned(), None)
        ]
    );
    assert_eq!(read(&root, "tool"), "a's");
    assert_eq!(read(&root, "stray"), "the user's");
    assert!(!root.bin().join("new").exists());
    assert_eq!(owners(&root), [("a".to_owned(), vec!["tool".to_owned()])]);

    let done = root
        .install(package("b"), &files, true, never_waits)
        .unwrap();
    assert_eq!(done.replaced, ["tool", "stray"]);
    assert_eq!(done.added, ["new"]);
    assert_eq!(read(&root, "tool"), "b's tool");
    assert_eq!(read(&root, "stray"), "b's stray");
    // `a` owned nothing else, so it is no longer installed.
    assert_eq!(
        owners(&root),
        [(
            "b".to_owned(),
            vec!["new".to_owned(), "stray".to_owned(), "tool".to_owned()]
        )]
    );
}

#[test]
fn installing_a_package_again_replaces_its_installation() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    let first = [
        built(&dir, "kept", "kept 1"),
        built(&dir, "dropped", "dropped 1"),
        built(&dir, "edited", "edited 1"),
    ];
    root.install(package("a"), &first, false, never_waits)
        .unwrap();
    fs::write(root.bin().join("edited"), "the user's").unwrap();
    // What a change cut short left behind.
    let leftover = root.state().join("tmp/0/old-0");
    fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    fs::write(&leftover, "stale").unwrap();

    let done = root
        .install(
            package("a"),
            &[built(&dir, "kept", "kept 2")],
            false,
            never_waits,
        )
        .unwrap();
    assert_eq!(
        done,
        Installed {
            added: Vec::new(),
            replaced: vec!["kept".to_owned()],
            removed: vec!["dropped".to_owned()],
            left: vec!["edited".to_owned()],
        }
    );
    assert_eq!(read(&root, "kept"), "kept 2");
    assert!(!root.bin().join("dropped").exists());
    assert_eq!(read(&root, "edited"), "the user's");
    assert_eq!(owners(&root), [("a".to_owned(), vec!["kept".to_owned()])]);
    assert!(!root.state().join("tmp").exists());
}

#[test]
fn uninstall_removes_only_the_files_cabin_installed() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    let files = [built(&dir, "one", "one"), built(&dir, "two", "two")];
    root.install(package("a"), &files, false, never_waits)
        .unwrap();
    fs::write(root.bin().join("two"), "the user's").unwrap();

    let refused = root.uninstall("a", &[], never_waits).unwrap_err();
    let Error::Modified(changed) = refused else {
        panic!("{refused}");
    };
    assert_eq!(changed, [root.bin().join("two")]);
    assert_eq!(read(&root, "one"), "one");
    assert_eq!(owners(&root).len(), 1);

    let unknown = root
        .uninstall("a", &["three".to_owned()], never_waits)
        .unwrap_err();
    assert!(matches!(unknown, Error::NotABinary { .. }), "{unknown}");

    let removed = root
        .uninstall("a", &["one".to_owned()], never_waits)
        .unwrap();
    assert_eq!(removed, ["one"]);
    assert!(!root.bin().join("one").exists());
    assert_eq!(owners(&root), [("a".to_owned(), vec!["two".to_owned()])]);

    // A file the user deleted is no reason to keep the package.
    fs::remove_file(root.bin().join("two")).unwrap();
    assert_eq!(root.uninstall("a", &[], never_waits).unwrap(), ["two"]);
    assert_eq!(owners(&root), []);
    let gone = root.uninstall("a", &[], never_waits).unwrap_err();
    assert!(matches!(gone, Error::NotInstalled(_)), "{gone}");
}

#[test]
fn a_change_that_fails_puts_every_file_back() {
    let dir = TempDir::new().unwrap();
    let (bin, tmp) = (dir.child("bin"), dir.child("tmp"));
    bin.child("replaced").write_str("old").unwrap();
    bin.child("removed").write_str("old").unwrap();
    tmp.child("new-0").write_str("new").unwrap();
    tmp.child("new-1").write_str("new").unwrap();

    let failed = swap(
        bin.path(),
        tmp.path(),
        &["replaced", "removed"],
        &[
            (tmp.path().join("new-0"), "replaced"),
            (tmp.path().join("new-1"), "added"),
        ],
        || {
            Err(Error::NotInstalled(
                "the record could not be written".to_owned(),
            ))
        },
    );
    assert!(failed.is_err());
    bin.child("replaced").assert("old");
    bin.child("removed").assert("old");
    bin.child("added").assert(predicates::path::missing());
}

#[test]
fn a_change_waits_for_the_one_before_it() {
    let dir = TempDir::new().unwrap();
    let root = Root::new(dir.path().join("root"));
    let file = built(&dir, "tool", "tool");
    let held = root.lock(never_waits).unwrap();
    let (waiting, waited) = mpsc::channel();
    std::thread::scope(|scope| {
        let install = scope
            .spawn(|| root.install(package("a"), &[file], false, || waiting.send(()).unwrap()));
        waited.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_eq!(owners(&root), []);
        drop(held);
        install.join().unwrap().unwrap();
    });
    assert_eq!(owners(&root), [("a".to_owned(), vec!["tool".to_owned()])]);
}
