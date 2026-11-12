//! The `examples/build-script-sdk` workspace: a package whose build
//! script takes zlib from the system, or leaves Cabin to build the
//! sources the package carries, and a C and a C++ program that call
//! the library.
//!
//! The system's zlib of these tests is one Cabin builds from the same
//! sources under another version string.  It tells which library a
//! program compiled against and linked; it is not evidence that any
//! vendor's SDK links.

use super::build_scripts::printed;
use super::system_deps_pkg_config::{Fixtures, cabin_with_fake_pkg_config};
use super::*;

fn example_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/build-script-sdk")
}

/// A copy of the example to build in.
pub(crate) fn example() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.copy_from(example_root(), &["**"]).unwrap();
    dir
}

/// The SDK directory `name` under `root`: the headers and the library
/// of a zlib that Cabin built for `target` from the example's sources,
/// whose header and `zlibVersion()` say `version` in place of 1.3.1.
pub(crate) fn build_sdk(root: &Path, name: &str, version: &str, target: Option<&str>) -> PathBuf {
    let src = root.join(format!("{name}-src"));
    assert_fs::fixture::ChildPath::new(&src)
        .copy_from(example_root().join("packages/zlib"), &["*"])
        .unwrap();
    let header = fs::read_to_string(src.join("zlib.h")).unwrap();
    let carried = r#"#define ZLIB_VERSION "1.3.1""#;
    assert!(header.contains(carried));
    let renamed = format!(r#"#define ZLIB_VERSION "{version}""#);
    fs::write(src.join("zlib.h"), header.replace(carried, &renamed)).unwrap();

    let mut built = root.join(format!("{name}-build"));
    let mut cmd = cabin();
    cmd.current_dir(&src)
        .args(["build", "--features", "vendored", "--build-dir"])
        .arg(&built);
    if let Some(target) = target {
        cmd.args(["--target", target]);
        built.extend([".targets", target]);
    }
    cmd.assert().success();

    let sdk = root.join(name);
    fs::create_dir_all(sdk.join("include")).unwrap();
    fs::create_dir_all(sdk.join("lib")).unwrap();
    for header in ["zlib.h", "zconf.h"] {
        fs::copy(src.join(header), sdk.join("include").join(header)).unwrap();
    }
    let library = host_static_lib("z");
    fs::copy(
        built.join("dev/packages/zlib").join(&library),
        sdk.join("lib").join(&library),
    )
    .unwrap();
    sdk
}

/// `cabin run -p <package>` in the example at `dir`, with no SDK
/// directory but the one a test then names.
fn run(dir: &Path, package: &str, vendored: bool) -> Command {
    let mut cmd = cabin();
    cmd.current_dir(dir)
        .args(["run", "-p", package])
        .env_remove("CABIN_SDK_ZLIB_DIR");
    if vendored {
        cmd.args(["--features", "vendored"]);
    }
    cmd
}

/// Whether a program of the example says it compiled against and
/// linked the zlib of `version`.
fn says(output: &str, version: &str) -> bool {
    output.contains(&format!("by zlib {version} (header {version})\n"))
}

/// Publishes the pkg-config module `zlib` at `version`, which is the
/// SDK directory `sdk`.
fn write_module(fixtures: &Fixtures, sdk: &Path, version: &str) {
    let sdk = sdk.display();
    fixtures.write(
        "zlib",
        &format!(
            r#"{{
    "version": "{version}",
    "cflags": "-I{sdk}/include",
    "libs": "-L{sdk}/lib -lz"
}}"#
        ),
    );
}

/// The `vendored` feature decides between the package's sources and
/// the system's zlib, in both directions and for the C program, which
/// sees zlib only through another library's public header, as for the
/// C++ one.  The system's zlib is the one in the directory the build
/// is pointed at.  `app`'s build script links a zlib of its own, always
/// the vendored one, whichever the program links.
#[test]
fn a_feature_chooses_between_the_systems_zlib_and_the_packages_sources() {
    require_c_and_cxx_build_tools();
    let dir = example();
    let scratch = TempDir::new().unwrap();
    let sdk = build_sdk(scratch.path(), "sdk", "1.3.1-sdk", None);
    let other = build_sdk(scratch.path(), "other", "1.3.1-other", None);

    for (vendored, sdk, version) in [
        (true, &sdk, "1.3.1"),
        (false, &sdk, "1.3.1-sdk"),
        (false, &other, "1.3.1-other"),
        (true, &other, "1.3.1"),
    ] {
        for package in ["app", "appxx"] {
            let mut cmd = run(dir.path(), package, vendored);
            cmd.env("CABIN_SDK_ZLIB_DIR", sdk);
            let output = printed(&cmd.assert().success());
            assert!(says(&output, version), "{package}: {output}");
            let packed = format!("packed by zlib 1.3.1, unpacked by zlib {version} ");
            assert!(
                package != "app"
                    || output.contains("Hello from a banner packed with zlib")
                        && output.contains(&packed),
                "{output}"
            );
        }
    }
}

/// A system zlib the script cannot use fails the build, and nothing
/// builds the package's sources in its place: no header, a header older
/// than the script accepts, no library, a library for another machine,
/// and, with no directory given, no answer from pkg-config or one that
/// is too old.
#[test]
fn a_system_zlib_that_cannot_be_used_fails_the_build() {
    require_c_and_cxx_build_tools();
    let dir = example();
    let scratch = TempDir::new().unwrap();
    let sdk = build_sdk(scratch.path(), "sdk", "1.3.1-sdk", None);
    let library = Path::new("lib").join(host_static_lib("z"));
    let header = Path::new("include/zlib.h");
    let refused = |sdk: &Path, phrase: &str| {
        let mut cmd = run(dir.path(), "app", false);
        cmd.env("CABIN_SDK_ZLIB_DIR", sdk);
        let output = printed(&cmd.assert().failure());
        assert!(
            crate::standard_compat::flat_contains(&output, phrase),
            "{phrase}: {output}"
        );
    };

    let empty = scratch.child("empty");
    empty.create_dir_all().unwrap();
    refused(empty.path(), "zlib: there is no ");
    refused(empty.path(), "or enable the `vendored` feature");

    let old = scratch.child("old");
    old.child(header)
        .write_str("#define ZLIB_VERNUM 0x1280\n")
        .unwrap();
    old.child(&library).write_file(&sdk.join(&library)).unwrap();
    refused(old.path(), "zlib: older than 1.2.11: ");

    let headers = scratch.child("headers");
    headers.child(header).write_file(&sdk.join(header)).unwrap();
    refused(
        headers.path(),
        "zlib: there is no zlib library under lib/ of ",
    );

    // An ELF object's header for RISC-V, which no machine these tests
    // run on is.
    let mut object = vec![0x7f, b'E', b'L', b'F', 2, 1, 1];
    object.resize(64, 0);
    object[16..20].copy_from_slice(&[1, 0, 243, 0]);
    let foreign = scratch.child("foreign");
    foreign.child(header).write_file(&sdk.join(header)).unwrap();
    foreign.child(&library).write_binary(&object).unwrap();
    refused(
        foreign.path(),
        "is built for riscv64, and this build is for ",
    );

    if cfg!(windows) {
        // The default Windows toolchain is MSVC, whose libraries
        // pkg-config does not describe.
        let output = printed(&run(dir.path(), "app", false).assert().failure());
        let phrase = "zlib: pkg-config does not describe libraries for an MSVC-style build";
        assert!(
            crate::standard_compat::flat_contains(&output, phrase),
            "{output}"
        );
        return;
    }
    let fixtures = Fixtures::new();
    let asked = || {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.current_dir(dir.path())
            .args(["run", "-p", "app"])
            .env_remove("CABIN_SDK_ZLIB_DIR");
        printed(&cmd.assert().failure())
    };
    let missing = asked();
    assert!(
        missing.contains("cabin::system_deps::package_not_found"),
        "{missing}"
    );
    write_module(&fixtures, &sdk, "1.2.8");
    let old = asked();
    assert!(
        old.contains("cabin::system_deps::version_mismatch"),
        "{old}"
    );
}

/// With no directory given, the system's zlib is the module pkg-config
/// knows.  The C program takes it, and the vendored one, on a `PATH`
/// that holds no C++ compiler: the package and its script are C.
#[cfg(unix)]
#[test]
fn the_systems_zlib_comes_from_pkg_config_and_needs_no_cxx_compiler() {
    require_c_and_cxx_build_tools();
    let dir = example();
    let scratch = TempDir::new().unwrap();
    let sdk = build_sdk(scratch.path(), "sdk", "1.3.1-sdk", None);
    let fixtures = Fixtures::new();
    write_module(&fixtures, &sdk, "1.3.1");
    let asked = |package: &str| {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.current_dir(dir.path())
            .args(["run", "-p", package])
            .env_remove("CABIN_SDK_ZLIB_DIR");
        cmd
    };

    for package in ["app", "appxx"] {
        let output = printed(&asked(package).assert().success());
        assert!(says(&output, "1.3.1-sdk"), "{package}: {output}");
    }

    let c_only = crate::c_language::c_only_path();
    let build_dir = scratch.path().join("c-only");
    for (mut cmd, version) in [
        (asked("app"), "1.3.1-sdk"),
        (run(dir.path(), "app", true), "1.3.1"),
    ] {
        cmd.arg("--build-dir")
            .arg(&build_dir)
            .env("PATH", c_only.path());
        let output = printed(&cmd.assert().success());
        assert!(says(&output, version), "{output}");
    }
}

/// `diamond` reaches zlib through `packer`, which asks for nothing, and
/// through `checksum`, which asks for `vendored`: one zlib is built,
/// the vendored one, and both libraries get it, although the system's
/// is there to take.
#[test]
fn one_request_for_vendored_gives_every_dependent_the_vendored_zlib() {
    require_c_and_cxx_build_tools();
    let dir = example();
    let scratch = TempDir::new().unwrap();
    let sdk = build_sdk(scratch.path(), "sdk", "1.3.1-sdk", None);
    dir.child("packages/checksum/cabin.toml")
        .write_str(
            r#"[package]
name = "checksum"
version = "0.1.0"
c-standard = "c11"

[dependencies]
zlib = { path = "../zlib", features = ["vendored"] }

[target.checksum]
type = "library"
sources = ["src/checksum.c"]
deps = ["zlib:z"]
"#,
        )
        .unwrap();
    dir.child("packages/checksum/src/checksum.c")
        .write_str(
            r#"#include <zlib.h>
const char *checksum_zlib(void) { return zlibVersion(); }
"#,
        )
        .unwrap();
    dir.child("packages/diamond/cabin.toml")
        .write_str(
            r#"[package]
name = "diamond"
version = "0.1.0"
c-standard = "c11"

[dependencies]
packer = { path = "../packer" }
checksum = { path = "../checksum" }

[target.diamond]
type = "executable"
sources = ["src/main.c"]
deps = ["packer:packer", "checksum:checksum"]
"#,
        )
        .unwrap();
    dir.child("packages/diamond/src/main.c")
        .write_str(
            r#"#include <packer.h>
#include <stdio.h>
const char *checksum_zlib(void);
int main(void) {
    printf("packer: zlib %s (header %s), checksum: zlib %s\n", packer_zlib(), ZLIB_VERSION,
           checksum_zlib());
    return 0;
}
"#,
        )
        .unwrap();

    let mut cmd = run(dir.path(), "diamond", false);
    cmd.env("CABIN_SDK_ZLIB_DIR", &sdk);
    let output = printed(&cmd.assert().success());
    assert!(
        output.contains("packer: zlib 1.3.1 (header 1.3.1), checksum: zlib 1.3.1\n"),
        "{output}"
    );
}

/// The package claims the native library `z` however it supplies it:
/// another package that claims `z` cannot be in the same build, with
/// the system's zlib or the vendored one.
#[test]
fn the_package_claims_its_native_library_in_either_mode() {
    let dir = example();
    dir.child("packages/other/cabin.toml")
        .write_str(
            r#"[package]
name = "other"
version = "0.1.0"
c-standard = "c11"

[target.other]
type = "library"
sources = ["src/other.c"]
links = "z"
"#,
        )
        .unwrap();
    dir.child("packages/both/cabin.toml")
        .write_str(
            r#"[package]
name = "both"
version = "0.1.0"
c-standard = "c11"

[dependencies]
zlib = { path = "../zlib" }
other = { path = "../other" }

[features]
vendored = ["zlib/vendored"]

[target.both]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib:z", "other:other"]
"#,
        )
        .unwrap();

    for features in [&[][..], &["--features", "vendored"]] {
        let mut cmd = cabin();
        cmd.current_dir(dir.path())
            .args(["build", "-p", "both"])
            .args(features);
        let output = printed(&cmd.assert().failure());
        let phrase = r#"native library "z" is claimed by multiple packages"#;
        assert!(
            crate::standard_compat::flat_contains(&output, phrase),
            "{output}"
        );
    }
}
