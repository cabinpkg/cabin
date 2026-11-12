use std::io::{BufRead as _, BufReader};
use std::process::{Command as StdCommand, Stdio};

use super::*;

/// `cabin install --path <package> --root <root>`.
fn install(package: &Path, root: &Path) -> Command {
    let mut cmd = cabin();
    cmd.args(["install", "--path"])
        .arg(package)
        .arg("--root")
        .arg(root);
    cmd
}

fn installed(root: &Path, name: &str) -> PathBuf {
    root.join("bin").join(host_exe(name))
}

/// What the program at `path` prints.
fn output_of(path: &Path) -> String {
    let output = StdCommand::new(path).output().unwrap();
    assert!(output.status.success(), "{}: {output:?}", path.display());
    String::from_utf8(output.stdout)
        .unwrap()
        .replace("\r\n", "\n")
}

fn list(root: &Path) -> String {
    let assert = cabin()
        .args(["install", "--list", "--root"])
        .arg(root)
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn record(root: &Path) -> String {
    fs::read_to_string(root.join(".cabin-install/installs.json")).unwrap()
}

/// A package `name` at `dir`: a C program `one` and a C++ program
/// `two`, each printing its name and `greeting`, and a library.
fn write_tools(dir: &Path, name: &str, version: &str, greeting: &str) {
    let dir = assert_fs::fixture::ChildPath::new(dir);
    dir.child("cabin.toml")
        .write_str(&format!(
            r#"[package]
name = "{name}"
version = "{version}"
c-standard = "c11"
cxx-standard = "c++17"

[target.helper]
type = "library"
sources = ["src/helper.c"]

[target.one]
type = "executable"
sources = ["src/one.c"]

[target.two]
type = "executable"
sources = ["src/two.cc"]
"#
        ))
        .unwrap();
    dir.child("src/helper.c")
        .write_str("int helper(void) { return 0; }\n")
        .unwrap();
    dir.child("src/one.c")
        .write_str(&format!(
            r#"#include <stdio.h>
int main(void) {{
    puts("one {greeting}");
    return 0;
}}
"#
        ))
        .unwrap();
    dir.child("src/two.cc")
        .write_str(&format!(
            r#"#include <cstdio>
int main() {{
    std::puts("two {greeting}");
    return 0;
}}
"#
        ))
        .unwrap();
}

/// Only the user's configuration applies, the package's directory
/// gains nothing, and the programs run once the build is gone: from a
/// root whose path has a space and a non-ASCII letter.
#[test]
fn installed_programs_run_without_their_build() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let tools = dir.child("tools");
    write_tools(tools.path(), "tools", "0.1.0", "v1");
    // Read as configuration, either file fails the command.
    tools.child(".cabin/config.toml").write_str("[").unwrap();
    let caller = dir.child("caller");
    caller.child(".cabin/config.toml").write_str("[").unwrap();
    caller
        .child("cabin.toml")
        .write_str(VALID_MANIFEST)
        .unwrap();
    caller
        .child("cabin.lock")
        .write_str("the caller's\n")
        .unwrap();
    let before = dir_listing(tools.path());
    let temp = dir.child("temp");
    temp.create_dir_all().unwrap();
    let root = dir.path().join("my tools \u{e9}");

    cabin_with_config()
        .current_dir(caller.path())
        .args(["install", "--path", "../tools", "--root"])
        .arg(&root)
        .env("TMPDIR", temp.path())
        .env("TMP", temp.path())
        .env("TEMP", temp.path())
        .assert()
        .success();

    let left: Vec<String> = dir_listing(temp.path())
        .into_iter()
        .filter(|name| name.starts_with("cabin-install-"))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "the build directory stayed");
    assert_eq!(dir_listing(tools.path()), before);
    caller.child("cabin.toml").assert(VALID_MANIFEST);
    caller.child("cabin.lock").assert("the caller's\n");
    assert_eq!(output_of(&installed(&root, "one")), "one v1\n");
    assert_eq!(output_of(&installed(&root, "two")), "two v1\n");
    let listed = list(&root);
    assert!(listed.starts_with("tools v0.1.0 ("), "{listed}");
    let binaries = format!("):\n    {}\n    {}\n", host_exe("one"), host_exe("two"));
    assert!(listed.ends_with(&binaries), "{listed}");
}

/// The `PATH` warning: none while `<root>/bin` is on `PATH`; otherwise
/// what to add, for `--root` and for the default root, as the platform
/// has it.  Cabin itself writes no startup file.
#[test]
fn a_bin_directory_missing_from_path_gets_what_to_add() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let tools = dir.child("tools");
    write_tools(tools.path(), "tools", "0.1.0", "v1");
    let home = dir.child("home");
    home.child(".zshrc").write_str("# mine\n").unwrap();
    home.child(".bashrc").write_str("# mine\n").unwrap();
    let stderr_of = |cmd: &mut Command| {
        let assert = cmd
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .assert()
            .success();
        String::from_utf8(assert.get_output().stderr.clone()).unwrap()
    };

    let root = dir.path().join("root");
    let bin = root.join("bin");
    let path = std::env::join_paths(
        std::iter::once(bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let on_path = stderr_of(install(tools.path(), &root).env("PATH", path));
    assert!(!on_path.contains("is not in your PATH"), "{on_path}");

    let custom = stderr_of(install(tools.path(), &root).env("SHELL", "bash"));
    let warning = format!("cabin: warning: {} is not in your PATH.\n", bin.display());
    assert!(custom.contains(&warning), "{custom}");
    let default = stderr_of(
        cabin()
            .args(["install", "--path"])
            .arg(tools.path())
            .env("SHELL", "zsh"),
    );
    if cfg!(windows) {
        for stderr in [&custom, &default] {
            assert!(stderr.contains("add it to your user Path"), "{stderr}");
            assert!(!stderr.contains("export"), "{stderr}");
        }
        let default_bin = home.path().join(".cabin").join("bin");
        assert!(
            default.contains(&format!("\n    {}\n", default_bin.display())),
            "{default}"
        );
    } else {
        // How the directory is quoted follows where the temporary
        // directory is; the unit tests pin that.
        assert!(custom.contains("line to ~/.bash"), "{custom}");
        assert!(custom.contains("\n    export PATH=\""), "{custom}");
        assert!(
            default.contains(
                r#"cabin: warning: ~/.cabin/bin is not in your PATH.

To run installed programs from anywhere, add the following
line to ~/.zshrc:

    export PATH="$HOME/.cabin/bin:$PATH"

Then restart your shell.
"#
            ),
            "{default}"
        );
    }

    home.child(".zshrc").assert("# mine\n");
    home.child(".bashrc").assert("# mine\n");
    for startup_file in [".bash_profile", ".profile", ".zprofile", ".config/fish"] {
        home.child(startup_file).assert(predicate::path::missing());
    }
}

/// What the record holds is listed with its control characters escaped.
#[test]
fn list_escapes_what_the_record_holds() {
    let root = TempDir::new().unwrap();
    root.child(".cabin-install/installs.json")
        .write_str(
            r#"{
  "version": 1,
  "packages": [
    {
      "name": "tools",
      "version": "0.1.0",
      "source": { "path": "/src/\u001b[2Jtools" },
      "features": [],
      "profile": "release",
      "toolchain": { "compiler": "clang", "version": null },
      "dependencies": [],
      "binaries": [{ "name": "one", "sha256": "" }]
    }
  ]
}
"#,
        )
        .unwrap();
    let listed = list(root.path());
    assert!(!listed.contains('\u{1b}'), "{listed:?}");
    assert!(listed.ends_with("[2Jtools):\n    one\n"), "{listed:?}");
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn bin_selects_the_executables_to_install() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let tools = dir.path().join("tools");
    write_tools(&tools, "tools", "0.1.0", "v1");
    let root = dir.path().join("root");

    install(&tools, &root)
        .args(["--bin", "missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--bin `missing` was not found in package `tools`",
        ));
    install(&tools, &root)
        .args(["--bin", "helper"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--bin `helper` is a `library` target of package `tools`",
        ));
    let library = dir.child("library");
    library
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "library"
version = "0.1.0"
c-standard = "c11"

[target.library]
type = "library"
sources = ["src/library.c"]
"#,
        )
        .unwrap();
    library
        .child("src/library.c")
        .write_str("int library(void) { return 0; }\n")
        .unwrap();
    install(library.path(), &root)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "package `library` has no `executable` target to install",
        ));
    cabin()
        .args(["uninstall", "tools", "--root"])
        .arg(&root)
        .assert()
        .failure()
        .stderr(predicate::str::contains("package `tools` is not installed"));
    assert!(!root.exists(), "a refused command made the root");

    install(&tools, &root)
        .args(["--bin", "two", "--bin", "two"])
        .assert()
        .success();
    assert_eq!(dir_listing(&root.join("bin")), [host_exe("two")]);
    cabin()
        .args(["uninstall", "tools", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert_eq!(dir_listing(&root.join("bin")), Vec::<String>::new());
}

/// A workspace member installs as its workspace builds it, with the C
/// standard it takes from the workspace root, whatever workspace the
/// command runs from.  The record names the packages the build used:
/// not a dependency no enabled feature asks for.
#[test]
fn a_workspace_member_installs_as_its_workspace_builds_it() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let workspace = dir.child("workspace");
    let library = |name: &str, body: &str| {
        workspace
            .child(format!("{name}/cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "{name}"
version = "0.1.0"
c-standard = {{ workspace = true }}

[target.{name}]
type = "library"
sources = ["src/{name}.c"]
include-dirs = ["include"]
"#
            ))
            .unwrap();
        workspace
            .child(format!("{name}/include/{name}.h"))
            .write_str(&format!("const char *{name}(void);\n"))
            .unwrap();
        workspace
            .child(format!("{name}/src/{name}.c"))
            .write_str(&format!(
                r#"#include "{name}.h"
const char *{name}(void) {{ return "{body}"; }}
"#
            ))
            .unwrap();
    };
    library("greet", "a member");
    library("unused", "never");
    workspace
        .child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["app", "greet", "unused"]
c-standard = "c11"
"#,
        )
        .unwrap();
    workspace
        .child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = { workspace = true }

[dependencies]
greet = { path = "../greet" }
unused = { path = "../unused", optional = true }

[features]
extra = ["dep:unused"]

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["greet"]
"#,
        )
        .unwrap();
    workspace
        .child("app/src/main.c")
        .write_str(
            r#"#include <stdio.h>
#include "greet.h"
int main(void) {
    puts(greet());
    return 0;
}
"#,
        )
        .unwrap();
    let caller = dir.child("caller");
    caller
        .child("cabin.toml")
        .write_str("[workspace]\nmembers = []\n")
        .unwrap();
    let before = dir_listing(workspace.path());
    let root = dir.path().join("root");

    cabin()
        .current_dir(caller.path())
        .args(["install", "--path", "../workspace/app", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert_eq!(output_of(&installed(&root, "app")), "a member\n");
    assert_eq!(dir_listing(workspace.path()), before);
    let record = record(&root);
    assert!(record.contains(r#""name": "greet""#), "{record}");
    assert!(!record.contains("unused"), "{record}");
}

/// The program is built with a source its build script generates, by
/// the generator the package declares: an installed program of the
/// generator's name, first on `PATH`, plays no part.
#[test]
fn a_program_a_build_script_generates_for_installs() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let example = dir.child("example");
    example
        .copy_from(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/build-script-generator"),
            &["**"],
        )
        .unwrap();
    let root = dir.path().join("root");
    let other = dir.child("other-embed");
    other
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "embed"
version = "9.0.0"
c-standard = "c11"

[target.embed]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    other
        .child("src/main.c")
        .write_str("int main(void) { return 1; }\n")
        .unwrap();
    install(other.path(), &root).assert().success();
    let path = std::env::join_paths(
        std::iter::once(root.join("bin"))
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();

    install(&example.path().join("packages/app"), &root)
        .env("PATH", path)
        .assert()
        .success();
    // The embedded file's own line ending is its checkout's.
    assert_eq!(
        output_of(&installed(&root, "app")).trim_end(),
        "Hello from an embedded file"
    );
}

#[test]
fn installing_again_replaces_and_uninstall_removes() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let tools = dir.path().join("tools");
    let root = dir.path().join("root");
    write_tools(&tools, "tools", "0.1.0", "v1");
    install(&tools, &root).assert().success();

    write_tools(&tools, "tools", "0.2.0", "v2");
    install(&tools, &root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Replacing"));
    assert_eq!(output_of(&installed(&root, "one")), "one v2\n");
    assert!(list(&root).starts_with("tools v0.2.0 ("));

    cabin()
        .args(["uninstall", "tools", "--bin", "one", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert_eq!(dir_listing(&root.join("bin")), [host_exe("two")]);
    cabin()
        .args(["uninstall", "tools", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert_eq!(dir_listing(&root.join("bin")), Vec::<String>::new());
    assert_eq!(list(&root), "");
}

#[test]
fn a_file_in_the_way_stays_unless_forced() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let root = dir.path().join("root");
    let theirs = installed(&root, "hello");
    assert_fs::fixture::ChildPath::new(&theirs)
        .write_str("the user's")
        .unwrap();

    install(dir.path(), &root)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "exists and Cabin did not install it",
        ))
        .stderr(predicate::str::contains("--force"));
    assert_eq!(fs::read_to_string(&theirs).unwrap(), "the user's");

    install(dir.path(), &root).arg("--force").assert().success();
    assert_eq!(output_of(&theirs), "Hello from Cabin\n");
}

#[test]
fn a_failed_build_leaves_the_installation_alone() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let root = dir.path().join("root");
    install(dir.path(), &root).assert().success();
    let before = record(&root);

    dir.child("src/main.cc").write_str("int main( {\n").unwrap();
    install(dir.path(), &root).assert().failure();
    assert_eq!(output_of(&installed(&root, "hello")), "Hello from Cabin\n");
    assert_eq!(record(&root), before);
}

/// `--locked` takes the versions `cabin.lock` names or fails, and no
/// install writes that file.
#[test]
fn locked_needs_a_lockfile_that_still_holds() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let app = dir.path().join("app");
    let index = dir.path().join("index");
    let root = dir.path().join("root");
    write_app_using_fmt(
        dir.path(),
        Some("#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n"),
    );
    // Each entry is one version of `fmt` and its archive's checksum.
    let write_index = |versions: &[(&str, String)]| {
        let entries: Vec<String> = versions
            .iter()
            .map(|(version, hex)| {
                format!(
                    r#""{version}": {{
      "dependencies": {{}},
      "yanked": false,
      "revision": "{revision}",
      "revisions": {{
        "{revision}": {{
          "checksum": "sha256:{hex}",
          "published-at": "2026-01-01T00:00:00Z",
          "source": {{ "type": "archive", "path": "archives/fmt-{version}.zip", "format": "zip" }}
        }}
      }}
    }}"#,
                    revision = &hex[..16]
                )
            })
            .collect();
        assert_fs::fixture::ChildPath::new(index.join("fmt.json"))
            .write_str(&format!(
                r#"{{ "schema": 1, "name": "fmt", "versions": {{ {} }} }}"#,
                entries.join(",\n")
            ))
            .unwrap();
    };
    let archive_fmt = |version: &str| {
        let manifest = format!(
            r#"[package]
name = "fmt"
version = "{version}"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#
        );
        let source = format!(
            r#"#include <iostream>
#include "fmt.h"
void say_hello() {{ std::cout << "fmt {version}\n"; }}
"#
        );
        make_archive(
            &index.join(format!("archives/fmt-{version}.zip")),
            &[
                ("cabin.toml", &manifest),
                ("include/fmt.h", "#pragma once\nvoid say_hello();\n"),
                ("src/fmt.cc", &source),
            ],
        )
    };
    let old = ("10.2.1", archive_fmt("10.2.1"));
    write_index(std::slice::from_ref(&old));
    // The index is the user's configuration: `cabin install` takes no
    // index flag.
    let config = dir.child("config");
    config
        .child("config.toml")
        .write_str(&format!(
            "[registry]\nindex-path = {}\n",
            serde_json::to_string(index.to_str().unwrap()).unwrap()
        ))
        .unwrap();
    let install = || {
        let mut cmd = cabin_with_config();
        cmd.env("CABIN_CONFIG_HOME", config.path())
            .env("CABIN_CACHE_DIR", dir.path().join("cache"))
            .args(["install", "--path"])
            .arg(&app)
            .arg("--root")
            .arg(&root);
        cmd
    };
    let lockfile = app.join("cabin.lock");

    install()
        .arg("--locked")
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin.lock"));
    install()
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "registry dependencies: 1, without a lockfile",
        ));
    assert!(!lockfile.exists(), "an install wrote the lockfile");
    assert_eq!(output_of(&installed(&root, "app")), "fmt 10.2.1\n");
    assert!(record(&root).contains(r#""locked": false"#));

    cabin_with_config()
        .env("CABIN_CONFIG_HOME", config.path())
        .args(["resolve", "--manifest-path"])
        .arg(app.join("cabin.toml"))
        .assert()
        .success();
    let locked = fs::read_to_string(&lockfile).unwrap();
    // A newer version is not taken while the lockfile names an older
    // one, with or without `--locked`.
    write_index(&[old, ("10.9.0", archive_fmt("10.9.0"))]);
    for flags in [&["--locked"][..], &[]] {
        install()
            .args(flags)
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "registry dependencies: 1, 1 at the versions",
            ));
        assert_eq!(output_of(&installed(&root, "app")), "fmt 10.2.1\n");
        assert!(record(&root).contains(r#""locked": true"#));
    }

    // The manifest moves past what the lockfile holds.
    let manifest = fs::read_to_string(app.join("cabin.toml")).unwrap();
    fs::write(
        app.join("cabin.toml"),
        manifest.replace(">=10.0.0 <11.0.0", ">=10.9.0 <11.0.0"),
    )
    .unwrap();
    install().arg("--locked").assert().failure();
    assert_eq!(output_of(&installed(&root, "app")), "fmt 10.2.1\n");
    install().assert().success();
    assert_eq!(output_of(&installed(&root, "app")), "fmt 10.9.0\n");
    assert!(record(&root).contains(r#""locked": false"#));
    assert_eq!(fs::read_to_string(&lockfile).unwrap(), locked);
}

/// A user configuration under `dir` whose registry is `registry`:
/// `index-path = ...` or `index-url = ...`.
fn registry_config(dir: &Path, registry: &str) -> PathBuf {
    assert_fs::fixture::ChildPath::new(dir.join("config.toml"))
        .write_str(&format!("[registry]\n{registry}\n"))
        .unwrap();
    dir.to_owned()
}

/// `cabin` reading the user configuration in `config`, with `cache`
/// as its cache.
fn cabin_configured(config: &Path, cache: &Path) -> Command {
    let mut cmd = cabin_with_config();
    cmd.env("CABIN_CONFIG_HOME", config)
        .env("CABIN_CACHE_DIR", cache);
    cmd
}

/// A published program installs by its name in the registry, and a
/// project builds with the same package as its `[build-dependencies]`
/// generator.  Neither needs the other: the project builds before
/// anything is installed, and each fills a cache of its own.
/// `@<req>` installs the newest version the requirement allows, a
/// request the registry cannot satisfy changes nothing, and an HTTP
/// registry serves as a directory does.
#[test]
fn a_registry_package_installs_by_name_and_generates_for_a_project() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let packages = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/build-script-generator/packages");
    let embed = dir.child("embed");
    embed.copy_from(packages.join("embed"), &["**"]).unwrap();
    let app = dir.child("app");
    app.copy_from(packages.join("app"), &["**"]).unwrap();
    let rewrite = |file: PathBuf, from: &str, to: &str| {
        let text = fs::read_to_string(&file).unwrap();
        fs::write(file, text.replace(from, to)).unwrap();
    };
    rewrite(
        embed.path().join("cabin.toml"),
        r#"name = "embed""#,
        "name = \"acme/embed\"\nlicense = \"MIT\"",
    );
    rewrite(
        app.path().join("cabin.toml"),
        r#"embed = { path = "../embed" }"#,
        r#""acme/embed" = "0.1.0""#,
    );
    let registry = dir.path().join("registry");
    let publish = || {
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(embed.path().join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .assert()
            .success();
    };
    publish();
    let config = registry_config(
        &dir.path().join("config"),
        &format!(
            "index-path = {}",
            serde_json::to_string(registry.to_str().unwrap()).unwrap()
        ),
    );
    let root = dir.path().join("root");

    cabin_configured(&config, &dir.path().join("project-cache"))
        .current_dir(app.path())
        .arg("run")
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from an embedded file"));

    let install = |spec: &str| {
        let mut cmd = cabin_configured(&config, &dir.path().join("install-cache"));
        cmd.args(["install", spec, "--root"]).arg(&root);
        cmd
    };
    // What the installed generator makes of the project's file.
    let generated = |root: &Path| {
        let output = StdCommand::new(installed(root, "embed"))
            .arg(app.path().join("assets/banner.txt"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    install("acme/embed").assert().success();
    assert!(generated(&root).starts_with("static const unsigned char embedded[]"));
    assert!(!generated(&root).contains("0.2.0"));

    rewrite(embed.path().join("cabin.toml"), "0.1.0", "0.2.0");
    rewrite(
        embed.path().join("src/main.c"),
        r#"puts("0};");"#,
        r#"puts("0}; // 0.2.0");"#,
    );
    publish();
    install("acme/embed").assert().success();
    assert!(generated(&root).contains("0.2.0"));
    let listed = list(&root);
    assert!(listed.starts_with("acme/embed v0.2.0 (sha256:"), "{listed}");

    install("acme/embed@0.1")
        .arg("--offline")
        .assert()
        .success();
    for (spec, message) in [
        ("acme/embed@0.3", r#"no version of "acme/embed" satisfies"#),
        ("acme/none", r#"package "acme/none" was not found"#),
        ("acme/embed@new", "invalid version requirement `new`"),
        ("embed", "`embed` is a bare name"),
    ] {
        install(spec)
            .assert()
            .failure()
            .stderr(predicate::str::contains(message));
    }
    assert!(!generated(&root).contains("0.2.0"));
    let listed = list(&root);
    assert!(listed.starts_with("acme/embed v0.1.0 (sha256:"), "{listed}");
    let record = record(&root);
    assert!(record.contains(r#""registry""#), "{record}");

    let server = super::sparse_http::TestServer::serve(registry);
    let config = registry_config(
        &dir.path().join("http-config"),
        &format!("index-url = \"{}\"", server.url()),
    );
    let root = dir.path().join("http-root");
    cabin_configured(&config, &dir.path().join("http-cache"))
        .args(["install", "acme/embed", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert!(generated(&root).contains("0.2.0"));
}

/// A registry package is built with the features asked for, by the
/// build script its index entry declares: only the script defines what
/// the program prints.
#[test]
fn a_registry_packages_features_and_build_script_apply() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let say = dir.child("say");
    say.child("cabin.toml")
        .write_str(
            r#"[package]
name = "acme/say"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[features]
default = ["loud"]
loud = []
twice = []

[target.say]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    say.child("build.c")
        .write_str(
            r#"#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    FILE *file = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!file) return 1;
    fprintf(file, "define say LOUD=%d\n", getenv("CABIN_FEATURE_LOUD") != NULL);
    fprintf(file, "define say TWICE=%d\n", getenv("CABIN_FEATURE_TWICE") != NULL);
    return fclose(file) != 0;
}
"#,
        )
        .unwrap();
    say.child("src/main.c")
        .write_str(
            r#"#include <stdio.h>
int main(void) {
    printf("%s%s\n", LOUD ? "HELLO" : "hello", TWICE ? " again" : "");
    return 0;
}
"#,
        )
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(say.path().join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    let config = registry_config(
        &dir.path().join("config"),
        &format!(
            "index-path = {}",
            serde_json::to_string(registry.to_str().unwrap()).unwrap()
        ),
    );
    let root = dir.path().join("root");
    let install = |flags: &[&str]| {
        cabin_configured(&config, &dir.path().join("cache"))
            .args(["install", "acme/say", "--root"])
            .arg(&root)
            .args(flags)
            .assert()
            .success();
        output_of(&installed(&root, "say"))
    };

    assert_eq!(install(&[]), "HELLO\n");
    assert_eq!(install(&["--no-default-features"]), "hello\n");
    assert_eq!(
        install(&["--no-default-features", "--features", "twice"]),
        "hello again\n"
    );
    let record = record(&root);
    assert!(record.contains(r#""twice""#), "{record}");
    assert!(!record.contains(r#""loud""#), "{record}");

    cabin()
        .args(["uninstall", "acme/say", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert!(!installed(&root, "say").exists());
}

#[test]
fn two_installs_at_once_both_land() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root");
    let commands: Vec<Command> = [("first", "one"), ("second", "two")]
        .into_iter()
        .map(|(name, bin)| {
            let package = dir.path().join(name);
            write_tools(&package, name, "0.1.0", name);
            let mut cmd = install(&package, &root);
            cmd.args(["--bin", bin]);
            cmd
        })
        .collect();
    std::thread::scope(|scope| {
        for mut cmd in commands {
            scope.spawn(move || cmd.assert().success());
        }
    });
    let listed = list(&root);
    assert!(listed.contains("first v0.1.0 ("), "{listed}");
    assert!(listed.contains("second v0.1.0 ("), "{listed}");
}

/// Windows refuses to overwrite or delete a running program's file;
/// Cabin moves it aside.
#[test]
fn a_running_program_is_replaced() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root");
    let write = |greeting: &str| {
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "waiter"
version = "0.1.0"
c-standard = "c11"

[target.waiter]
type = "executable"
sources = ["src/main.c"]
"#,
            )
            .unwrap();
        dir.child("src/main.c")
            .write_str(&format!(
                r#"#include <stdio.h>
int main(void) {{
    puts("{greeting}");
    fflush(stdout);
    return getchar() == EOF ? 0 : 1;
}}
"#
            ))
            .unwrap();
    };
    // Start the installed program and wait for its first line.
    let start = || {
        let mut child = StdCommand::new(installed(&root, "waiter"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        (child, line)
    };
    write("first");
    install(dir.path(), &root).assert().success();
    let (mut running, line) = start();
    assert_eq!(line.trim_end(), "first");

    write("second");
    install(dir.path(), &root).assert().success();
    let (mut replaced, line) = start();
    assert_eq!(line.trim_end(), "second");
    for child in [&mut running, &mut replaced] {
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }

    // Windows may hold an exited program's file a moment longer, so
    // when the file moved aside is deleted is not this test's to say.
    cabin()
        .args(["uninstall", "waiter", "--root"])
        .arg(&root)
        .assert()
        .success();
    assert_eq!(dir_listing(&root.join("bin")), Vec::<String>::new());
}
