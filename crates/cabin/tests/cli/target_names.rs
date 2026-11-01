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

#[test]
fn sources_one_letter_case_apart_collide_only_where_the_filesystem_ignores_case() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // Where Windows can make a directory case-sensitive, the default
    // for its platform would answer wrongly.
    #[cfg(windows)]
    let _ = std::process::Command::new("fsutil.exe")
        .args(["file", "setCaseSensitiveInfo"])
        .arg(dir.path())
        .arg("enable")
        .output();
    dir.child("cabin.toml")
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\", \"src/a.c\", \"src/A.c\"]\n",
        )
        .unwrap();
    dir.child("src/main.c")
        .write_str(
            "int lower(void);\nint upper(void);\nint main(void) { return lower() + upper(); }\n",
        )
        .unwrap();
    dir.child("src/a.c")
        .write_str("int lower(void) { return 0; }\n")
        .unwrap();
    let ignores_case = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.path().join("src/A.c"))
    {
        Ok(_) => false,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => true,
        Err(err) => panic!("{err}"),
    };
    if !ignores_case {
        fs::write(
            dir.path().join("src/A.c"),
            "int upper(void) { return 0; }\n",
        )
        .unwrap();
    }
    // A build directory made empty beforehand holds nothing to look up
    // until the build puts its lock file there.
    fs::create_dir(dir.path().join("build")).unwrap();
    let assert = cabin().current_dir(dir.path()).arg("build").assert();
    if ignores_case {
        let stderr = stderr_with_wrapping_joined(&assert.failure());
        assert!(stderr.contains("differonlyinlettercase"), "{stderr}");
    } else {
        assert.success();
    }
}
