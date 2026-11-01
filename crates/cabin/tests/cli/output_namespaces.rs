//! A `--target` build and a profile never share an output directory,
//! even when the profile is named like the triple, so building or
//! cleaning one leaves the other's outputs alone.

use super::*;

/// A triple the detected C++ compiler builds for, which a `--target`
/// build accepts without a cross toolchain: the banner's target, as
/// `cabin metadata` reports it, or GCC's `-dumpmachine`.  On a Windows
/// ARM runner that is x64, the architecture its MSVC targets.
pub(super) fn build_machine_triple() -> String {
    let tmp = TempDir::new().unwrap();
    tmp.child("cabin.toml")
        .write_str("[package]\nname = \"probe\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let metadata = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(tmp.path().join("cabin.toml"))
        .assert()
        .success();
    let value: serde_json::Value = serde_json::from_slice(&metadata.get_output().stdout).unwrap();
    let cxx = &value["toolchain"]["detected"]["cxx"];
    if let Some(target) = cxx["identity"]["target"].as_str() {
        return target.to_owned();
    }
    // GCC's `--version` names no target.
    let output = std::process::Command::new(cxx["path"].as_str().unwrap())
        .arg("-dumpmachine")
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A C program and a custom profile named like `triple`.
fn write_project(dir: &Path, triple: &str) {
    let root = assert_fs::fixture::ChildPath::new(dir);
    root.child("cabin.toml")
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.app]
type = "executable"
sources = ["src/main.c"]

[profile."{triple}"]
inherits = "dev"
"#
        ))
        .unwrap();
    root.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
}

fn cabin_at(dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    cabin().current_dir(dir).args(args).assert()
}

fn assert_up_to_date(dir: &Path, args: &[&str]) {
    cabin_at(dir, &[&["-v"], args].concat())
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));
}

#[test]
fn a_profile_named_like_a_triple_builds_and_cleans_apart_from_that_triple() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let triple = &build_machine_triple();
    write_project(dir, triple);
    let exe = host_exe("app");
    let native = dir.join("build").join(triple);
    let cross_dev = dir.join("build/.targets").join(triple).join("dev");
    let cross_custom = dir.join("build/.targets").join(triple).join(triple);
    let builds: [&[&str]; 3] = [
        &["build", "--profile", triple],
        &["build", "--target", triple],
        &["build", "--target", triple, "--profile", triple],
    ];

    for args in builds {
        cabin_at(dir, args).success();
    }
    for profile_dir in [&native, &cross_dev, &cross_custom] {
        assert!(
            profile_dir.join("packages/app").join(&exe).is_file(),
            "{}",
            profile_dir.display()
        );
    }
    for args in builds {
        assert_up_to_date(dir, args);
    }

    // Cleaning the profile keeps both builds for the triple.
    cabin_at(dir, &["clean", "--profile", triple]).success();
    assert!(!native.exists());
    assert_up_to_date(dir, builds[1]);
    assert_up_to_date(dir, builds[2]);

    // Cleaning one profile of the triple keeps its other profile.
    cabin_at(dir, &["clean", "--target", triple, "--profile", triple]).success();
    assert!(!cross_custom.exists());
    assert_up_to_date(dir, builds[1]);

    // Cleaning the triple keeps the profile.
    cabin_at(dir, builds[0]).success();
    cabin_at(dir, &["clean", "--target", triple]).success();
    assert!(!dir.join("build/.targets").join(triple).exists());
    assert_up_to_date(dir, builds[0]);
}

#[test]
fn target_outputs_follow_a_custom_build_dir() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(project.join("sub")).unwrap();
    let triple = &build_machine_triple();
    write_project(&project, triple);
    let exe = host_exe("app");
    let out = tmp.path().join("out");

    // A relative `--build-dir` resolves against the current directory,
    // `..` included.
    for args in [
        ["build", "--target", triple],
        ["build", "--profile", triple],
    ] {
        cabin()
            .current_dir(project.join("sub"))
            .args(args)
            .args([
                "--manifest-path",
                "../cabin.toml",
                "--build-dir",
                "../../out",
            ])
            .assert()
            .success();
    }
    let cross = out.join(".targets").join(triple).join("dev");
    assert!(cross.join("packages/app").join(&exe).is_file());
    assert!(out.join(triple).join("packages/app").join(&exe).is_file());
    assert!(!project.join("build").exists());

    cabin()
        .current_dir(&project)
        .args(["clean", "--target", triple, "--build-dir"])
        .arg(&out)
        .assert()
        .success();
    assert!(!out.join(".targets").join(triple).exists());
    assert!(out.join(triple).join("packages/app").join(&exe).is_file());
}

#[test]
fn clean_never_removes_a_target_tree_from_before_targets_as_a_profile() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let triple = "aarch64-unknown-linux-gnu";
    write_project(dir, triple);
    // What `cabin build --target <triple>` wrote before `.targets`.
    let previous = dir.join("build").join(triple);
    let output = assert_fs::fixture::ChildPath::new(previous.join("dev/packages/app/app"));
    output.write_str("cross").unwrap();
    assert_fs::fixture::ChildPath::new(previous.join("dev/build.ninja"))
        .write_str("ninja")
        .unwrap();

    for args in [
        &["clean", "--profile", triple][..],
        &["clean", "--package", "app"][..],
    ] {
        cabin_at(dir, args)
            .failure()
            .stderr(predicate::str::contains(format!(
                "{} as profile `{triple}`",
                host_path(&format!("build/{triple}"))
            )))
            .stderr(predicate::str::contains("yourself"));
        output.assert("cross");
    }
    // Cleaning the triple reaches only its current tree.
    cabin_at(dir, &["clean", "--target", triple]).success();
    output.assert("cross");
    // Emptying the whole build directory removes it.
    cabin_at(dir, &["clean"]).success();
    assert!(!previous.exists());
}

#[test]
fn triples_two_platforms_would_alias_are_rejected() {
    let tmp = TempDir::new().unwrap();
    write_project(tmp.path(), "aarch64-unknown-linux-gnu");
    for (triple, reason) in [
        ("AArch64-unknown-linux-gnu", "uppercase"),
        ("aarch64-unknown-linux-gnu.", "ends with `.`"),
        ("aux.x-linux", "Windows device name"),
    ] {
        for command in ["build", "clean", "metadata"] {
            cabin_at(tmp.path(), &[command, "--target", triple])
                .failure()
                .stderr(predicate::str::contains(reason));
        }
    }
    assert!(!tmp.path().join("build").exists());
}

#[test]
fn old_usage_records_collect_only_the_tree_they_name() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let triple = &build_machine_triple();
    write_project(dir, triple);
    let build = dir.join("build");
    // An output `--target <triple>` wrote, and recorded, before `.targets`.
    let rel = format!(
        "{triple}/dev/packages/app/obj/app/src/old.c.{}",
        host_obj_ext()
    );
    assert_fs::fixture::ChildPath::new(build.join(&rel))
        .write_str("old")
        .unwrap();
    let today = cabin_build::gc::today();
    cabin_build::gc::record_use(
        &build,
        [camino::Utf8Path::new(&build.join(&rel).to_string_lossy())],
        today,
    )
    .unwrap();
    for day in 1..=31 {
        cabin_build::gc::record_use(&build, std::iter::empty::<&camino::Utf8Path>(), today + day)
            .unwrap();
    }

    for args in [
        &["build", "--target", triple][..],
        &["build", "--profile", triple][..],
    ] {
        cabin_with_config()
            .current_dir(dir)
            .args(args)
            .assert()
            .success();
    }
    assert!(!build.join(&rel).exists(), "an unused output is collected");
    let exe = host_exe("app");
    assert!(build.join(triple).join("packages/app").join(&exe).is_file());
    assert!(
        build
            .join(".targets")
            .join(triple)
            .join("dev/packages/app")
            .join(&exe)
            .is_file()
    );
}
