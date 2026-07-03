use super::*;

/// Set up a single-package package at `dir` with a fully
/// populated build directory layout that mirrors what `cabin
/// build` would produce (`<build>/<profile>/{build.ninja,
/// packages/<pkg>/..., cargo/<pkg>/...}`).
fn populate_project(dir: &Path) {
    write_hello_project(dir);
    assert_fs::fixture::ChildPath::new(dir.join("cabin.lock"))
        .write_str("# lock\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("vendor/.keep"))
        .write_str("")
        .unwrap();
    assert_fs::fixture::ChildPath::new(
        dir.join("build")
            .join("dev")
            .join("packages")
            .join("hello")
            .join("hello"),
    )
    .write_str("obj")
    .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("build").join("dev").join("build.ninja"))
        .write_str("ninja")
        .unwrap();
    assert_fs::fixture::ChildPath::new(
        dir.join("build")
            .join("release")
            .join("packages")
            .join("hello")
            .join("hello"),
    )
    .write_str("obj")
    .unwrap();
}

#[test]
fn clean_removes_build_dir() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed"));

    assert!(
        !dir.path().join("build").exists(),
        "build dir should be gone"
    );
    assert!(dir.path().join("cabin.toml").exists(), "manifest preserved");
    assert!(
        dir.path().join("src").join("main.cc").exists(),
        "src preserved"
    );
    assert!(dir.path().join("cabin.lock").exists(), "lockfile preserved");
    assert!(dir.path().join("vendor").exists(), "vendor preserved");
}

#[test]
fn clean_succeeds_when_build_dir_missing() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean"])
        .assert()
        .success()
        .stdout(predicate::str::contains("does not exist"));
}

#[test]
fn clean_dry_run_lists_paths_and_keeps_files() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());

    let output = cabin()
        .current_dir(dir.path())
        .args(["clean", "--dry-run"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dry-run"));
    assert!(stdout.contains("Removed"));
    assert!(stdout.contains(&host_path("/build")));

    assert!(
        dir.path().join("build").exists(),
        "dry run must not delete files"
    );
}

#[test]
fn clean_profile_narrows_to_one_profile_dir() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--profile", "release"])
        .assert()
        .success();

    assert!(
        !dir.path().join("build").join("release").exists(),
        "release tree should be gone"
    );
    assert!(
        dir.path().join("build").join("dev").exists(),
        "dev tree must remain"
    );
}

#[test]
fn clean_release_alias_matches_profile_release() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--release"])
        .assert()
        .success();
    assert!(!dir.path().join("build").join("release").exists());
    assert!(dir.path().join("build").join("dev").exists());
}

#[test]
fn clean_package_targets_per_package_paths_across_profiles() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());
    // Add a sibling package output that must survive a -p
    // selection that names only `hello`.
    assert_fs::fixture::ChildPath::new(
        dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("other")
            .join("libother.a"),
    )
    .write_str("obj")
    .unwrap();

    cabin()
        .current_dir(dir.path())
        .args(["clean", "-p", "hello"])
        .assert()
        .success();

    assert!(
        !dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("hello")
            .exists()
    );
    assert!(
        !dir.path()
            .join("build")
            .join("release")
            .join("packages")
            .join("hello")
            .exists()
    );
    assert!(
        dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("other")
            .exists(),
        "non-selected package output must remain"
    );
    assert!(
        dir.path()
            .join("build")
            .join("dev")
            .join("build.ninja")
            .exists(),
        "profile-level files must remain"
    );
}

#[test]
fn clean_profile_and_package_combine() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--profile", "dev", "-p", "hello"])
        .assert()
        .success();

    assert!(
        !dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("hello")
            .exists()
    );
    assert!(
        dir.path()
            .join("build")
            .join("release")
            .join("packages")
            .join("hello")
            .exists(),
        "release tree untouched when profile narrowed to dev"
    );
}

#[test]
fn clean_respects_custom_build_dir() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let custom = dir.path().join("custom-build-dir");
    assert_fs::fixture::ChildPath::new(custom.join("dev").join("build.ninja"))
        .write_str("x")
        .unwrap();

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--build-dir"])
        .arg(&custom)
        .assert()
        .success();

    assert!(!custom.exists(), "custom build dir should be removed");
    // Default `build/` was never created and must remain absent.
    assert!(!dir.path().join("build").exists());
}

#[test]
fn clean_rejects_build_dir_that_contains_source_files() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--build-dir", "src"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("source file"));

    assert!(
        dir.path().join("src").join("main.cc").exists(),
        "source file must not be removed"
    );
}

#[test]
fn clean_rejects_workspace_root_as_build_dir() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--build-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to clean"));
    assert!(dir.path().join("cabin.toml").exists());
    assert!(dir.path().join("src").join("main.cc").exists());
}

/// Set up a two-member workspace at `dir` with build-tree
/// fixtures for both members under `dev/` and `release/`.
fn populate_workspace(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str("[workspace]\nmembers = [\"hello\", \"util\"]\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("hello").join("cabin.toml"))
        .write_str(
            "[package]\n\
             name = \"hello\"\n\
             version = \"0.1.0\"\n\
             cxx-standard = \"c++17\"\n\
             \n\
             [target.hello]\n\
             type = \"executable\"\n\
             sources = [\"src/main.cc\"]\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("hello").join("src").join("main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("util").join("cabin.toml"))
        .write_str(
            "[package]\n\
             name = \"util\"\n\
             version = \"0.1.0\"\n\
             cxx-standard = \"c++17\"\n\
             \n\
             [target.util]\n\
             type = \"library\"\n\
             sources = [\"src/util.cc\"]\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("util").join("src").join("util.cc"))
        .write_str("void util(){}\n")
        .unwrap();
    for profile in ["dev", "release"] {
        for pkg in ["hello", "util"] {
            assert_fs::fixture::ChildPath::new(
                dir.join("build")
                    .join(profile)
                    .join("packages")
                    .join(pkg)
                    .join("artifact"),
            )
            .write_str("x")
            .unwrap();
        }
    }
}

#[test]
fn clean_workspace_with_exclude_skips_excluded_package() {
    let dir = TempDir::new().unwrap();
    populate_workspace(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--workspace", "--exclude", "hello"])
        .assert()
        .success();

    for profile in ["dev", "release"] {
        assert!(
            dir.path()
                .join("build")
                .join(profile)
                .join("packages")
                .join("hello")
                .exists(),
            "excluded `hello` output must remain ({profile})"
        );
        assert!(
            !dir.path()
                .join("build")
                .join(profile)
                .join("packages")
                .join("util")
                .exists(),
            "non-excluded `util` output should be removed ({profile})"
        );
    }
}

#[test]
fn clean_rejects_root_path() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());

    cabin()
        .current_dir(dir.path())
        .args(["clean", "--build-dir", "/"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("root path"));
}

#[cfg(unix)]
#[test]
fn clean_rejects_symlink_build_dir() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let real = dir.path().join("real-build");
    let link = dir.path().join("build");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_fs::fixture::ChildPath::new(real.join("dev").join("build.ninja"))
        .write_str("x")
        .unwrap();

    cabin()
        .current_dir(dir.path())
        .args(["clean"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));

    assert!(real.exists(), "real build dir untouched");
    assert!(link.exists(), "symlink itself untouched");
}

#[test]
fn clean_dry_run_output_is_sorted_and_deterministic() {
    let dir = TempDir::new().unwrap();
    populate_project(dir.path());
    assert_fs::fixture::ChildPath::new(
        dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("zeta")
            .join("libzeta.a"),
    )
    .write_str("x")
    .unwrap();
    assert_fs::fixture::ChildPath::new(
        dir.path()
            .join("build")
            .join("dev")
            .join("packages")
            .join("alpha")
            .join("libalpha.a"),
    )
    .write_str("x")
    .unwrap();

    let stdout = capture_dry_run(dir.path(), &["clean", "--dry-run"]);
    let stdout_again = capture_dry_run(dir.path(), &["clean", "--dry-run"]);
    assert_eq!(stdout, stdout_again, "dry-run output must be deterministic");
}

#[test]
fn clean_help_describes_dry_run_and_profile() {
    let stdout = cabin()
        .args(["clean", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let body = String::from_utf8(stdout.stdout).unwrap();
    for needle in ["--dry-run", "--profile", "--build-dir", "--package"] {
        assert!(
            body.contains(needle),
            "clean --help missing `{needle}`:\n{body}"
        );
    }
}

fn capture_dry_run(cwd: &Path, args: &[&str]) -> String {
    let output = cabin()
        .current_dir(cwd)
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    String::from_utf8(output.stdout).unwrap()
}
