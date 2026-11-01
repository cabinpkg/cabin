//! Builds whose paths stress Ninja's manifest and depfile syntax,
//! checked against real tools: the generated `build.ninja` must parse, a
//! header edit must rebuild exactly its dependents, and an unchanged
//! rebuild must do no work.

use super::*;

fn write(dir: &Path, path: &str, body: &str) {
    assert_fs::fixture::ChildPath::new(dir.join(path))
        .write_str(body)
        .unwrap();
}

fn ninja(build_root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("ninja")
        .arg("-C")
        .arg(build_root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A C and C++ library whose sources share a header through another
/// header, one source that includes nothing, and an executable.
fn write_project(dir: &Path) {
    write(
        dir,
        "cabin.toml",
        r#"[package]
name = "fx"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.fx]
type = "library"
sources = ["src/a.cc", "src/b.c", "src/plain.cc"]
include-dirs = ["include"]

[target.app]
type = "executable"
sources = ["app/main.cc"]
deps = ["fx"]
"#,
    );
    write(dir, "include/fx/inner.h", "#define FX_INNER 1\n");
    write(
        dir,
        "include/fx/a.hpp",
        "#include \"fx/inner.h\"\nint fx_a();\nextern \"C\" int fx_b(void);\n",
    );
    write(
        dir,
        "src/a.cc",
        "#include \"fx/a.hpp\"\nint fx_a() { return FX_INNER; }\n",
    );
    write(
        dir,
        "src/b.c",
        "#include \"fx/inner.h\"\nint fx_b(void) { return FX_INNER; }\n",
    );
    write(dir, "src/plain.cc", "int fx_plain() { return 0; }\n");
    write(
        dir,
        "app/main.cc",
        "#include \"fx/a.hpp\"\nint main() { return fx_a() == fx_b() ? 0 : 1; }\n",
    );
}

#[test]
fn special_character_paths_rebuild_only_what_changed() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    // `;` and the others Ninja's depfile parser splits on, `|` that ends a
    // manifest path, and what both escape.  Windows file names cannot
    // hold `|`, `"`, `:`, `*`, `?`, `<`, or `>`.  There, characters outside
    // the active code page reach Ninja through `/showIncludes` as UTF-8,
    // and `Program Files` above the build directory does not get the
    // headers dropped as system headers.
    let names: &[&str] = if cfg!(windows) {
        &[
            "semi;colon",
            "single'quote",
            "sp ace$dollar#hash&amp^caret`tick",
            "Program Files ✓é日本語",
        ]
    } else {
        &[
            "semi;colon",
            "single'quote",
            "pi|pe",
            "sp ace$dollar:colon#hash\"dq&amp?q*star<lt>gt^caret`tick✓",
        ]
    };
    for name in names {
        let dir = tmp.path().join(name);
        write_project(&dir);
        cabin().arg("build").current_dir(&dir).assert().success();
        let root = dir.canonicalize().unwrap().join("build").join("dev");
        assert!(
            ninja(&root, &["-n"]).contains("no work to do"),
            "[{name}] an unchanged rebuild has work"
        );
        // MSVC dependencies come from `/showIncludes`, which Ninja records
        // relative to the build root.
        if !cfg!(windows) {
            let object = root.join("packages/fx/obj/fx/src/a.cc.o");
            let header = dir.canonicalize().unwrap().join("include/fx/inner.h");
            let deps = ninja(&root, &["-t", "deps", object.to_str().unwrap()]);
            assert!(deps.contains(header.to_str().unwrap()), "[{name}] {deps}");
        }

        write(&dir, "include/fx/inner.h", "#define FX_INNER 2\n");
        let plan = ninja(&root, &["-n"]);
        for object in ["a.cc.o", "b.c.o", "main.cc.o"] {
            assert!(
                plan.contains(object),
                "[{name}] {object} is not rebuilt: {plan}"
            );
        }
        assert!(!plan.contains("plain.cc.o"), "[{name}] {plan}");
        cabin().arg("build").current_dir(&dir).assert().success();
        assert!(ninja(&root, &["-n"]).contains("no work to do"), "[{name}]");
    }

    // GCC escapes a tab in a depfile and Clang writes it as is.  Ninja's
    // log separates its fields with tabs, so the outputs live elsewhere.
    if !cfg!(windows) {
        let dir = tmp.path().join("ta\tb");
        write_project(&dir);
        let build = tmp.path().join("tab-build");
        let build_tab = || {
            cabin()
                .arg("build")
                .arg("--build-dir")
                .arg(&build)
                .current_dir(&dir)
                .assert()
                .success();
        };
        build_tab();
        let root = build.canonicalize().unwrap().join("dev");
        assert!(ninja(&root, &["-n"]).contains("no work to do"));
        write(&dir, "include/fx/inner.h", "#define FX_INNER 2\n");
        assert!(ninja(&root, &["-n"]).contains("a.cc.o"));
        build_tab();
        assert!(ninja(&root, &["-n"]).contains("no work to do"));
    }

    // A syntax check runs `cabin stamp`, then `cabin depfile`, then the
    // compiler, and keeps the same dependencies.
    let dir = tmp.path().join(names[0]);
    cabin().arg("check").current_dir(&dir).assert().success();
    cabin()
        .args(["-v", "check"])
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));

    // A failing compile through `cabin depfile` fails the build and
    // recovers once fixed.
    write(&dir, "src/plain.cc", "int fx_plain() { return }\n");
    cabin().arg("build").current_dir(&dir).assert().failure();
    write(&dir, "src/plain.cc", "int fx_plain() { return 0; }\n");
    cabin().arg("build").current_dir(&dir).assert().success();
    cabin()
        .args(["-v", "build"])
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));
}

/// Where Ninja's own depfile parser reads a depfile right - names made
/// of the bytes every Ninja since 1.10 takes as plain text, with the
/// escapes compilers write - Ninja records the same dependencies through
/// `cabin depfile` and `deps = msvc` as through `deps = gcc`.
#[cfg(unix)]
#[test]
fn cabin_depfile_reads_what_ninja_reads() {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;

    const PLAIN: &[&str] = &[
        "a", "Z", "0", "+", ",", "_", ".", "~", "(", ")", "{", "}", "%", "=", "@", "[", "]", "!",
        "-", "é", "✓",
    ];
    const ESCAPED: &[&str] = &["\\ ", "\\\\\\ ", "\\#", "\\\\#", "$$", "\\x"];
    const SEPARATORS: &[&str] = &[" ", "  ", " \\\n ", " \\\r\n  "];

    // Seeded xorshift, so a failure reproduces.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % u64::try_from(bound).unwrap()).unwrap()
    };

    let tmp = TempDir::new().unwrap();
    let cabin = assert_cmd::cargo::cargo_bin("cabin");
    let mut manifest = format!(
        "rule gcc\n  command = touch $out\n  depfile = $out.d\n  deps = gcc\n\
         rule cabin\n  command = '{}' depfile $out.d -- touch $out\n  deps = msvc\n  \
         msvc_deps_prefix = cabin-depfile:\n",
        cabin
            .to_str()
            .unwrap()
            .replace('\'', "'\\''")
            .replace('$', "$$"),
    );
    for edge in 0..100 {
        let mut prerequisites = String::new();
        for _ in 0..=next(8) {
            prerequisites += SEPARATORS[next(SEPARATORS.len())];
            // A leading letter keeps a name from being `.` or `..`.
            prerequisites.push('w');
            for _ in 0..next(6) {
                prerequisites += if next(3) == 0 {
                    ESCAPED[next(ESCAPED.len())]
                } else {
                    PLAIN[next(PLAIN.len())]
                };
            }
        }
        for out in [format!("gcc{edge}"), format!("cabin{edge}")] {
            write(
                tmp.path(),
                &format!("{out}.d"),
                &format!("{out}:{prerequisites}\n"),
            );
        }
        writeln!(manifest, "build gcc{edge}: gcc\nbuild cabin{edge}: cabin").unwrap();
    }
    write(tmp.path(), "build.ninja", &manifest);
    ninja(tmp.path(), &[]);

    let deps = ninja(tmp.path(), &["-t", "deps"]);
    let mut recorded: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut target = "";
    for line in deps.lines() {
        if let Some(dep) = line.strip_prefix("    ") {
            recorded.entry(target).or_default().insert(dep);
        } else if let Some((name, _)) = line.split_once(": #deps") {
            target = name;
        }
    }
    for edge in 0..100 {
        let gcc = &recorded[format!("gcc{edge}").as_str()];
        assert!(!gcc.is_empty(), "{deps}");
        assert_eq!(gcc, &recorded[format!("cabin{edge}").as_str()], "{edge}");
    }
}

/// Ninja's `deps = msvc` drops every dependency under a `program files`
/// path, so a project there reruns the compiles `cabin depfile` reports
/// instead of going stale.
#[cfg(unix)]
#[test]
fn compiles_whose_dependencies_ninja_drops_rerun() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("Program Files/semi;colon");
    write_project(&dir);
    cabin().arg("build").current_dir(&dir).assert().success();
    let root = dir.canonicalize().unwrap().join("build").join("dev");
    let plan = ninja(&root, &["-n"]);
    assert!(plan.contains("a.cc.o"), "{plan}");
}

/// Archive and link commands past the host's command-line limit pass
/// their arguments through response files.  Other hosts cap only the
/// total argument size and keep them inline.
#[cfg(any(target_os = "linux", windows))]
#[test]
fn long_archive_and_link_commands_use_response_files() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("long paths");
    // On Linux, 150 objects of ~900 bytes each pass the 128 KiB a single
    // `/bin/sh -c` argument may hold.  Windows keeps each path under
    // `MAX_PATH`, so 900 objects of ~200 bytes pass both `CreateProcess`'s
    // 32,767 characters and the 128 KiB line MSVC tools read from a
    // response file.
    let segment = "s".repeat(if cfg!(windows) { 100 } else { 200 });
    let nested = [segment.as_str(); if cfg!(windows) { 1 } else { 4 }].join("/");
    let count = if cfg!(windows) { 900 } else { 150 };
    let sources = |prefix: &str| {
        (0..count)
            .map(|i| format!("\"{prefix}/{nested}/u{i}.c\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    write(
        &dir,
        "cabin.toml",
        &format!(
            "[package]\nname = \"big\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n\
             [target.big]\ntype = \"library\"\nsources = [{}]\n\n\
             [target.app]\ntype = \"executable\"\nsources = [\"app/main.c\", {}]\ndeps = [\"big\"]\n",
            sources("lib"),
            sources("app"),
        ),
    );
    for i in 0..count {
        write(
            &dir,
            &format!("lib/{nested}/u{i}.c"),
            &format!("int lib_{i}(void) {{ return {i}; }}\n"),
        );
        write(
            &dir,
            &format!("app/{nested}/u{i}.c"),
            &format!("int app_{i}(void) {{ return {i}; }}\n"),
        );
    }
    let last = count - 1;
    write(
        &dir,
        "app/main.c",
        &format!(
            "#include <stdio.h>\nint lib_{last}(void);\nint app_{last}(void);\n\
             int main(void) {{ printf(\"value=%d\\n\", lib_{last}() + app_{last}()); return 0; }}\n"
        ),
    );
    cabin()
        .arg("run")
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("value={}", 2 * last)));
    let manifest = fs::read_to_string(dir.join("build/dev/build.ninja")).unwrap();
    assert_eq!(manifest.matches("\n  rspfile = ").count(), 2, "{manifest}");
    if cfg!(windows) {
        assert_eq!(manifest.matches("\n  rsp_head = ").count(), 2, "{manifest}");
    }
    cabin()
        .args(["-v", "build"])
        .current_dir(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));
}
