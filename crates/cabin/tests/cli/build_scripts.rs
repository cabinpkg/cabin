//! End-to-end coverage for build scripts: a `build.c` or `build.cpp`
//! in a package's root, built for the build machine and run confined
//! before the package's units compile.

use super::*;

const RAN: &str = "Running build script";

/// A C build script that writes `generated.h`, defining `VALUE` as the
/// string the C expression `value` gives, for the target `app`, and
/// then the result lines the C statements `report` print to `file`.
fn header_script(value: &str, report: &str) -> String {
    r##"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
    const char *out = getenv("CABIN_OUT_DIR");
    const char *root = getenv("CABIN_MANIFEST_DIR");
    char path[4096];
    FILE *file;
    (void)root;
    snprintf(path, sizeof path, "%s/generated.h", out);
    if (!(file = fopen(path, "w"))) return 1;
    fprintf(file, "#define VALUE \"%s\"\n", VALUE_EXPRESSION);
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "include-dir app %s\n", out);
    REPORT
    return fclose(file) != 0;
}
"##
    .replace("VALUE_EXPRESSION", value)
    .replace("REPORT", report)
}

/// `app`, a C program that prints `VALUE`, a macro of the `generated.h`
/// its build script is to supply, with `script` as its `build.c`.
fn write_app(root: &Path, script: &str) {
    let root = assert_fs::fixture::ChildPath::new(root);
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[features]
extra = []

[target.app]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    root.child("src/main.c")
        .write_str(
            r#"#include <stdio.h>
#include "generated.h"
int main(void) { puts(VALUE); return 0; }
"#,
        )
        .unwrap();
    root.child("build.c").write_str(script).unwrap();
}

fn cabin_in(root: &Path, args: &[&str]) -> Command {
    let mut cmd = cabin();
    cmd.current_dir(root).args(args);
    cmd
}

/// What `cabin` printed: status lines and the program's own output on
/// standard output, then diagnostics on standard error.
fn printed(assert: &assert_cmd::assert::Assert) -> String {
    let output = assert.get_output();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace("\r\n", "\n")
}

/// `lib` says "HELLO" when its build script, seeing the feature `loud`,
/// defines `LOUD` as 1, and "hello" otherwise.  `app` links `lib`
/// without the feature, and its own build script links `lib` with it,
/// as a build dependency, to generate the greeting it prints second.
#[test]
fn each_context_runs_a_script_of_its_own_with_its_features() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path().join("a project"));
    root.child("lib/cabin.toml")
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"
c-standard = "c11"

[features]
loud = []

[target.lib]
type = "library"
sources = ["src/lib.c"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    root.child("lib/include/lib.h")
        .write_str("const char *greeting(void);\n")
        .unwrap();
    root.child("lib/src/lib.c")
        .write_str(
            r#"#include <lib.h>
const char *greeting(void) { return LOUD ? "HELLO" : "hello"; }
"#,
        )
        .unwrap();
    root.child("lib/build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fprintf(file, "define lib LOUD=%d\n", getenv("CABIN_FEATURE_LOUD") != NULL);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    write_app(
        &root.path().join("app"),
        &format!("#include <lib.h>\n{}", header_script("greeting()", "")),
    );
    let manifest = root.child("app/cabin.toml");
    let text = fs::read_to_string(manifest.path()).unwrap();
    manifest
        .write_str(&format!(
            r#"{text}
[dependencies]
lib = {{ path = "../lib" }}

[build-dependencies]
lib = {{ path = "../lib", features = ["loud"] }}
"#
        ))
        .unwrap();
    root.child("app/src/main.c")
        .write_str(
            r#"#include <stdio.h>
#include <lib.h>
#include "generated.h"
int main(void) { printf("%s %s\n", greeting(), VALUE); return 0; }
"#,
        )
        .unwrap();
    let text = fs::read_to_string(manifest.path()).unwrap();
    manifest
        .write_str(&text.replace(
            "sources = [\"src/main.c\"]\n",
            "sources = [\"src/main.c\"]\ndeps = [\"lib:lib\"]\n",
        ))
        .unwrap();

    let app = root.path().join("app");
    let printed = printed(&cabin_in(&app, &["run"]).assert().success());
    let at = |line: &str| {
        printed
            .find(line)
            .unwrap_or_else(|| panic!("no `{line}` in: {printed}"))
    };
    at("\nhello HELLO\n");
    // The script of the build machine's `lib` ran before the script
    // that links that `lib`.
    assert!(
        at("Running build script (build machine) of lib") < at("Running build script of app"),
        "{printed}"
    );
    at("Running build script of lib");
    let profile = app.join("build/dev");
    for run in ["packages/lib", "build-machine/packages/lib", "packages/app"] {
        assert!(
            profile.join(run).join(".build-script/stamp").is_file(),
            "{run}"
        );
    }
}

/// An optional build dependency is built, and its executable offered
/// to the build script, only when a feature enables it, in the context
/// that feature is on: `app` enables `gen` for its own script, and its
/// `gen/loud` reaches the build machine's `gen`, whose script then
/// takes `helper`.
#[test]
fn an_optional_build_dependency_follows_the_features_that_name_it() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("helper/cabin.toml")
        .write_str(
            r#"[package]
name = "helper"
version = "0.1.0"
c-standard = "c11"

[target.helper]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    root.child("helper/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    root.child("gen/cabin.toml")
        .write_str(
            r#"[package]
name = "gen"
version = "0.1.0"
c-standard = "c11"

[features]
loud = ["dep:helper"]

[build-dependencies]
helper = { path = "../helper", optional = true }

[target.gen]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    root.child("gen/src/main.c")
        .write_str(
            r#"#include <stdio.h>
int main(void) { puts(LOUD ? "LOUD" : "quiet"); return 0; }
"#,
        )
        .unwrap();
    root.child("gen/build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fprintf(file, "define gen LOUD=%d\n", getenv("CABIN_BIN_HELPER") != NULL);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    let app = dir.path().join("app");
    write_app(
        &app,
        &header_script(r#"(getenv("CABIN_BIN_GEN") ? "tool" : "none")"#, ""),
    );
    let manifest = root.child("app/cabin.toml");
    let text = fs::read_to_string(manifest.path()).unwrap();
    manifest
        .write_str(&format!(
            r#"{}
[build-dependencies]
gen = {{ path = "../gen", optional = true }}
"#,
            text.replace(
                "extra = []\n",
                "codegen = [\"dep:gen\"]\nloud = [\"gen/loud\"]\n"
            )
        ))
        .unwrap();
    let built = |package: &str| {
        app.join("build/dev/build-machine/packages")
            .join(package)
            .join(host_exe(package))
    };

    let printed_by = |args: &[&str]| printed(&cabin_in(&app, args).assert().success());
    assert!(printed_by(&["run"]).contains("\nnone\n"));
    assert!(!built("gen").exists());
    for (feature, says) in [("codegen", "quiet"), ("loud", "LOUD")] {
        let printed = printed_by(&["run", "--features", feature]);
        assert!(printed.contains("\ntool\n"), "{printed}");
        let output = std::process::Command::new(built("gen")).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), says);
        assert_eq!(built("helper").exists(), feature == "loud");
    }
}

/// A build script in C++ links the C++ library of a build dependency,
/// and the C program it prepares still links with the C driver.
#[test]
fn a_cxx_build_script_leaves_its_c_package_linking_as_c() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("helper/cabin.toml")
        .write_str(
            r#"[package]
name = "helper"
version = "0.1.0"
cxx-standard = "c++17"

[target.helper]
type = "library"
sources = ["src/helper.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    root.child("helper/include/helper.hpp")
        .write_str("#include <string>\nstd::string helped();\n")
        .unwrap();
    root.child("helper/src/helper.cc")
        .write_str("#include <helper.hpp>\nstd::string helped() { return \"helped\"; }\n")
        .unwrap();
    write_app(&root.path().join("app"), "");
    let app = root.path().join("app");
    fs::remove_file(app.join("build.c")).unwrap();
    root.child("app/build.cpp")
        .write_str(
            r##"#include <cstdlib>
#include <fstream>
#include <string>
#include <helper.hpp>

int main() {
    const std::string out = std::getenv("CABIN_OUT_DIR");
    std::ofstream(out + "/generated.h") << "#define VALUE \"" << helped() << "\"\n";
    std::ofstream(std::getenv("CABIN_RESULT_FILE")) << "include-dir app " << out << "\n";
}
"##,
        )
        .unwrap();
    let manifest = root.child("app/cabin.toml");
    let text = fs::read_to_string(manifest.path()).unwrap();
    manifest
        .write_str(&format!(
            "{text}\n[build-dependencies]\nhelper = {{ path = \"../helper\" }}\n"
        ))
        .unwrap();

    cabin_in(&app, &["run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("helped"));
    let detected = &run_metadata(&app.join("cabin.toml"))["toolchain"]["detected"];
    let ninja = fs::read_to_string(app.join("build/dev/build.ninja")).unwrap();
    let link = |tail: &str| crate::build_dependencies::link_program(&ninja, tail);
    assert_eq!(
        link(&format!("packages/app/{}", host_exe("app"))),
        detected["cc"]["path"].as_str().unwrap()
    );
    assert_eq!(
        link(&format!(
            "build-machine/packages/app/.build-script/target/{}",
            host_exe("build")
        )),
        detected["cxx"]["path"].as_str().unwrap()
    );
}

/// A script that exits unsuccessfully, is killed, or reports what Cabin
/// refuses fails the build, leaves no result, and runs again next time.
#[test]
fn a_failed_build_script_fails_the_build_and_runs_again() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The first byte of `mode.txt` chooses what the script does.
    let script = header_script(
        "\"made\"",
        r#"{
        char mode = 0;
        FILE *chosen;
        snprintf(path, sizeof path, "%s/mode.txt", root);
        if ((chosen = fopen(path, "r"))) { mode = (char)fgetc(chosen); fclose(chosen); }
        if (mode == 'f') { fputs("the widget is missing\n", stderr); return 3; }
        if (mode == 'k') abort();
        if (mode == 'b') fputs("frobnicate app 1\n", file);
    }"#,
    );
    write_app(dir.path(), &script);
    let mode = dir.child("mode.txt");
    let stamp = dir
        .path()
        .join("build/dev/packages/app/.build-script/stamp");

    mode.write_str("ok").unwrap();
    cabin_in(dir.path(), &["run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("made"));
    assert!(stamp.is_file());

    mode.write_str("fail").unwrap();
    for _ in 0..2 {
        let printed = printed(&cabin_in(dir.path(), &["build"]).assert().failure());
        for expected in [RAN, "cabin::build_script::error", "the widget is missing"] {
            assert!(printed.contains(expected), "{printed}");
        }
        assert!(!stamp.exists());
    }

    // Only Unix has a death by signal; elsewhere `abort` is one more
    // exit status.
    if cfg!(unix) {
        mode.write_str("killed").unwrap();
        cabin_in(dir.path(), &["build"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("the build script failed"));
    }

    mode.write_str("bad").unwrap();
    let assert = cabin_in(dir.path(), &["build"]).assert().failure();
    let stderr = stderr_with_wrapping_joined(&assert);
    assert!(
        stderr.contains("line2ofthebuildscript'sresultfile:`frobnicate`isnotadirective"),
        "{stderr}"
    );
    assert!(!stamp.exists());

    mode.write_str("ok").unwrap();
    let assert = cabin_in(dir.path(), &["build"]).assert().success();
    assert!(printed(&assert).contains(RAN));
}

/// A script runs again when its source, its package's features, a file
/// it named, or a `CABIN_SDK_*` variable changes, and not otherwise;
/// when it writes what it wrote before, nothing compiles again.
#[test]
fn a_build_script_runs_again_only_when_what_it_depends_on_changes() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let script = |tag: &str| {
        header_script(
            &format!("strcat(strcpy(path, \"{tag}:\"), sdk ? sdk : \"none\")"),
            r#"fprintf(file, "rerun-if-changed %s/watched.txt\n", root);"#,
        )
        .replace(
            "char path[4096];",
            "char path[4096];\n    const char *sdk = getenv(\"CABIN_SDK_DEMO\");",
        )
    };
    write_app(dir.path(), &script("v1"));
    dir.child("watched.txt").write_str("one").unwrap();
    let build = |args: &[&str], sdk: Option<&str>| {
        let mut cmd = cabin_in(dir.path(), args);
        if let Some(sdk) = sdk {
            cmd.env("CABIN_SDK_DEMO", sdk);
        }
        printed(&cmd.assert().success())
    };

    let first = build(&["run"], None);
    assert!(
        first.contains(RAN) && first.contains("\nv1:none\n"),
        "{first}"
    );

    let unchanged = build(&["build"], None);
    assert!(
        !unchanged.contains(RAN) && !unchanged.contains("Compiling"),
        "{unchanged}"
    );

    dir.child("notes.txt").write_str("unwatched").unwrap();
    let unwatched = build(&["build"], None);
    assert!(!unwatched.contains(RAN), "{unwatched}");

    dir.child("watched.txt").write_str("two").unwrap();
    let watched = build(&["build"], None);
    assert!(
        watched.contains(RAN) && !watched.contains("Compiling"),
        "{watched}"
    );

    let feature = build(&["build", "--features", "extra"], None);
    assert!(feature.contains(RAN), "{feature}");

    dir.child("build.c").write_str(&script("v2")).unwrap();
    let source = build(&["run", "--features", "extra"], None);
    assert!(
        source.contains(RAN) && source.contains("\nv2:none\n"),
        "{source}"
    );

    let sdk = build(&["run", "--features", "extra"], Some("two"));
    assert!(sdk.contains(RAN) && sdk.contains("\nv2:two\n"), "{sdk}");
}

/// Only the building commands build and run a build script, `cabin
/// check` among them, and publishing is not one; a root holding both
/// spellings is refused; and a file of another name is no build script.
#[test]
fn check_runs_a_build_script_and_inspection_does_not() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_app(dir.path(), &header_script("\"made\"", ""));
    let broken = "#error this is not to be compiled\n";
    for other in ["build.cc", "build.cxx"] {
        dir.child(other).write_str(broken).unwrap();
    }
    cabin_in(dir.path(), &["check"]).assert().success();
    assert!(
        dir.path()
            .join("build/dev/packages/app/.build-script/out/generated.h")
            .is_file()
    );

    dir.child("build.c").write_str(broken).unwrap();
    for inspection in ["metadata", "tree", "fetch", "package"] {
        cabin_in(dir.path(), &[inspection]).assert().success();
    }
    cabin_in(dir.path(), &["check"]).assert().failure();

    // Publishing declares the script without building it either.
    let registry = TempDir::new().unwrap();
    let manifest = fs::read_to_string(dir.path().join("cabin.toml")).unwrap();
    dir.child("cabin.toml")
        .write_str(&manifest.replace("name = \"app\"", "name = \"acme/app\"\nlicense = \"MIT\""))
        .unwrap();
    cabin_in(dir.path(), &["publish", "--dry-run"])
        .assert()
        .success();
    cabin_in(dir.path(), &["publish", "--registry-dir"])
        .arg(registry.path())
        .assert()
        .success();
    let entry = fs::read_to_string(registry.path().join("packages/acme/app.json")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(&entry).unwrap();
    assert_eq!(entry["versions"]["0.1.0"]["build-script"], "build.c");

    dir.child("build.cpp").write_str(broken).unwrap();
    cabin_in(dir.path(), &["build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin::build_script::error"))
        .stderr(predicate::str::contains(
            "holds both `build.c` and `build.cpp`",
        ));
}

/// A standard the compiler has no flag for is refused before a build
/// script's step writes a Ninja file, as it is without a build script.
#[test]
#[cfg_attr(
    not(windows),
    ignore = "MSVC has no C99 mode; Windows CI is the MSVC-dialect e2e leg"
)]
fn a_standard_without_a_flag_is_refused_before_a_build_script_is_built() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_app(dir.path(), &header_script("\"made\"", ""));
    let manifest = dir.child("cabin.toml");
    let text = fs::read_to_string(manifest.path()).unwrap();
    manifest.write_str(&text.replace("c11", "c99")).unwrap();
    cabin_in(dir.path(), &["build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no stable MSVC"));
    assert!(!dir.path().join("build/dev/build.ninja").exists());
}

/// A script reads its package but for what an archive of it leaves
/// out, writes nowhere in it, and sees none of the user's credentials.
#[test]
fn a_build_script_reaches_neither_its_package_nor_credentials() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let script = r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>

static int opens(const char *root, const char *name, const char *mode) {
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/%s", root, name);
    if ((file = fopen(path, mode))) fclose(file);
    return file != NULL;
}

int main(void) {
    const char *root = getenv("CABIN_MANIFEST_DIR");
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fprintf(file, "define app READ_SOURCE=%d\n", opens(root, "src/main.c", "r"));
    fprintf(file, "define app READ_GIT=%d\n", opens(root, ".git/config", "r"));
    fprintf(file, "define app WROTE_NEW=%d\n", opens(root, "planted.txt", "w"));
    fprintf(file, "define app WROTE_MANIFEST=%d\n", opens(root, "cabin.toml", "a"));
    fprintf(file, "define app SAW_SECRET=%d\n",
            getenv("CABIN_REGISTRY_TOKEN") || getenv("GITHUB_TOKEN") || getenv("SSH_AUTH_SOCK")
                || getenv("ACTIONS_ID_TOKEN_REQUEST_TOKEN"));
    return fclose(file) != 0;
}
"#;
    write_app(dir.path(), script);
    dir.child("src/main.c")
        .write_str(
            r#"#include <stdio.h>
int main(void) {
    printf("%d %d %d %d %d\n", READ_SOURCE, READ_GIT, WROTE_NEW, WROTE_MANIFEST, SAW_SECRET);
    return 0;
}
"#,
        )
        .unwrap();
    dir.child(".git/config").write_str("token\n").unwrap();
    let manifest = fs::read_to_string(dir.path().join("cabin.toml")).unwrap();

    cabin_in(dir.path(), &["run"])
        .env("CABIN_REGISTRY_TOKEN", "secret")
        .env("GITHUB_TOKEN", "secret")
        .env("SSH_AUTH_SOCK", dir.path().join("agent"))
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "secret")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 0 0 0 0"));
    assert!(!dir.path().join("planted.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("cabin.toml")).unwrap(),
        manifest
    );
}

/// Publish `acme/helper`, `acme/gen`, and `acme/lib` to a file registry
/// under `dir` and return it.  `acme/helper` says "HELLO" when its
/// build script sees the feature `loud` and "hello" otherwise.
/// `acme/lib` links it without the feature, and its build script links
/// it with the feature, to generate the `MADE` the library returns
/// from that greeting and from whether it was offered `acme/gen`'s
/// executable.
fn publish_packages_with_build_scripts(dir: &Path) -> PathBuf {
    let registry = dir.join("registry");
    let publish = |name: &str, files: &[(&str, &str)]| {
        let root = assert_fs::fixture::ChildPath::new(dir.join(name));
        for (path, text) in files {
            root.child(path).write_str(text).unwrap();
        }
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(root.path().join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .assert()
            .success();
    };
    publish(
        "helper",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/helper"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[features]
loud = []

[target.helper]
type = "library"
sources = ["src/helper.c"]
include-dirs = ["include"]
"#,
            ),
            ("include/helper.h", "const char *greeting(void);\n"),
            (
                "src/helper.c",
                r#"#include <helper.h>
const char *greeting(void) { return LOUD ? "HELLO" : "hello"; }
"#,
            ),
            (
                "build.c",
                r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fprintf(file, "define helper LOUD=%d\n", getenv("CABIN_FEATURE_LOUD") != NULL);
    return fclose(file) != 0;
}
"#,
            ),
        ],
    );
    publish(
        "gen",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/gen"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[target.gen]
type = "executable"
sources = ["src/main.c"]
"#,
            ),
            ("src/main.c", "int main(void) { return 0; }\n"),
        ],
    );
    publish(
        "lib",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/lib"
version = "1.0.0"
license = "MIT"
c-standard = "c11"
include = ["src", "include"]

[dependencies]
"acme/helper" = "^1"

[build-dependencies]
"acme/helper" = { version = "^1", features = ["loud"] }
"acme/gen" = "^1"

[target.lib]
type = "library"
sources = ["src/lib.c"]
include-dirs = ["include"]
deps = ["acme/helper"]
"#,
            ),
            ("include/lib.h", "const char *lib_made(void);\n"),
            (
                "src/lib.c",
                r#"#include <helper.h>
#include <lib.h>
#include <stdio.h>
#include "generated.h"
const char *lib_made(void) {
    static char made[64];
    snprintf(made, sizeof made, "%s %s", greeting(), MADE);
    return made;
}
"#,
            ),
            (
                "build.c",
                r##"#define _CRT_SECURE_NO_WARNINGS
#include <helper.h>
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    const char *out = getenv("CABIN_OUT_DIR");
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/generated.h", out);
    if (!(file = fopen(path, "w"))) return 1;
    fprintf(file, "#define MADE \"%s %s\"\n", greeting(),
            getenv("CABIN_BIN_GEN") ? "tool" : "none");
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "include-dir lib %s\n", out);
    return fclose(file) != 0;
}
"##,
            ),
        ],
    );
    registry
}

/// `app`, which prints what `acme/lib` made.
fn write_registry_app(root: &Path) {
    let root = assert_fs::fixture::ChildPath::new(root);
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[dependencies]
"acme/lib" = "^1"

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["acme/lib"]
"#,
        )
        .unwrap();
    root.child("src/main.c")
        .write_str(
            r#"#include <lib.h>
#include <stdio.h>
int main(void) { puts(lib_made()); return 0; }
"#,
        )
        .unwrap();
}

/// A published package's build script runs for whoever depends on the
/// package, in each context with that context's features, with the
/// library and the executable of its registry build dependencies:
/// from a file registry, again with no network and the lockfile
/// fixed, from a vendored copy, and from an HTTP registry into an
/// empty cache.  `include` does not list the script, which is archived
/// regardless.
#[test]
fn a_published_packages_build_script_runs_for_its_dependents() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let registry = publish_packages_with_build_scripts(dir.path());
    let app = dir.path().join("app");
    write_registry_app(&app);
    let run = |args: &[&str], index: &std::ffi::OsStr, cache: &str| {
        let assert = cabin_in(&app, &["run"])
            .args(args)
            .arg(index)
            .arg("--cache-dir")
            .arg(dir.path().join(cache))
            .assert()
            .success();
        let printed = printed(&assert);
        for line in [
            "\nhello HELLO tool\n",
            "Running build script of acme/helper",
            "Running build script (build machine) of acme/helper",
            "Running build script of acme/lib",
        ] {
            assert!(printed.contains(line), "no `{line}` in: {printed}");
        }
    };

    run(&["--index-path"], registry.as_os_str(), "cache");
    fs::remove_dir_all(app.join("build")).unwrap();
    run(
        &["--offline", "--locked", "--index-path"],
        registry.as_os_str(),
        "cache",
    );

    let vendor = dir.path().join("vendor");
    cabin_in(&app, &["vendor", "--vendor-dir"])
        .arg(&vendor)
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    fs::remove_dir_all(app.join("build")).unwrap();
    run(
        &["--offline", "--locked", "--index-path"],
        vendor.as_os_str(),
        "vendor-cache",
    );

    fs::remove_dir_all(app.join("build")).unwrap();
    fs::remove_file(app.join("cabin.lock")).unwrap();
    let server = super::sparse_http::TestServer::serve(registry);
    run(&["--index-url"], server.url().as_ref(), "http-cache");
}

/// A respin may drop the build script.  A lockfile pinning the
/// revision before it keeps running the script that revision declares,
/// and `cabin update` moves to the revision without one.
#[test]
fn a_respin_that_drops_the_build_script_leaves_old_pins_running_it() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let registry = publish_packages_with_build_scripts(dir.path());
    let app = dir.path().join("app");
    write_registry_app(&app);
    let run = |args: &[&str]| {
        let assert = cabin_in(&app, &["run", "--index-path"])
            .arg(&registry)
            .args(args)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
        printed(&assert)
    };
    assert!(run(&[]).contains("\nhello HELLO tool\n"));

    let helper = dir.path().join("helper");
    fs::remove_file(helper.join("build.c")).unwrap();
    fs::write(
        helper.join("src/helper.c"),
        "#include <helper.h>\nconst char *greeting(void) { return \"quiet\"; }\n",
    )
    .unwrap();
    cabin()
        .args(["publish", "--new-revision", "--manifest-path"])
        .arg(helper.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    fs::remove_dir_all(app.join("build")).unwrap();
    let pinned = run(&["--locked"]);
    assert!(pinned.contains("\nhello HELLO tool\n"), "{pinned}");
    assert!(
        pinned.contains("Running build script of acme/helper"),
        "{pinned}"
    );

    cabin_in(&app, &["update", "--index-path"])
        .arg(&registry)
        .assert()
        .success();
    let updated = run(&[]);
    assert!(updated.contains("\nquiet quiet tool\n"), "{updated}");
    assert!(!updated.contains("of acme/helper"), "{updated}");
}

/// A `build.c` in a registry package's archive that the package's
/// index entry does not declare is not run: the one here would not
/// even compile.  An entry declaring a script the archive does not
/// hold under that name is refused.
#[test]
fn a_registry_build_script_the_index_does_not_declare_is_not_run() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("config.json"))
        .write_str("{\"schema\":2,\"kind\":\"file-registry\",\"packages\":\"packages\",\"archives\":\"archives\"}\n")
        .unwrap();
    let checksum = make_archive(
        &index.join("archives/dep/dep-1.0.0.zip"),
        &[
            (
                "cabin.toml",
                r#"[package]
name = "dep"
version = "1.0.0"
c-standard = "c11"

[target.dep]
type = "library"
sources = ["src/dep.c"]
"#,
            ),
            ("src/dep.c", "int dep(void) { return 0; }\n"),
            ("build.c", "#error this is not to be compiled\n"),
        ],
    );
    let revision = &checksum[..16];
    let entry = format!(
        "{{\"schema\": 1, \"name\": \"dep\", \"versions\": {{\"1.0.0\": {{\"dependencies\": {{}}, \"yanked\": false, \"standards\": {{\"targets\": {{\"dep\": {{\"interface\": {{\"c\": {{\"min\": \"c11\"}}}}}}}}}}, \"revision\": \"{revision}\", \"revisions\": {{\"{revision}\": {{\"checksum\": \"sha256:{checksum}\", \"published-at\": \"2026-01-01T00:00:00Z\", \"source\": {{\"type\": \"archive\", \"path\": \"../archives/dep/dep-1.0.0.zip\", \"format\": \"zip\"}}}}}}}}}}}}\n"
    );
    let entry_file = assert_fs::fixture::ChildPath::new(index.join("packages/dep.json"));
    entry_file.write_str(&entry).unwrap();
    let proj = assert_fs::fixture::ChildPath::new(dir.path().join("proj"));
    proj.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[dependencies]
dep = "1.0.0"

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["dep:dep"]
"#,
        )
        .unwrap();
    proj.child("src/main.c")
        .write_str("int dep(void);\nint main(void) { return dep(); }\n")
        .unwrap();

    let run = || {
        cabin_in(proj.path(), &["run", "--index-path"])
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
    };
    assert!(!printed(&run().success()).contains(RAN));

    entry_file
        .write_str(&entry.replace(
            "\"yanked\": false,",
            "\"yanked\": false, \"build-script\": \"build.cpp\",",
        ))
        .unwrap();
    run().failure().stderr(predicate::str::contains(
        "the index declares the build script `build.cpp` for package `dep`, which its archive does not hold",
    ));
}

/// A registry package's build script cannot put a file named like a
/// standard header where the package's dependents would include it.
#[test]
fn a_registry_build_script_cannot_shadow_a_standard_header() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    let shadow = assert_fs::fixture::ChildPath::new(dir.path().join("shadow"));
    shadow
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "acme/shadow"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[target.shadow]
type = "library"
sources = ["src/shadow.c"]
"#,
        )
        .unwrap();
    shadow
        .child("src/shadow.c")
        .write_str("int shadow(void) { return 0; }\n")
        .unwrap();
    shadow
        .child("build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    const char *out = getenv("CABIN_OUT_DIR");
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/stdint.h", out);
    if (!(file = fopen(path, "w"))) return 1;
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "include-dir shadow %s\n", out);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(shadow.path().join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    let app = assert_fs::fixture::ChildPath::new(dir.path().join("app"));
    app.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[dependencies]
"acme/shadow" = "^1"

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["acme/shadow"]
"#,
        )
        .unwrap();
    app.child("src/main.c")
        .write_str("int shadow(void);\nint main(void) { return shadow(); }\n")
        .unwrap();

    cabin_in(app.path(), &["build", "--index-path"])
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("so `#include <stdint.h>`"));
}

/// Two builds of one build directory at once both succeed: one runs
/// the script, under the directory's lock, and the other takes its
/// result.
#[test]
fn builds_at_once_share_a_build_scripts_run() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_app(dir.path(), &header_script("\"made\"", ""));
    let runs: usize = std::thread::scope(|scope| {
        let builds: Vec<_> = (0..2)
            .map(|_| scope.spawn(|| printed(&cabin_in(dir.path(), &["build"]).assert().success())))
            .collect();
        builds
            .into_iter()
            .map(|build| usize::from(build.join().unwrap().contains(RAN)))
            .sum()
    });
    assert_eq!(runs, 1);
    cabin_in(dir.path(), &["run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("made"));
}

/// A library can take every source from its build script, the first
/// build included.
#[test]
fn a_target_takes_every_source_from_its_build_script() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[target.answer]
type = "library"
sources = []
interface-c-standard = "c11"

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["answer"]
"#,
        )
        .unwrap();
    dir.child("src/main.c")
        .write_str(
            r#"#include <stdio.h>
int answer(void);
int main(void) { printf("%d\n", answer()); return 0; }
"#,
        )
        .unwrap();
    dir.child("build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/answer.c", getenv("CABIN_OUT_DIR"));
    if (!(file = fopen(path, "w"))) return 1;
    fputs("int answer(void) { return 42; }\n", file);
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "source answer %s\n", path);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    for _ in 0..2 {
        cabin_in(dir.path(), &["run"])
            .assert()
            .success()
            .stdout(predicate::str::contains("42"));
    }
}

/// A build needs the scripts of what it builds, and no other is built,
/// run, or asked what it links: `cabin check` leaves a build
/// dependency's alone when no script links it, and a build leaves alone
/// the script of a package it plans no target of, also when its own
/// script generates its every source.
#[test]
fn a_build_needs_only_the_build_scripts_of_what_it_builds() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["app", "gated", "made", "other", "solo"]
"#,
        )
        .unwrap();
    let generate = r##"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    char path[4096];
    FILE *file;
    snprintf(path, sizeof path, "%s/main.c", getenv("CABIN_OUT_DIR"));
    if (!(file = fopen(path, "w"))) return 1;
    fputs("#include <stdio.h>\nint main(void) { puts(\"made\"); return 0; }\n", file);
    fclose(file);
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "source made %s\n", path);
    return fclose(file) != 0;
}
"##;
    let broken = "#error not to be compiled\n";
    let listed = r#"sources = ["src/main.c"]"#;
    // Each package's script, the keys of its one executable, and the
    // rest of its manifest.
    for (name, script, target, rest) in [
        (
            "app",
            None,
            listed,
            r#"[build-dependencies]
gen = { path = "../gen" }
"#,
        ),
        ("gen", Some("int main(void) { return 0; }\n"), listed, ""),
        ("solo", None, listed, ""),
        ("made", Some(generate), "sources = []", ""),
        (
            "gated",
            Some(broken),
            r#"sources = ["src/main.c"]
required-features = ["on"]"#,
            "[features]\non = []\n",
        ),
        // Its script cannot tell which of two libraries to link.
        (
            "other",
            Some(broken),
            listed,
            r#"[build-dependencies]
two = { path = "../two" }
"#,
        ),
    ] {
        dir.child(format!("{name}/cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "{name}"
version = "0.1.0"
c-standard = "c11"

[target.{name}]
type = "executable"
{target}

{rest}"#
            ))
            .unwrap();
        dir.child(format!("{name}/src/main.c"))
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        if let Some(script) = script {
            dir.child(format!("{name}/build.c"))
                .write_str(script)
                .unwrap();
        }
    }
    dir.child("two/cabin.toml")
        .write_str(
            r#"[package]
name = "two"
version = "0.1.0"
c-standard = "c11"

[target.one]
type = "library"
sources = ["src/one.c"]

[target.another]
type = "library"
sources = ["src/another.c"]
"#,
        )
        .unwrap();
    for library in ["one", "another"] {
        dir.child(format!("two/src/{library}.c"))
            .write_str(&format!("int {library}(void) {{ return 0; }}\n"))
            .unwrap();
    }

    let checked = printed(
        &cabin_in(dir.path(), &["check", "-p", "app"])
            .assert()
            .success(),
    );
    assert!(!checked.contains(RAN), "{checked}");

    let ran = printed(
        &cabin_in(dir.path(), &["run", "--bin", "app"])
            .assert()
            .success(),
    );
    assert!(
        ran.contains("Running build script (build machine) of gen"),
        "{ran}"
    );
    assert!(!ran.contains("build script of"), "{ran}");

    let made = printed(
        &cabin_in(dir.path(), &["run", "--bin", "made"])
            .assert()
            .success(),
    );
    assert!(made.contains("Running build script of made"), "{made}");
    assert!(made.contains("\nmade\n"), "{made}");

    // No target of `gated` is built without its feature, which leaves
    // these two nothing to build for the build machine.
    cabin_in(dir.path(), &["build", "-p", "solo", "-p", "gated"])
        .assert()
        .success();

    cabin_in(dir.path(), &["build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("builds more than one library"));
}

/// Build scripts add no order of their own: packages that are each
/// other's build dependencies are a package cycle.
#[test]
fn build_scripts_needing_each_other_are_a_package_cycle() {
    let dir = TempDir::new().unwrap();
    for (name, other) in [("a", "b"), ("b", "a")] {
        write_app(&dir.path().join(name), "int main(void) { return 0; }\n");
        let manifest = dir.child(format!("{name}/cabin.toml"));
        let text = fs::read_to_string(manifest.path()).unwrap();
        manifest
            .write_str(&format!(
                "{}\n[build-dependencies]\n{other} = {{ path = \"../{other}\" }}\n",
                text.replace("\"app\"", &format!("\"{name}\""))
                    .replace("[target.app]", &format!("[target.{name}]"))
            ))
            .unwrap();
    }
    cabin_in(&dir.path().join("a"), &["build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "package dependency cycle detected: a -> b -> a",
        ));
}
