//! Build-output garbage collection against real tools: what a build
//! collects on its own, what `cabin clean --gc` collects, and how both
//! cooperate with a process using the build directory.

use std::io::{BufRead as _, BufReader, Read as _};
use std::process::Stdio;

use cabin_build::gc::BuildDirLock;

use super::*;

fn write(dir: &Path, path: &str, body: &str) {
    assert_fs::fixture::ChildPath::new(dir.join(path))
        .write_str(body)
        .unwrap();
}

fn cabin_in(dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    cabin_with_config()
        .args(args)
        .arg("--manifest-path")
        .arg(dir.join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.join("build"))
        .assert()
}

/// A C++ program calling into C, both built from `sources`.
fn write_project(dir: &Path, cxx: &str, c: &str) {
    write(
        dir,
        "cabin.toml",
        &format!(
            r#"[package]
name = "demo"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.demo]
type = "executable"
sources = ["src/{cxx}", "src/{c}"]
"#
        ),
    );
    write(
        dir,
        &format!("src/{cxx}"),
        "#include <cstdio>\nextern \"C\" int helper(void);\nint main() {\n  std::puts(\"demo-ran\");\n  return helper();\n}\n",
    );
    write(dir, &format!("src/{c}"), "int helper(void) { return 0; }\n");
}

/// Build, then rename both sources so their objects leave the plan.
fn build_then_rename(dir: &Path) {
    write_project(dir, "main.cc", "helper.c");
    cabin_in(dir, &["build"]).success();
    fs::remove_file(dir.join("src/main.cc")).unwrap();
    fs::remove_file(dir.join("src/helper.c")).unwrap();
    write_project(dir, "app.cc", "util.c");
}

fn object(dir: &Path, source: &str) -> PathBuf {
    dir.join("build/dev/packages/demo/obj/demo/src")
        .join(format!("{source}.{}", host_obj_ext()))
}

/// Advance the build directory's clock by `days` active days on which
/// nothing was used, as if other projects' builds had shared it.
fn pass_active_days(dir: &Path, days: u64) {
    let today = cabin_build::gc::today();
    for day in 1..=days {
        cabin_build::gc::record_use(
            &dir.join("build"),
            std::iter::empty::<&camino::Utf8Path>(),
            today + day,
        )
        .unwrap();
    }
}

#[test]
fn build_collects_outputs_unused_for_over_max_age() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    build_then_rename(dir);
    write(dir, "build/notes.txt", "mine");

    pass_active_days(dir, 30);
    cabin_in(dir, &["build"]).success();
    assert!(
        object(dir, "main.cc").exists() && object(dir, "helper.c").exists(),
        "30 active days is not over the default max-age"
    );

    pass_active_days(dir, 1);
    cabin_in(dir, &["build"])
        .success()
        .stdout(predicate::str::contains("unused for over 30 active days"));
    assert!(!object(dir, "main.cc").exists());
    assert!(!object(dir, "helper.c").exists());
    assert!(object(dir, "app.cc").exists() && object(dir, "util.c").exists());
    assert!(dir.join("build/notes.txt").exists(), "untracked files stay");

    cabin_in(dir, &["-v", "build"])
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));
}

#[test]
fn a_build_dir_spelled_with_dot_dot_is_collected() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    fs::create_dir(dir.join("sub")).unwrap();
    let build = || {
        cabin_with_config()
            .args(["build", "--manifest-path"])
            .arg(dir.join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.join("sub/../build"))
            .assert()
            .success();
    };
    write_project(dir, "main.cc", "helper.c");
    build();
    fs::remove_file(dir.join("src/main.cc")).unwrap();
    fs::remove_file(dir.join("src/helper.c")).unwrap();
    write_project(dir, "app.cc", "util.c");
    pass_active_days(dir, 31);
    build();
    assert!(!object(dir, "main.cc").exists());
    assert!(object(dir, "app.cc").exists());
}

#[test]
fn auto_false_leaves_collection_to_clean_gc() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write(dir, ".cabin/config.toml", "[gc]\nauto = false\n");
    build_then_rename(dir);
    pass_active_days(dir, 31);

    cabin_in(dir, &["build"]).success();
    assert!(object(dir, "main.cc").exists());

    let before = dir_files(&dir.join("build"));
    cabin_in(dir, &["clean", "--gc", "--dry-run"])
        .success()
        .stdout(predicate::str::contains("(dry-run"))
        .stdout(predicate::str::contains(host_path("src/main.cc.")));
    assert_eq!(
        dir_files(&dir.join("build")),
        before,
        "dry-run changes nothing"
    );

    cabin_in(dir, &["clean", "--gc"]).success();
    let mut removed: Vec<_> = before
        .difference(&dir_files(&dir.join("build")))
        .cloned()
        .collect();
    removed.retain(|path| path.extension().is_some_and(|ext| ext == host_obj_ext()));
    assert_eq!(
        removed,
        [object(dir, "helper.c"), object(dir, "main.cc")],
        "the real run removes the objects the dry run listed"
    );
}

#[test]
fn clean_gc_flags_override_the_policy() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    build_then_rename(dir);
    pass_active_days(dir, 3);
    // The rebuild marks the renamed sources' objects used today; the
    // old objects were last used three active days ago.
    cabin_in(dir, &["build"]).success();

    cabin_in(dir, &["clean", "--gc", "--max-age", "3"]).success();
    assert!(object(dir, "main.cc").exists());
    cabin_in(dir, &["clean", "--gc", "--max-age", "2"]).success();
    assert!(!object(dir, "main.cc").exists());

    // Recently used outputs survive any size budget, with a warning.
    cabin_in(dir, &["clean", "--gc", "--max-size", "0"])
        .success()
        .stderr(predicate::str::contains("more than the 0 B size budget"));
    assert!(object(dir, "app.cc").exists());
}

#[test]
fn collection_waits_for_processes_using_the_build_dir() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    build_then_rename(dir);
    pass_active_days(dir, 31);

    // Stands in for a `cabin run` or `cabin test` still using outputs.
    let lock = BuildDirLock::shared(&dir.join("build"), || {}).unwrap();
    cabin_in(dir, &["build", "-v"])
        .success()
        .stdout(predicate::str::contains("in use by another process"));
    assert!(object(dir, "main.cc").exists());

    let mut clean = spawn_cabin(dir, &["clean", "--gc"]);
    let mut stdout = BufReader::new(clean.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("Blocking"), "{line}");
    assert!(object(dir, "main.cc").exists());

    drop(lock);
    stdout.read_to_string(&mut line).unwrap();
    assert!(clean.wait().unwrap().success(), "{line}");
    assert!(!object(dir, "main.cc").exists());
}

#[test]
fn run_collects_after_the_program_exits() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    build_then_rename(dir);
    pass_active_days(dir, 31);
    let assert = cabin_in(dir, &["run"]).success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let ran = stdout.find("demo-ran").expect("the program ran");
    let removed = stdout
        .find("Removed")
        .expect("unused outputs were collected");
    assert!(ran < removed, "{stdout}");
    assert!(!object(dir, "main.cc").exists());
}

#[test]
fn run_holds_the_lock_until_the_program_exits() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "cabin.toml",
        r#"[package]
name = "wait"
version = "0.1.0"
c-standard = "c11"

[target.wait]
type = "executable"
sources = ["src/main.c"]
"#,
    );
    // Says it runs, then waits for its input to close.
    write(
        dir,
        "src/main.c",
        "#include <stdio.h>\nint main(void) {\n  puts(\"ready\");\n  fflush(stdout);\n  while (getchar() != EOF) {}\n  return 0;\n}\n",
    );
    let mut run = spawn_cabin(dir, &["run"]);
    let mut stdout = BufReader::new(run.stdout.take().unwrap());
    let mut line = String::new();
    while line.trim_end() != "ready" {
        line.clear();
        assert_ne!(
            stdout.read_line(&mut line).unwrap(),
            0,
            "the program never ran"
        );
    }
    let lock = fs::File::open(dir.join("build/.cabin-build.lock")).unwrap();
    assert!(lock.try_lock().is_err(), "held while the program runs");

    drop(run.stdin.take());
    assert!(run.wait().unwrap().success());
    lock.try_lock().unwrap();
}

/// `cabin` as a child the test talks to while it runs, isolated as
/// [`cabin()`] isolates it.  Its stderr goes to the test's own.
pub(super) fn spawn_cabin(dir: &Path, args: &[&str]) -> std::process::Child {
    let template = cabin();
    let mut cmd = std::process::Command::new(template.get_program());
    for (key, value) in template.get_envs() {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }
    cmd.args(args)
        .arg("--manifest-path")
        .arg(dir.join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.join("build"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Every file under `dir`, recursively.
fn dir_files(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
    let mut files = std::collections::BTreeSet::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path);
            }
        }
    }
    files
}
