//! End-to-end coverage for `--target <triple>` builds.

use super::*;

#[test]
fn build_target_rejects_a_gcc_that_builds_for_another_machine() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let bin = TempDir::new().unwrap();
    let gcc = bin.path().join("fake-g++");
    fs::write(
        &gcc,
        "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo x86_64-linux-gnu; else echo 'g++ (GCC) 13.2.0'; fi\n",
    )
    .unwrap();
    fs::set_permissions(&gcc, fs::Permissions::from_mode(0o755)).unwrap();
    let ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    cabin()
        .current_dir(dir.path())
        .args(["build", "--target", "aarch64-unknown-linux-gnu", "--cc"])
        .arg(&gcc)
        .arg("--cxx")
        .arg(&gcc)
        .arg("--ar")
        .arg(&ar)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "builds for x86_64-linux-gnu, but `--target` requests aarch64-unknown-linux-gnu",
        ));
}

/// A GCC builds only its own target, so it cannot also build a build
/// dependency for the build machine.
#[test]
fn build_target_rejects_build_dependencies_under_a_cross_gcc() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    crate::build_dependencies::write_generator_project(dir.path());
    let bin = TempDir::new().unwrap();
    let gcc = bin.path().join("fake-g++");
    fs::write(
        &gcc,
        "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo riscv64-linux-gnu; else echo 'g++ (GCC) 13.2.0'; fi\n",
    )
    .unwrap();
    fs::set_permissions(&gcc, fs::Permissions::from_mode(0o755)).unwrap();
    let ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let command = |subcommand: &str| {
        let mut command = cabin();
        command
            .current_dir(dir.path().join("app"))
            .args([subcommand, "--target", "riscv64-unknown-linux-gnu", "--cc"])
            .arg(&gcc)
            .arg("--cxx")
            .arg(&gcc)
            .arg("--ar")
            .arg(&ar);
        command
    };
    let refusal = "`[build-dependencies]` are built for the build machine";
    command("build")
        .assert()
        .failure()
        .stderr(predicate::str::contains(refusal));
    // `cabin check` checks only the selected packages' own sources,
    // so it plans no build context and has nothing to refuse.
    command("check").assert().success();
}

/// Apple Clang builds either macOS architecture on any Mac, so these
/// tests cross-build for the architecture the build machine is not.
#[cfg(target_os = "macos")]
mod apple_clang {
    use super::*;

    /// The other macOS architecture: its triple and its Mach-O CPU type.
    fn cross() -> (&'static str, &'static str, u32) {
        if std::env::consts::ARCH == "aarch64" {
            ("x86_64-apple-macos13", "x86_64", 0x0100_0007)
        } else {
            ("arm64-apple-macos13", "aarch64", 0x0100_000c)
        }
    }

    fn mach_o_cpu_type(path: &Path) -> u32 {
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes[..4], [0xcf, 0xfa, 0xed, 0xfe], "{}", path.display());
        u32::from_le_bytes(bytes[4..8].try_into().unwrap())
    }

    /// A C and a C++ executable whose `picked()` comes from a source
    /// the target's architecture selects, plus a C test.
    fn write_project(root: &Path) {
        let root = assert_fs::fixture::ChildPath::new(root);
        root.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.capp]
type = "executable"
sources = ["src/main.c"]

[target.cxxapp]
type = "executable"
sources = ["src/main.cc"]

[target.smoke]
type = "test"
sources = ["tests/smoke.c"]

[target.'cfg(arch = "x86_64")'.sources]
capp = ["src/x86_64.c"]
cxxapp = ["src/x86_64.c"]

[target.'cfg(arch = "aarch64")'.sources]
capp = ["src/aarch64.c"]
cxxapp = ["src/aarch64.c"]
"#,
            )
            .unwrap();
        root.child("src/main.c")
            .write_str("#include <stdio.h>\nconst char *picked(void);\nint main(void) { puts(picked()); return 0; }\n")
            .unwrap();
        root.child("src/main.cc")
            .write_str("#include <cstdio>\nextern \"C\" const char *picked(void);\nint main() { std::puts(picked()); }\n")
            .unwrap();
        for arch in ["x86_64", "aarch64"] {
            root.child(format!("src/{arch}.c"))
                .write_str(&format!(
                    "const char *picked(void) {{ return \"picked-{arch}\"; }}\n"
                ))
                .unwrap();
        }
        root.child("tests/smoke.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
    }

    #[test]
    fn build_target_cross_compiles_c_and_cxx_beside_the_native_build() {
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_project(dir.path());
        let (triple, arch, cpu_type) = cross();
        let host_arch = std::env::consts::ARCH;

        cabin()
            .current_dir(dir.path())
            .args(["build", "--target", triple])
            .assert()
            .success();
        cabin()
            .current_dir(dir.path())
            .arg("build")
            .assert()
            .success();

        let build = dir.path().join("build");
        for exe in ["capp", "cxxapp"] {
            let cross_exe = build.join(triple).join("dev/packages/app").join(exe);
            assert_eq!(mach_o_cpu_type(&cross_exe), cpu_type, "{exe}");
            let body = fs::read(&cross_exe).unwrap();
            let picked = format!("picked-{arch}");
            assert!(
                body.windows(picked.len()).any(|w| w == picked.as_bytes()),
                "{exe} must link the {arch} source"
            );

            let native = std::process::Command::new(build.join("dev/packages/app").join(exe))
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&native.stdout).trim(),
                format!("picked-{host_arch}")
            );
        }
    }

    /// The stage 04 demonstration: the build context's C generator is
    /// built for and run on the build machine, and its output is
    /// compiled into a C++ program for the other architecture.
    #[test]
    fn a_build_machine_generator_feeds_a_cross_build() {
        use crate::build_dependencies::{run_generator, write_generator_project};
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_generator_project(dir.path());
        let app = dir.path().join("app");
        let (triple, _, cpu_type) = cross();

        cabin()
            .current_dir(&app)
            .args(["build", "--target", triple])
            .assert()
            .success();
        let build_root = app.join("build").join(triple).join("dev");
        let generator = build_root.join("build-machine/packages/gen/gen");
        assert_ne!(mach_o_cpu_type(&generator), cpu_type);
        run_generator(&build_root, dir.path());

        cabin()
            .current_dir(&app)
            .args(["build", "--target", triple, "--features", "generated"])
            .assert()
            .success();
        let program = build_root.join("packages/app/app-generated");
        assert_eq!(mach_o_cpu_type(&program), cpu_type);
        let body = fs::read(&program).unwrap();
        assert!(body.windows(5).any(|w| w == b"HELLO"));
    }

    #[test]
    fn run_target_naming_the_build_machine_runs() {
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_project(dir.path());
        let arch = std::env::consts::ARCH;
        let native = if arch == "aarch64" {
            "arm64-apple-macos13"
        } else {
            "x86_64-apple-macos13"
        };
        cabin()
            .current_dir(dir.path())
            .args(["run", "--target", native, "--bin", "capp"])
            .assert()
            .success()
            .stdout(predicate::str::contains(format!("picked-{arch}")));
    }

    #[test]
    fn run_and_test_build_for_the_target_but_execute_nothing() {
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_project(dir.path());
        let (triple, ..) = cross();

        cabin()
            .current_dir(dir.path())
            .args(["run", "--target", triple, "--bin", "capp"])
            .assert()
            .failure()
            .stdout(predicate::str::contains("picked-").not())
            .stderr(predicate::str::contains(format!(
                "for {triple}, but did not run it"
            )));
        assert!(
            dir.path()
                .join("build")
                .join(triple)
                .join("dev/packages/app/capp")
                .is_file()
        );

        cabin()
            .current_dir(dir.path())
            .args(["test", "--target", triple])
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "for {triple}, but ran none"
            )));
    }
}
