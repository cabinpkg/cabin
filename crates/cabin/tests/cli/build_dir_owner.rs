//! A build directory serves the one workspace that first built into
//! it: another workspace's commands refuse it before writing or
//! removing any output.

use std::io::{BufRead as _, BufReader, Read as _};
use std::process::Output;
use std::sync::Barrier;

use cabin_build::gc::BuildDirLock;

use super::*;

const FOREIGN: &str = "holdstheoutputsoftheworkspaceat";

/// A workspace at `dir` whose package `app` has an executable and a
/// test target, in C++ when `cxx` and in C otherwise.
fn write_workspace(dir: &Path, cxx: bool) {
    let root = assert_fs::fixture::ChildPath::new(dir);
    let (standard, source, body) = if cxx {
        (
            "cxx-standard = \"c++17\"",
            "src/main.cc",
            "int main() { return 0; }\n",
        )
    } else {
        (
            "c-standard = \"c11\"",
            "src/main.c",
            "int main(void) { return 0; }\n",
        )
    };
    root.child("cabin.toml")
        .write_str(&format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n{standard}\n\n[target.app]\ntype = \"executable\"\nsources = [\"{source}\"]\n\n[target.app_test]\ntype = \"test\"\nsources = [\"{source}\"]\n"
        ))
        .unwrap();
    root.child(source).write_str(body).unwrap();
}

fn cabin_in(dir: &Path, args: &[&str], build_dir: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .current_dir(dir)
        .args(args)
        .arg("--build-dir")
        .arg(build_dir)
        .assert()
}

fn joined_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace(['\r', '\n', '│', ' '], "")
}

#[test]
fn a_build_dir_serves_only_the_workspace_that_first_built_into_it() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let (c, cxx) = (tmp.path().join("c"), tmp.path().join("cxx"));
    write_workspace(&c, false);
    write_workspace(&cxx, true);
    let shared = tmp.path().join("shared");
    cabin_in(&c, &["build"], &shared).success();
    let outputs = shared.join("dev/packages/app");
    let ninja = fs::read(shared.join("dev/build.ninja")).unwrap();

    for args in [
        &["build"][..],
        &["check"],
        &["run"],
        &["test"],
        &["clean"],
        &["clean", "--dry-run"],
        &["clean", "--gc"],
    ] {
        let assert = cabin_in(&cxx, args, &shared).failure();
        let stderr = stderr_with_wrapping_joined(&assert);
        assert!(stderr.contains(FOREIGN), "{args:?}: {stderr}");
    }
    assert!(outputs.exists());
    assert_eq!(fs::read(shared.join("dev/build.ninja")).unwrap(), ninja);

    // Cleaning the outputs keeps the directory its owner's.
    cabin_in(&c, &["clean"], &shared).success();
    assert!(!outputs.exists());
    cabin_in(&cxx, &["build"], &shared).failure();
    // Removing the directory hands it over.
    fs::remove_dir_all(&shared).unwrap();
    cabin_in(&cxx, &["build"], &shared).success();
}

#[test]
fn concurrent_first_builds_let_one_workspace_claim_a_build_dir() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let (c, cxx) = (tmp.path().join("c"), tmp.path().join("cxx"));
    write_workspace(&c, false);
    write_workspace(&cxx, true);
    let race = |first: &Path, second: &Path, build_dir: &Path| -> [Output; 2] {
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            [first, second]
                .map(|workspace| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        cabin()
                            .current_dir(workspace)
                            .arg("build")
                            .arg("--build-dir")
                            .arg(build_dir)
                            .output()
                            .unwrap()
                    })
                })
                .map(|thread| thread.join().unwrap())
        })
    };
    for round in 0..3 {
        let shared = tmp.path().join(format!("shared-{round}"));
        let outputs = race(&c, &cxx, &shared);
        let failed: Vec<&Output> = outputs.iter().filter(|o| !o.status.success()).collect();
        assert_eq!(failed.len(), 1, "exactly one workspace claims it");
        assert!(joined_stderr(failed[0]).contains(FOREIGN));
    }
    // Two first builds of one workspace share the claim, and their Ninja
    // runs take turns.
    let shared = tmp.path().join("same");
    for output in race(&c, &c, &shared) {
        assert!(output.status.success(), "{}", joined_stderr(&output));
    }
}

#[test]
fn a_clean_that_waited_for_the_lock_refuses_a_claim_made_meanwhile() {
    let tmp = TempDir::new().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write_workspace(&a, false);
    write_workspace(&b, false);
    let build = a.join("build");
    claim_build_dir(&build, &a);
    fs::create_dir(build.join("dev")).unwrap();
    // Stands in for a command using `a`'s outputs, during which the
    // directory is removed and a first build of `b` claims it.
    let lock = BuildDirLock::shared(&build, || {}).unwrap();
    let mut clean = super::build_gc::spawn_cabin(&a, &["clean"]);
    let mut stdout = BufReader::new(clean.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("Blocking"), "{line}");
    fs::remove_dir(build.join("dev")).unwrap();
    fs::remove_file(build.join(cabin_build::owner::OWNER_FILENAME)).unwrap();
    cabin_build::owner::claim(&build, &b).unwrap();
    fs::create_dir(build.join("dev")).unwrap();
    drop(lock);
    stdout.read_to_string(&mut line).unwrap();
    assert!(!clean.wait().unwrap().success(), "{line}");
    assert!(build.join("dev").exists());
}

#[test]
fn spellings_of_one_build_dir_share_its_owner() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let (c, cxx) = (tmp.path().join("c"), tmp.path().join("cxx"));
    write_workspace(&c, false);
    write_workspace(&cxx, true);
    cabin_in(&c, &["build"], &tmp.path().join("shared")).success();
    cabin_in(&c, &["build"], Path::new("../shared")).success();
    let assert = cabin_in(&cxx, &["build"], Path::new("../c/../shared")).failure();
    assert!(stderr_with_wrapping_joined(&assert).contains(FOREIGN));
    #[cfg(unix)]
    {
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(tmp.path().join("shared"), &link).unwrap();
        cabin_in(&c, &["build"], &link).success();
        let assert = cabin_in(&cxx, &["build"], &link).failure();
        assert!(stderr_with_wrapping_joined(&assert).contains(FOREIGN));
    }
}

#[test]
fn a_moved_workspace_keeps_only_the_build_dir_inside_it() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    write_workspace(&ws, false);
    let outside = tmp.path().join("outside");
    cabin_in(&ws, &["build"], &ws.join("build")).success();
    cabin_in(&ws, &["build"], &outside).success();
    let moved = tmp.path().join("moved");
    fs::rename(&ws, &moved).unwrap();
    cabin_in(&moved, &["build"], &moved.join("build")).success();
    let assert = cabin_in(&moved, &["build"], &outside).failure();
    assert!(stderr_with_wrapping_joined(&assert).contains(FOREIGN));
}

#[test]
fn a_build_dir_without_an_owner_record_is_left_alone() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    write_workspace(&ws, false);
    let build = ws.join("build");
    cabin_in(&ws, &["build"], &build).success();
    // As a Cabin from before the record leaves it; a directory that is
    // no build directory at all looks the same.
    let record = build.join(".cabin-workspace");
    fs::remove_file(&record).unwrap();
    for args in [
        &["build"][..],
        &["clean"],
        &["clean", "--dry-run"],
        &["clean", "--profile", "dev"],
        &["clean", "-p", "app"],
        &["clean", "--gc"],
    ] {
        let assert = cabin_in(&ws, args, &build).failure();
        let stderr = stderr_with_wrapping_joined(&assert);
        assert!(
            stderr.contains("norecordoftheworkspace"),
            "{args:?}: {stderr}"
        );
    }
    assert!(build.join("dev/packages/app").is_dir());
    fs::remove_dir_all(&build).unwrap();
    cabin_in(&ws, &["build"], &build).success();
    assert!(record.exists());
    // A directory holding only files is claimed like a new one, and a
    // clean leaves its files.
    let plain = tmp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join(".gitignore"), "*\n").unwrap();
    cabin_in(&ws, &["build"], &plain).success();
    cabin_in(&ws, &["clean"], &plain).success();
    assert!(plain.join(".gitignore").is_file());
    assert!(!plain.join("dev").exists());
}
