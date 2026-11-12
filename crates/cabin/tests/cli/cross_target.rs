//! End-to-end coverage for `--target <triple>` builds.

use super::*;

#[test]
fn build_target_rejects_a_gcc_that_builds_for_another_machine() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let bin = TempDir::new().unwrap();
    let gcc = bin.path().join("fake-g++");
    write_executable(
        &gcc,
        "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo x86_64-linux-gnu; else echo 'g++ (GCC) 13.2.0'; fi\n",
    );
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

/// A MinGW GCC chosen by its C++ driver alone brings the C compiler
/// and archiver beside it, and its executables are `<name>.exe`.
#[test]
fn a_mingw_build_takes_the_drivers_siblings_and_names_executables_exe() {
    let dir = TempDir::new().unwrap();
    crate::c_language::write_mixed_library(dir.path());
    let bin = TempDir::new().unwrap();
    for driver in ["g++", "gcc"] {
        write_executable(
            &bin.path().join(format!("x86_64-w64-mingw32-{driver}")),
            "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo x86_64-w64-mingw32; else echo 'g++ (GCC) 13.2.0'; fi\n",
        );
    }
    fake_tool_with_output(
        bin.path(),
        "x86_64-w64-mingw32-ar",
        "GNU ar (GNU Binutils) 2.40\n",
        "",
        0,
    );
    // The fakes compile nothing, so only the plan is checked.
    cabin()
        .current_dir(dir.path())
        .args(["build", "--target", "x86_64-w64-mingw32", "--cxx"])
        .arg(bin.path().join("x86_64-w64-mingw32-g++"))
        .output()
        .unwrap();
    let out = dir.path().join("build/.targets/x86_64-w64-mingw32/dev");
    let ninja = fs::read_to_string(out.join("build.ninja")).unwrap();
    assert!(ninja.contains("packages/mixed/app.exe"), "{ninja}");
    assert!(ninja.contains("x86_64-w64-mingw32-ar"), "{ninja}");
    let database: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("compile_commands.json")).unwrap())
            .unwrap();
    let c_part = database
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file"].as_str().unwrap().ends_with("c_part.c"))
        .unwrap();
    assert!(
        c_part["arguments"][0]
            .as_str()
            .unwrap()
            .contains("x86_64-w64-mingw32-gcc"),
        "{c_part}"
    );
}

/// A GCC builds only its own target, so build dependencies build with
/// the build machine's compilers, chosen by `HOST_CC`, `HOST_CXX`, and
/// `HOST_AR`.  A build-machine compiler of another dialect is refused.
#[test]
fn build_dependencies_under_a_cross_gcc_build_with_the_host_compilers() {
    let dir = TempDir::new().unwrap();
    crate::build_dependencies::write_generator_project(dir.path());
    let bin = TempDir::new().unwrap();
    let fake_gcc = |name: &str, machine: &str| {
        let path = bin.path().join(name);
        write_executable(
            &path,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo {machine}; else echo 'g++ (GCC) 13.2.0'; fi\n"
            ),
        );
        path
    };
    let gcc = fake_gcc("fake-g++", "riscv64-linux-gnu");
    let ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "linux-gnu"
    };
    let host = fake_gcc("host-g++", &format!("{}-{os}", std::env::consts::ARCH));
    let command = |subcommand: &str, host: &Path| {
        let mut command = cabin();
        command
            .current_dir(dir.path().join("app"))
            .args([subcommand, "--target", "riscv64-unknown-linux-gnu", "--cc"])
            .arg(&gcc)
            .arg("--cxx")
            .arg(&gcc)
            .arg("--ar")
            .arg(&ar)
            .env("HOST_CC", host)
            .env("HOST_CXX", host)
            .env("HOST_AR", &ar);
        command
    };
    // The fakes compile nothing, so only the plan is checked.
    command("build", &host).output().unwrap();
    let database: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            dir.path()
                .join("app/build/.targets/riscv64-unknown-linux-gnu/dev/compile_commands.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let entries = database.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["output"].as_str().unwrap().contains("build-machine")),
        "{database:#}"
    );
    for entry in entries {
        let compiler = if entry["output"].as_str().unwrap().contains("build-machine") {
            &host
        } else {
            &gcc
        };
        assert!(
            entry["arguments"][0]
                .as_str()
                .unwrap()
                .contains(compiler.to_str().unwrap()),
            "{entry}"
        );
    }

    command("build", &gcc)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "builds for riscv64-linux-gnu, not the build machine",
        ));
    let cl = fake_tool_with_output(
        bin.path(),
        "host-cl",
        "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.0 for x64\n",
        "",
        0,
    );
    command("build", &cl)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "one build cannot mix MSVC-style and GCC-style compilers",
        ));
    // `cabin check` checks only the selected packages' own sources,
    // so it needs no build-machine toolchain, and it keeps the one the
    // toolchain stamp recorded rather than rebuilding every edge.
    command("check", &bin.path().join("no-such-compiler"))
        .assert()
        .success();
    let stamp = fs::read_to_string(
        dir.path()
            .join("app/build/.targets/riscv64-unknown-linux-gnu/dev/cabin-toolchain.json"),
    )
    .unwrap();
    assert!(stamp.contains("\"build_machine\""), "{stamp}");

    // MSVC links read `LIB`, which both contexts would share.
    let arm64_cl = fake_tool_with_output(
        bin.path(),
        "cl-arm64",
        "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.0 for ARM64\n",
        "",
        0,
    );
    let lib = fake_tool_with_output(
        bin.path(),
        "fake-lib",
        "Microsoft (R) Library Manager Version 14.39.33523.0\n",
        "",
        0,
    );
    cabin()
        .current_dir(dir.path().join("app"))
        .args(["build", "--target", "aarch64-pc-windows-msvc", "--cc"])
        .arg(&arm64_cl)
        .arg("--cxx")
        .arg(&arm64_cl)
        .arg("--ar")
        .arg(&lib)
        .assert()
        .failure()
        .stderr(predicate::str::contains("the one `LIB` environment"));
}

/// The build machine's toolchain for a `--target` build is a project
/// choice too: a root manifest cannot run an executable it names by
/// path through it, and `HOST_CXX`, not `--cxx`, overrides it.
#[test]
fn an_untrusted_project_cannot_run_a_build_machine_tool_it_names_by_path() {
    let dir = TempDir::new().unwrap();
    crate::build_dependencies::write_generator_project(dir.path());
    let shim = dir.path().join("app/shim");
    write_executable(&shim, "#!/bin/sh\n: >\"$0.ran\"\n");
    let manifest = dir.path().join("app/cabin.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            r"{text}
[toolchain]
cxx = {shim:?}
"
        ),
    )
    .unwrap();
    let bin = TempDir::new().unwrap();
    let fake_gcc = |name: &str, machine: &str| {
        let path = bin.path().join(name);
        write_executable(
            &path,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = -dumpmachine ]; then echo {machine}; else echo 'g++ (GCC) 13.2.0'; fi\n"
            ),
        );
        path
    };
    let gcc = fake_gcc("fake-g++", "riscv64-linux-gnu");
    let ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let mut command = cabin();
    command
        .current_dir(dir.path().join("app"))
        .env("CABIN_CONFIG_HOME", bin.path())
        .args(["build", "--target", "riscv64-unknown-linux-gnu", "--cc"])
        .arg(&gcc)
        .arg("--cxx")
        .arg(&gcc)
        .arg("--ar")
        .arg(&ar)
        .env_remove("HOST_CC")
        .env_remove("HOST_CXX")
        .env_remove("HOST_AR");
    command
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "which Cabin runs only for a project you trust",
        ))
        .stderr(predicate::str::contains("`HOST_CXX`"));
    assert!(!dir.path().join("app/shim.ran").exists());
}

/// Apple Clang builds either macOS architecture on any Mac, so these
/// tests cross-build for the architecture the build machine is not.
#[cfg(target_os = "macos")]
mod apple_clang {
    use super::*;

    /// The other macOS architecture: its triple, its `cfg(arch)` name,
    /// and its Mach-O CPU type.
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
            .write_str(
                r"#include <stdio.h>
const char *picked(void);
int main(void) { puts(picked()); return 0; }
",
            )
            .unwrap();
        root.child("src/main.cc")
            .write_str(
                r#"#include <cstdio>
extern "C" const char *picked(void);
int main() { std::puts(picked()); }
"#,
            )
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
            let cross_exe = build
                .join(".targets")
                .join(triple)
                .join("dev/packages/app")
                .join(exe);
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

    /// The build context's C generator is built for and run on the
    /// build machine, and its output is compiled into a C++ program for
    /// the other architecture.
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
        let build_root = app.join("build/.targets").join(triple).join("dev");
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

    /// A build script is built for and run on the build machine, is told
    /// the platform it prepares its package for, and what it generates
    /// is compiled for that platform, apart from the native build's.
    #[test]
    fn a_build_script_runs_on_the_build_machine_for_the_target() {
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[target.app]
type = "executable"
sources = ["src/main.c"]
"#,
            )
            .unwrap();
        dir.child("src/main.c")
            .write_str(
                r#"#include <stdio.h>
#include "arch.h"
int main(void) { puts(PREPARED_FOR); return 0; }
"#,
            )
            .unwrap();
        dir.child("build.c")
            .write_str(
                r##"#include <stdio.h>
#include <stdlib.h>
int main(void) {
    const char *out = getenv("CABIN_OUT_DIR");
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/arch.h", out);
    if (!(file = fopen(path, "w"))) return 1;
    fprintf(file, "#define PREPARED_FOR \"prepared-for-%s\"\n", getenv("CABIN_TARGET_ARCH"));
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "include-dir app %s\n", out);
    return fclose(file) != 0;
}
"##,
            )
            .unwrap();
        let (triple, arch, cpu_type) = cross();

        cabin()
            .current_dir(dir.path())
            .args(["build", "--target", triple])
            .assert()
            .success();
        let profile = dir.path().join("build/.targets").join(triple).join("dev");
        let script = profile.join("build-machine/packages/app/.build-script/target/build");
        assert_ne!(mach_o_cpu_type(&script), cpu_type);
        let program = profile.join("packages/app/app");
        assert_eq!(mach_o_cpu_type(&program), cpu_type);
        let prepared = format!("prepared-for-{arch}");
        let body = fs::read(&program).unwrap();
        assert!(
            body.windows(prepared.len())
                .any(|w| w == prepared.as_bytes())
        );

        cabin()
            .current_dir(dir.path())
            .arg("run")
            .assert()
            .success()
            .stdout(predicate::str::contains(format!(
                "prepared-for-{}",
                std::env::consts::ARCH
            )));
    }

    /// A library a build script supplies is one for the machine the
    /// build is for: the target's under `--target`, the build machine's
    /// without, and the other's is refused before any link.
    #[test]
    fn a_build_script_supplies_a_library_for_the_machine_the_build_is_for() {
        use crate::build_scripts::{build_greet_sdk, write_greet_app};
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_greet_app(dir.path());
        let (triple, arch, cpu_type) = cross();
        build_greet_sdk(dir.path(), "sdk-native", "native", None);
        build_greet_sdk(dir.path(), "sdk-cross", "cross", Some(triple));
        let app = dir.path().join("app");
        let cabin_build = |sdk: &str, target: bool| {
            let mut command = cabin();
            command
                .current_dir(&app)
                .arg("build")
                .env("CABIN_SDK_GREET", dir.path().join(sdk));
            if target {
                command.args(["--target", triple]);
            }
            command.assert()
        };
        let host_arch = std::env::consts::ARCH;

        cabin_build("sdk-cross", true).success();
        let profile = app.join("build/.targets").join(triple).join("dev");
        for program in ["app", "appxx"] {
            let program = profile.join("packages/app").join(program);
            assert_eq!(mach_o_cpu_type(&program), cpu_type);
        }
        for (sdk, target, built, machine) in [
            ("sdk-native", true, host_arch, arch),
            ("sdk-cross", false, arch, host_arch),
        ] {
            let refused = cabin_build(sdk, target).failure();
            let stderr = String::from_utf8_lossy(&refused.get_output().stderr).into_owned();
            let phrase =
                format!("libgreet.a` is built for {built}, and this build is for {machine}");
            assert!(
                crate::standard_compat::flat_contains(&stderr, &phrase),
                "{stderr}"
            );
        }
        cabin_build("sdk-native", false).success();
    }

    /// A `--target` build asks the pkg-config it is given, the target's,
    /// for a library a build script supplies.  A build dependency's
    /// script cannot ask it: its library is the build machine's.
    #[test]
    fn a_build_script_takes_a_library_from_the_targets_pkg_config() {
        use crate::build_scripts::{
            build_greet_sdk, write_greet_app, write_greet_consumer, write_greet_module,
        };
        use crate::system_deps_pkg_config::{Fixtures, cabin_with_fake_pkg_config};
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_greet_app(dir.path());
        write_greet_consumer(dir.path());
        let (triple, _, cpu_type) = cross();
        build_greet_sdk(dir.path(), "sdk", "cross", Some(triple));
        let fixtures = Fixtures::new();
        write_greet_module(&fixtures, dir.path(), "sdk", "1.2.3");
        let cabin_build = |package: &str, args: &[&str]| {
            let mut command = cabin_with_fake_pkg_config(&fixtures);
            command
                .current_dir(dir.path().join(package))
                .args(["build", "--target", triple])
                .args(args);
            command.assert()
        };

        cabin_build("app", &["--features", "system"]).success();
        let built = dir.path().join("app/build/.targets").join(triple);
        assert_eq!(
            mach_o_cpu_type(&built.join("dev/packages/app/app")),
            cpu_type
        );

        let refused = cabin_build("consumer", &[]).failure();
        let stderr = String::from_utf8_lossy(&refused.get_output().stderr).into_owned();
        let phrase = "`greet` cannot come from pkg-config for a build dependency";
        assert!(
            crate::standard_compat::flat_contains(&stderr, phrase),
            "{stderr}"
        );
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
                .join("build/.targets")
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

    #[test]
    fn run_and_test_hand_target_executables_to_the_configured_runner() {
        require_c_and_cxx_build_tools();
        let dir = TempDir::new().unwrap();
        write_project(dir.path());
        let (triple, ..) = cross();
        // Records its arguments instead of running anything.  It is
        // named relative to `tools/`, where both commands start, while
        // `cabin test` runs each test from the package directory.
        let tools = dir.path().join("tools");
        fs::create_dir(&tools).unwrap();
        let runner = tools.join("runner.sh");
        write_executable(&runner, "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$0.log\"\n");
        fs::create_dir(dir.path().join(".cabin")).unwrap();
        fs::write(
            dir.path().join(".cabin/config.toml"),
            format!("[runner]\n\"{triple}\" = [\"./runner.sh\", \"--emulate\"]\n"),
        )
        .unwrap();

        cabin_with_config()
            .current_dir(&tools)
            .args([
                "run",
                "--manifest-path",
                "../cabin.toml",
                "--target",
                triple,
            ])
            .args(["--bin", "capp", "--", "arg"])
            .assert()
            .success()
            .stdout(predicate::str::contains("/tools/runner.sh --emulate /"))
            .stdout(predicate::str::contains(format!(
                "/build/.targets/{triple}/dev/packages/app/capp`"
            )));
        cabin_with_config()
            .current_dir(&tools)
            .args([
                "test",
                "--manifest-path",
                "../cabin.toml",
                "--target",
                triple,
            ])
            .assert()
            .success();

        let log = fs::read_to_string(tools.join("runner.sh.log")).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        let built = format!("/build/.targets/{triple}/dev/packages/app/");
        assert_eq!(lines.len(), 5, "{log}");
        assert_eq!(
            [lines[0], lines[2], lines[3]],
            ["--emulate", "arg", "--emulate"]
        );
        assert!(lines[1].ends_with(&format!("{built}capp")), "{log}");
        assert!(lines[4].ends_with(&format!("{built}smoke")), "{log}");
    }
}

#[cfg(target_os = "linux")]
mod linux_clang {
    use super::*;

    /// ELF `e_machine` of `path` (both machines are little-endian).
    fn elf_machine(path: &Path) -> u16 {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"\x7fELF", "{}", path.display());
        u16::from_le_bytes([bytes[18], bytes[19]])
    }

    fn objects_under(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                objects_under(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "o") {
                out.push(path);
            }
        }
    }

    /// Clang builds a C and C++ library for the other Linux architecture while
    /// the build dependency's generator is built for the build machine.
    #[test]
    fn a_clang_cross_build_keeps_the_build_machine_tool_native() {
        require_c_and_cxx_build_tools();
        require_external_tool("clang");
        require_external_tool("clang++");
        // EM_X86_64 = 62, EM_AARCH64 = 183.
        let (triple, native, foreign) = match std::env::consts::ARCH {
            "x86_64" => ("aarch64-unknown-linux-gnu", 62, 183),
            "aarch64" => ("x86_64-unknown-linux-gnu", 183, 62),
            other => panic!("no cross triple for {other}"),
        };
        let dir = TempDir::new().unwrap();
        let root = assert_fs::fixture::ChildPath::new(dir.path());
        root.child("tools/cabin.toml")
            .write_str(
                r#"[package]
name = "tools"
version = "0.1.0"
c-standard = "c11"

[target.gen]
type = "executable"
sources = ["src/gen.c"]
"#,
            )
            .unwrap();
        root.child("tools/src/gen.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        root.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[build-dependencies]
tools = { path = "../tools" }

[target.applib]
type = "library"
sources = ["src/lib.c", "src/lib.cc"]
"#,
            )
            .unwrap();
        root.child("app/src/lib.c")
            .write_str("int applib(void) { return 42; }\n")
            .unwrap();
        root.child("app/src/lib.cc")
            .write_str("int applib_cxx() { return 42; }\n")
            .unwrap();
        let app = dir.path().join("app");
        cabin()
            .current_dir(&app)
            .env("CC", "clang")
            .env("CXX", "clang++")
            .args(["build", "--target", triple])
            .assert()
            .success();

        let build_root = app.join("build/.targets").join(triple).join("dev");
        assert_eq!(
            elf_machine(&build_root.join("build-machine/packages/tools/gen")),
            native
        );
        let mut objects = Vec::new();
        objects_under(&build_root.join("packages/app"), &mut objects);
        assert_eq!(objects.len(), 2, "{objects:?}");
        for object in &objects {
            assert_eq!(elf_machine(object), foreign, "{}", object.display());
        }
    }
}
