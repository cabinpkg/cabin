//! Schema-lock tests for the bundled miniz foundation port - the
//! first zip-sourced port - plus a hermetic end-to-end build of a
//! fake zip port over a `file://` URL.  The real-upstream build/run
//! path is covered by `cabin_examples.rs::miniz_usage_builds_and_runs`.

use std::io::Write as _;

use super::*;

#[test]
fn port_toml_schema_for_real_ports_miniz_matches_published_values() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
    let port_toml = PathBuf::from(manifest_dir)
        .join("../cabin-port/ports/miniz/3.1.2/port.toml")
        .canonicalize()
        .expect("canonicalize ports/miniz/3.1.2/port.toml");
    let descriptor =
        cabin_port::load_port(&port_toml).expect("ports/miniz/3.1.2/port.toml should parse");
    assert_eq!(descriptor.name.as_str(), "miniz");
    assert_eq!(descriptor.version, semver::Version::new(3, 1, 2));
    match &descriptor.source {
        cabin_port::PortSource::Archive {
            url,
            sha256,
            strip_prefix,
        } => {
            // Upstream's only official release artifact is the
            // amalgamated zip; the URL extension is what opts the
            // port into the zip extraction path.
            assert!(
                url.as_str().to_ascii_lowercase().ends_with(".zip"),
                "expected a .zip URL, got {url}"
            );
            assert_eq!(
                cabin_port::ArchiveKind::from_url(url),
                cabin_port::ArchiveKind::Zip
            );
            assert_eq!(url.scheme(), "https");
            assert_eq!(sha256.to_hex().len(), 64);
            // The amalgamated zip has no root directory.
            assert_eq!(strip_prefix.as_deref(), None);
        }
    }
    assert_eq!(
        descriptor.overlay.relative_path,
        PathBuf::from("cabin.toml")
    );
    assert_eq!(descriptor.metadata.license.as_deref(), Some("MIT"));
}

#[test]
fn miniz_is_bundled_and_parses() {
    let entry = cabin_port::builtin::lookup("miniz", &semver::VersionReq::parse("^3.1").unwrap())
        .expect("miniz should be bundled");
    assert_eq!(entry.name, "miniz");
    assert_eq!(entry.version, "3.1.2");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:miniz>/port.toml"),
    )
    .expect("embedded miniz port.toml parses");
    assert_eq!(descriptor.name.as_str(), "miniz");
    assert_eq!(descriptor.version.to_string(), "3.1.2");
}

#[test]
fn miniz_overlay_declares_single_amalgamated_library_target() {
    let entry =
        cabin_port::builtin::lookup("miniz", &semver::VersionReq::parse(">=0").unwrap()).unwrap();
    let overlay = entry.overlay_toml;
    assert!(overlay.contains("[target.miniz]"), "overlay: {overlay}");
    assert!(overlay.contains("type = \"library\""), "overlay: {overlay}");
    assert!(
        overlay.contains("sources = [\"miniz.c\"]"),
        "overlay: {overlay}"
    );
    assert!(
        overlay.contains("include-dirs = [\".\"]"),
        "overlay: {overlay}"
    );
    // Upstream's bundled sample programs stay unbuilt.
    assert!(
        !overlay.contains("\"examples/"),
        "overlay should not build upstream examples: {overlay}"
    );
}

/// Hermetic end-to-end proof of the zip source-archive path: a fake
/// single-file C library shipped as a local zip (via `file://`)
/// prepares, builds, and links into a consumer executable - no
/// network involved.  Mirrors `foundation_port_zlib`'s tarball
/// lifecycle tests for the zip format.
#[test]
fn fake_zip_port_builds_downstream_consumer() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();

    // Lay the fake upstream zip: a root-less archive like miniz's
    // release asset (header + single TU at the archive root).
    let downloads = tmp.path().join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let zip_path = downloads.join("fakeminiz-1.0.0.zip");
    let f = std::fs::File::create(&zip_path).unwrap();
    let mut writer = zip::ZipWriter::new(f);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("fakeminiz.h", options).unwrap();
    writer
        .write_all(b"#ifndef FAKEMINIZ_H\n#define FAKEMINIZ_H\nconst char *fakeminiz_version(void);\n#endif\n")
        .unwrap();
    writer.start_file("fakeminiz.c", options).unwrap();
    writer
        .write_all(b"#include \"fakeminiz.h\"\nconst char *fakeminiz_version(void) { return \"1.0.0\"; }\n")
        .unwrap();
    writer.finish().unwrap();
    let hex = {
        use sha2::{Digest, Sha256};
        let bytes = fs::read(&zip_path).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        cabin_core::hash::hex_digest(&hasher.finalize())
    };
    let archive_url = url::Url::from_file_path(&zip_path).unwrap();

    // Local port recipe pointing at the zip.
    let port_dir = tmp.path().join("ports/fakeminiz/1.0.0");
    assert_fs::fixture::ChildPath::new(port_dir.join("port.toml"))
        .write_str(&format!(
            "[port]\nname = \"fakeminiz\"\nversion = \"1.0.0\"\n\n[source]\ntype = \"archive\"\nurl = \"{archive_url}\"\nsha256 = \"{hex}\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n"
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(port_dir.join("cabin.toml"))
        .write_str(
            "[package]\nname = \"fakeminiz\"\nversion = \"1.0.0\"\n\n[target.fakeminiz]\ntype = \"library\"\nsources = [\"fakeminiz.c\"]\ninclude-dirs = [\".\"]\n",
        )
        .unwrap();

    // Consumer package.
    let consumer_manifest = tmp.path().join("consumer/cabin.toml");
    assert_fs::fixture::ChildPath::new(&consumer_manifest)
        .write_str(
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nfakeminiz = { port-path = \"../ports/fakeminiz/1.0.0\" }\n\n[target.consumer]\ntype = \"executable\"\nsources = [\"src/main.c\"]\ndeps = [\"fakeminiz\"]\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("consumer/src/main.c"))
        .write_str(
            "#include <fakeminiz.h>\n#include <stdio.h>\n\nint main(void) {\n    puts(fakeminiz_version());\n    return 0;\n}\n",
        )
        .unwrap();

    let build_dir = tmp.path().join("build");
    let cache_dir = tmp.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&consumer_manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .arg("--cache-dir")
        .arg(&cache_dir)
        .assert()
        .success();

    // The zip must have been cached under its own extension.
    let cached = cache_dir
        .join("ports/archives/sha256")
        .join(format!("{hex}.zip"));
    assert!(
        cached.is_file(),
        "expected cached zip at {}",
        cached.display()
    );

    let exe_name = format!("consumer{}", std::env::consts::EXE_SUFFIX);
    let exe = build_dir.join("dev/packages/consumer").join(&exe_name);
    let output = std::process::Command::new(&exe)
        .output()
        .expect("run consumer");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1.0.0"), "stdout = {stdout:?}");
}
