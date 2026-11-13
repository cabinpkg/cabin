//! The fake pkg-config the tests ask, and a package that is nothing
//! but a library of the system.

use super::*;
use std::path::PathBuf;

/// The fake pkg-config binary, built by `cabin-system-deps` under its
/// `test-fake-pkg-config` feature.
fn fake_pkg_config_path() -> PathBuf {
    workspace_test_bin("cabin-system-deps-fake-pkg-config")
}

/// TempDir of `<module>.json` files the fake pkg-config reads through
/// `CABIN_FAKE_PKG_CONFIG_FIXTURES`; `write` publishes one module's
/// metadata.
pub(super) struct Fixtures {
    dir: TempDir,
}

impl Fixtures {
    pub(super) fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    pub(super) fn write(&self, name: &str, body: &str) {
        assert_fs::fixture::ChildPath::new(self.dir.path().join(format!("{name}.json")))
            .write_str(body)
            .unwrap();
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A `cabin` command whose pkg-config is the fake one, reading module
/// metadata from `fixtures`.
pub(super) fn cabin_with_fake_pkg_config(fixtures: &Fixtures) -> Command {
    let mut cmd = cabin();
    cmd.env("CABIN_PKG_CONFIG", fake_pkg_config_path());
    cmd.env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path());
    // Poison the environment with the registry token and the Actions
    // OIDC request token: the fake pkg-config hard-fails when it sees
    // either, so every command built here enforces the child-env scrub.
    cmd.env("CABIN_REGISTRY_TOKEN", "cabin_secretToken1234");
    cmd.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "runner-oidc-secret");
    cmd
}

/// A workspace under `root/ws`.  `greet-sys` builds nothing: its
/// library `greet` is the pkg-config module `greet`, at 1.2 or newer.
/// `app` needs it for a C and a C++ program, `tested` for a test alone.
fn write_system_library_workspace(root: &Path) {
    let dir = assert_fs::fixture::ChildPath::new(root.join("ws"));
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["app", "greet-sys", "tested"]
"#,
        )
        .unwrap();
    dir.child("greet-sys/cabin.toml")
        .write_str(
            r#"[package]
name = "greet-sys"
version = "0.1.0"
interface-c-standard = "c89"
interface-cxx-standard = "c++98"

[target.greet]
type = "library"
sources = []
links = "greet"
"#,
        )
        .unwrap();
    dir.child("greet-sys/build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fputs("external greet\npkg-config greet greet >=1.2\n", file);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[dependencies]
greet-sys = { path = "../greet-sys" }

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["greet-sys"]

[target.appxx]
type = "executable"
sources = ["src/main.cc"]
deps = ["greet-sys"]
"#,
        )
        .unwrap();
    let body = r#"#include <greet.h>
int main(void) { printf("%s: %s\n", GREET_FROM, greet()); return 0; }
"#;
    dir.child("app/src/main.c")
        .write_str(&format!("#include <stdio.h>\n{body}"))
        .unwrap();
    dir.child("app/src/main.cc")
        .write_str(&format!("#include <cstdio>\n{body}"))
        .unwrap();
    dir.child("tested/cabin.toml")
        .write_str(
            r#"[package]
name = "tested"
version = "0.1.0"
c-standard = "c11"

[dev-dependencies]
greet-sys = { path = "../greet-sys" }

[target.tested]
type = "executable"
sources = ["src/main.c"]

[target.says]
type = "test"
sources = ["tests/says.c"]
deps = ["greet-sys"]
"#,
        )
        .unwrap();
    dir.child("tested/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    dir.child("tested/tests/says.c")
        .write_str(
            r#"#include <greet.h>
#include <string.h>
int main(void) { return strcmp(greet(), "one") != 0; }
"#,
        )
        .unwrap();
}

/// A package whose build script names a pkg-config module, and has no
/// source of its own, gives the packages that depend on it a library of
/// the system: its header and its file reach their C and C++ programs.
/// pkg-config is asked only by a build that reaches that package, so
/// one that needs it for a test alone builds without a pkg-config.
#[test]
fn a_package_supplies_a_library_of_the_system_to_those_that_need_it() {
    use super::build_scripts::{build_greet_sdk, printed, write_greet_module};
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_system_library_workspace(dir.path());
    let fixtures = Fixtures::new();
    let cabin_in_workspace = |pkg_config: Option<&Path>, args: &[&str]| {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        if let Some(pkg_config) = pkg_config {
            cmd.env("CABIN_PKG_CONFIG", pkg_config);
        }
        cmd.current_dir(dir.path().join("ws")).args(args);
        cmd.assert()
    };
    let absent = dir.path().join("no-pkg-config");

    cabin_in_workspace(Some(&absent), &["build", "-p", "tested"]).success();
    if cfg!(windows) {
        // The default Windows toolchain is MSVC, which takes none of
        // pkg-config's flags.
        return;
    }
    let unasked = printed(&cabin_in_workspace(Some(&absent), &["run", "--bin", "app"]).failure());
    assert!(
        unasked.contains("cabin::system_deps::executable_not_found")
            && unasked.contains("CABIN_PKG_CONFIG"),
        "{unasked}"
    );

    build_greet_sdk(dir.path(), "sdk", "one", None);
    write_greet_module(&fixtures, dir.path(), "sdk", "1.2.3");
    for bin in ["app", "appxx"] {
        let ran = printed(&cabin_in_workspace(None, &["run", "--bin", bin]).success());
        assert!(ran.contains("\nsdk: one\n"), "{ran}");
    }
    cabin_in_workspace(None, &["test", "-p", "tested"]).success();
}
