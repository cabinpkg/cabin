use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::Digest;
use std::fs::File;
use std::io::Write;

fn manifest_for(name: &str, version: &str, deps: &[(&str, &str)]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    writeln!(out, "[package]\nname = \"{name}\"\nversion = \"{version}\"").unwrap();
    if !deps.is_empty() {
        out.push_str("\n[dependencies]\n");
        for (name, req) in deps {
            writeln!(out, "{name} = \"{req}\"").unwrap();
        }
    }
    out
}

/// Build a `.tar.gz` containing the given file entries (relative
/// path -> body). Returns the archive path and its `sha256` hex.
/// Same as [`make_archive`] but the caller chooses the entry type
/// and writes the path bytes directly so we can construct unsafe
/// archive entries that the tar crate's safe API would refuse.
fn make_archive_with_raw_name(
    path: &Path,
    raw_name: &str,
    entry_type: tar::EntryType,
    body: &[u8],
) -> String {
    if let Some(parent) = path.parent() {
        assert_fs::fixture::ChildPath::new(parent)
            .create_dir_all()
            .unwrap();
    }
    let f = File::create(path).unwrap();
    let enc = GzEncoder::new(f, Compression::default());
    let mut builder = tar::Builder::new(enc);
    let mut header = tar::Header::new_old();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(entry_type);
    {
        let bytes = raw_name.as_bytes();
        let old = header.as_old_mut();
        for b in &mut old.name[..] {
            *b = 0;
        }
        let n = bytes.len().min(old.name.len());
        old.name[..n].copy_from_slice(&bytes[..n]);
    }
    header.set_cksum();
    builder.append(&header, body).unwrap();
    let enc = builder.into_inner().unwrap();
    enc.finish().unwrap().flush().unwrap();
    sha256_hex(path)
}

fn sha256_hex(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    cabin_core::hash::hex_digest(&hasher.finalize())
}

fn fmt_archive_entries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("cabin.toml", FMT_PKG_MANIFEST),
        ("include/fmt.h", FMT_HEADER),
        ("src/fmt.cc", FMT_SRC),
    ]
}

const FMT_PKG_MANIFEST: &str = r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#;

const FMT_HEADER: &str = "#pragma once\nvoid say_hello();\n";

const FMT_SRC: &str = "#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n";

const APP_MAIN: &str = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";

/// Write an `app/` package whose root manifest depends on
/// `fmt = ">=10 <11"` plus a `[target.app]` linking against `fmt`.
fn write_app_using_fmt(dir: &Path) {
    let manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#;
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(manifest)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
        .write_str(APP_MAIN)
        .unwrap();
}

#[test]
fn fetch_extracts_registry_package_into_cache() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();

    // Lockfile written next to root manifest.
    let lock_path = dir.path().join("app/cabin.lock");
    assert!(lock_path.is_file(), "cabin.lock should exist");
    let lock_body = fs::read_to_string(&lock_path).unwrap();
    assert!(lock_body.contains(r#"name = "fmt""#));
    assert!(lock_body.contains(&format!("checksum = \"sha256:{hex}\"")));

    // Archive present in the checksum-addressed cache.
    let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
    assert!(archive_in_cache.is_file(), "archive should be cached");
    // Source extracted with cabin.toml at root.
    let source_in_cache = cache.join("sources/sha256").join(&hex);
    assert!(source_in_cache.join("cabin.toml").is_file());
}

#[test]
fn fetch_emits_json_when_requested() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );
    let cache = dir.path().join("cache");
    let value = run_json(
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .args(["--format", "json"]),
    );
    let pkgs = value["packages"].as_array().unwrap();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0]["name"], "fmt");
    assert_eq!(pkgs[0]["version"], "10.2.1");
    assert_eq!(pkgs[0]["checksum"], format!("sha256:{hex}"));
}

#[test]
fn build_links_against_registry_package() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    let build_dir = dir.path().join("build");
    let cache = dir.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();

    assert!(build_dir.join("dev").join("build.ninja").is_file());
    assert!(
        build_dir
            .join("dev")
            .join("compile_commands.json")
            .is_file()
    );
    let exe = build_dir.join("dev/packages/app").join(host_exe("app"));
    assert!(exe.is_file(), "executable should exist at {exe:?}");
    let output = std::process::Command::new(&exe).output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));

    // The registry package's headers are third-party code: the
    // consumer's compile marks fmt's extracted include dir as a
    // *system* search path, while fmt's own translation units keep
    // seeing it as a plain `-I` include.
    let ccdb: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(build_dir.join("dev/compile_commands.json")).unwrap(),
    )
    .unwrap();
    // Each entry stores its shell-joined `command`; temp-dir paths
    // carry no whitespace, so token scanning stays faithful. The
    // shell quoting wraps Windows paths in double quotes and escapes
    // each `\` as `\\`, so strip the quotes and collapse the doubled
    // separators after normalizing to `/`.
    let command_tokens_for = |suffix: &str| -> Vec<String> {
        ccdb.as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["file"]
                    .as_str()
                    .is_some_and(|f| f.replace('\\', "/").ends_with(suffix))
            })
            .unwrap_or_else(|| panic!("compile entry for {suffix} present"))["command"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .map(|t| t.trim_matches('"').replace('\\', "/").replace("//", "/"))
            .collect()
    };
    let value_of = |tokens: &[String], flag: &str| -> Option<String> {
        tokens
            .iter()
            .position(|t| t == flag)
            .map(|i| tokens[i + 1].clone())
    };
    // The Windows runner builds through the MSVC dialect, which
    // spells the two buckets `/I` and `/external:I` instead of the
    // GNU `-I` / `-isystem`.
    let (user_flag, system_flag) = if cfg!(windows) {
        ("/I", "/external:I")
    } else {
        ("-I", "-isystem")
    };

    let app_tokens = command_tokens_for("src/main.cc");
    let system_dir = value_of(&app_tokens, system_flag)
        .expect("app compile must mark fmt's include dir as a system include");
    assert!(
        system_dir.contains("sources/sha256") && system_dir.ends_with("/include"),
        "system include must point into the extracted cache: {system_dir}",
    );
    assert!(
        value_of(&app_tokens, user_flag).is_none(),
        "app compile must not also spell fmt's include dir as a user include: {app_tokens:?}",
    );

    let fmt_tokens = command_tokens_for("src/fmt.cc");
    assert_eq!(
        value_of(&fmt_tokens, user_flag).as_deref(),
        Some(system_dir.as_str()),
        "fmt's own compile keeps its include dir as a user include",
    );
    assert!(
        !fmt_tokens.iter().any(|t| t == system_flag),
        "fmt's own compile must not mark its own headers as system: {fmt_tokens:?}",
    );
}

#[test]
fn build_handles_transitive_registry_dependency() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    // Root depends only on spdlog; spdlog depends on fmt.
    let app_manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
spdlog = ">=1.0.0 <2.0.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["spdlog"]
"#;
    let app_main = "#include \"spdlog.h\"\nint main() { log_hello(); return 0; }\n";
    dir.child("app/cabin.toml").write_str(app_manifest).unwrap();
    dir.child("app/src/main.cc").write_str(app_main).unwrap();

    // fmt archive (library).
    let fmt_archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let fmt_hex = make_archive(&fmt_archive, &fmt_archive_entries());

    // spdlog archive: library that depends on fmt.
    let spdlog_manifest = r#"[package]
name = "spdlog"
version = "1.13.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[target.spdlog]
type = "library"
sources = ["src/spdlog.cc"]
include-dirs = ["include"]
deps = ["fmt"]
"#;
    let spdlog_header = "#pragma once\nvoid log_hello();\n";
    let spdlog_src =
        "#include \"spdlog.h\"\n#include \"fmt.h\"\nvoid log_hello() { say_hello(); }\n";
    let spdlog_archive = dir.path().join("artifacts/spdlog-1.13.0.tar.gz");
    let spdlog_hex = make_archive(
        &spdlog_archive,
        &[
            ("cabin.toml", spdlog_manifest),
            ("include/spdlog.h", spdlog_header),
            ("src/spdlog.cc", spdlog_src),
        ],
    );

    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &fmt_hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );
    write_index_entry(
        &dir.path().join("index"),
        "spdlog",
        "1.13.0",
        r#"{ "fmt": ">=10.0.0 <11.0.0" }"#,
        &spdlog_hex,
        "../artifacts/spdlog-1.13.0.tar.gz",
    );

    let build_dir = dir.path().join("build");
    let cache = dir.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();

    // Both packages should have been fetched and built.
    assert!(
        cache
            .join("sources/sha256")
            .join(&fmt_hex)
            .join("cabin.toml")
            .is_file()
    );
    assert!(
        cache
            .join("sources/sha256")
            .join(&spdlog_hex)
            .join("cabin.toml")
            .is_file()
    );
    assert!(
        build_dir
            .join("dev/packages/fmt")
            .join(host_static_lib("fmt"))
            .is_file()
    );
    assert!(
        build_dir
            .join("dev/packages/spdlog")
            .join(host_static_lib("spdlog"))
            .is_file()
    );
    assert!(
        build_dir
            .join("dev/packages/app")
            .join(host_exe("app"))
            .is_file()
    );
}

#[test]
fn fetch_fails_on_checksum_mismatch() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    make_archive(&archive, &fmt_archive_entries());
    // Index advertises a checksum that doesn't match the archive's
    // actual bytes.
    let bogus_hex = "0".repeat(64);
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &bogus_hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("checksum mismatch"));
}

#[test]
fn fetch_rejects_unsafe_archive() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex =
        make_archive_with_raw_name(&archive, "../escape.txt", tar::EntryType::Regular, b"evil");
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsafe archive entry"));
    // Nothing escaped the cache.
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn fetch_fails_when_index_has_no_source() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry_no_source(&dir.path().join("index"), "fmt", "10.2.1", &hex);

    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no source artifact"));
}

#[test]
fn frozen_uses_cache_after_initial_fetch() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    let cache = dir.path().join("cache");
    // Populate cache normally.
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
    // Now move the source archive away and re-run with --frozen;
    // cache hit should let it succeed.
    fs::remove_file(&archive).unwrap();
    cabin()
        .args(["fetch", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
}

#[test]
fn frozen_fails_on_cache_miss() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );
    // Pre-populate a lockfile so --frozen can run resolution.
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();

    let empty_cache = dir.path().join("empty-cache");
    cabin()
        .args(["fetch", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&empty_cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--frozen"))
        .stderr(predicate::str::contains("not cached"));
}

#[test]
fn frozen_does_not_write_lockfile_or_cache() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    // No lockfile, no cache pre-populated. --frozen must refuse.
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure();
    // Lockfile must not have been created by the failed run.
    assert!(!dir.path().join("app/cabin.lock").exists());
    // Cache must not have been populated by the failed run.
    let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
    assert!(!archive_in_cache.exists());
}

#[test]
fn fetch_fails_when_archive_manifest_disagrees() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    // Archive declares fmt 10.1.0 but the index promises 10.2.1.
    let mut entries = fmt_archive_entries();
    entries[0].1 = r#"[package]
name = "fmt"
version = "10.1.0"
"#;
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &entries);
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("contains package"));
}

#[test]
fn fetch_with_no_versioned_deps_succeeds() {
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_for("solo", "0.1.0", &[]))
        .unwrap();
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success()
        .stdout(predicate::str::contains("(no registry dependencies"));
}

#[test]
fn build_uses_separate_cache_dir_when_specified() {
    let dir = TempDir::new().unwrap();
    write_app_using_fmt(dir.path());
    let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(&archive, &fmt_archive_entries());
    write_index_entry(
        &dir.path().join("index"),
        "fmt",
        "10.2.1",
        "{}",
        &hex,
        "../artifacts/fmt-10.2.1.tar.gz",
    );

    let cache = dir.path().join("alt-cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
    assert!(
        cache
            .join("archives/sha256")
            .join(format!("{hex}.tar.gz"))
            .is_file()
    );
    // Default cache must NOT have been populated.
    assert!(!dir.path().join("app/.cabin/cache").exists());
}
