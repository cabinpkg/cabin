//! Target names, and the outputs the build derives from them, never let
//! two targets share a file.  The name rules themselves are unit-tested
//! in `cabin-core`.

use super::*;

fn manifest(package: &str, targets: &[&str]) -> String {
    let header =
        format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n");
    targets.iter().fold(header, |text, name| {
        text + &format!(
            "\n[target.\"{name}\"]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n"
        )
    })
}

#[test]
fn targets_one_letter_case_apart_are_rejected_in_any_member() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str("[workspace]\nmembers = [\"a\", \"b\"]\n")
        .unwrap();
    dir.child("a/cabin.toml")
        .write_str(&manifest("a", &["tool"]))
        .unwrap();
    dir.child("b/cabin.toml")
        .write_str(&manifest("b", &["Tool", "tool"]))
        .unwrap();
    for args in [&["build", "-p", "a"][..], &["metadata"]] {
        cabin()
            .current_dir(dir.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "target names \"Tool\" and \"tool\" differ only in letter case",
            ));
    }
    assert!(!dir.path().join("build").exists());
}

#[test]
fn colliding_outputs_fail_before_the_build_writes_anything() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    dir.child("cabin.toml")
        .write_str(&manifest("app", &["app"]))
        .unwrap();
    cabin()
        .current_dir(dir.path())
        .arg("build")
        .assert()
        .success();
    let profile = dir.path().join("build/dev");
    let written =
        ["build.ninja", "compile_commands.json"].map(|file| fs::read(profile.join(file)).unwrap());

    // Listing the source twice compiles both into one object.
    dir.child("cabin.toml")
        .write_str(
            &manifest("app", &["app"])
                .replace("[\"src/main.c\"]", "[\"src/main.c\", \"./src/main.c\"]"),
        )
        .unwrap();
    for command in ["build", "check", "run"] {
        cabin()
            .current_dir(dir.path())
            .arg(command)
            .assert()
            .failure()
            .stderr(predicate::str::contains("collide"))
            .stderr(predicate::str::contains(host_path("obj/app/src/main.c.")));
    }
    let after =
        ["build.ninja", "compile_commands.json"].map(|file| fs::read(profile.join(file)).unwrap());
    assert_eq!(written, after);
}
