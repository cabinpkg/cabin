//! Hermetic end-to-end smoke tests for every bundled foundation port.
//! These tests use fake archives over loopback HTTP so default CI
//! proves Cabin's port build path without depending on public internet
//! availability.  Real upstream compatibility remains covered by the
//! ignored `cabin_examples` smoke tests and the scheduled/manual
//! workflow.

use super::*;

const FAKE_ZLIB_H: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/zlib/zlib.h");
const FAKE_ZLIB_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/zlib/zutil.c");
const ZLIB_OVERLAY: &str = include_str!("../../../cabin-port/ports/zlib/1.3.1/cabin.toml");
const ZLIB_MAIN_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/zlib/main.c");

const FAKE_CJSON_H: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/cJSON/cJSON.h");
const FAKE_CJSON_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/cJSON/cJSON.c");
const CJSON_OVERLAY: &str = include_str!("../../../cabin-port/ports/cJSON/1.7.18/cabin.toml");
const CJSON_MAIN_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/cJSON/main.c");

const FAKE_XXHASH_H: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/xxhash/xxhash.h");
const FAKE_XXHASH_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/xxhash/xxhash.c");
const XXHASH_OVERLAY: &str = include_str!("../../../cabin-port/ports/xxhash/0.8.3/cabin.toml");
const XXHASH_MAIN_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/xxhash/main.c");

const FAKE_TINYXML2_H: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/tinyxml2/tinyxml2.h");
const FAKE_TINYXML2_CPP: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/tinyxml2/tinyxml2.cpp");
const TINYXML2_OVERLAY: &str = include_str!("../../../cabin-port/ports/tinyxml2/11.0.0/cabin.toml");
const TINYXML2_MAIN_CPP: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/tinyxml2/main.cpp");

const FAKE_SQLITE3_H: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/sqlite3/sqlite3.h");
const FAKE_SQLITE3_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/sqlite3/sqlite3.c");
const SQLITE3_OVERLAY: &str = include_str!("../../../cabin-port/ports/sqlite3/3.53.2/cabin.toml");
const SQLITE3_MAIN_C: &str =
    include_str!("../../../cabin-port/tests/fixtures/fake-port-smoke/sqlite3/main.c");

struct FakeConsumer<'a> {
    name: &'a str,
    dep_name: &'a str,
    dep_version: &'a str,
    source_name: &'a str,
    source: &'a str,
    features: &'a [&'a str],
}

impl FakeConsumer<'_> {
    fn write(&self, root: &Path) -> PathBuf {
        let consumer_dir = root.join(format!("consumer-{}", self.name));
        fs::create_dir_all(consumer_dir.join("src")).expect("consumer src dir");
        fs::write(consumer_dir.join("src").join(self.source_name), self.source)
            .expect("write consumer source");
        fs::write(consumer_dir.join("cabin.toml"), self.manifest()).expect("write consumer");
        consumer_dir.join("cabin.toml")
    }

    fn manifest(&self) -> String {
        let feature_suffix = if self.features.is_empty() {
            String::new()
        } else {
            format!(
                ", features = [{}]",
                self.features
                    .iter()
                    .map(|feature| format!("\"{feature}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let standard = if Path::new(self.source_name)
            .extension()
            .is_some_and(|ext| ext == "cpp")
        {
            "cxx-standard = \"c++17\""
        } else {
            "c-standard = \"c11\""
        };
        format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n{}\n\n[dependencies]\n{} = {{ port-path = \"../ports/{}/{}\"{} }}\n\n[target.{}]\ntype = \"executable\"\nsources = [\"src/{}\"]\ndeps = [\"{}\"]\n",
            self.name,
            standard,
            self.dep_name,
            self.dep_name,
            self.dep_version,
            feature_suffix,
            self.name,
            self.source_name,
            self.dep_name
        )
    }
}

#[test]
fn fake_zlib_port_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let zlib = repo
        .port("zlib", "1.3.1")
        .archive_prefix("zlib-1.3.1")
        .file("zlib.h", FAKE_ZLIB_H)
        .stub_declared_sources_except(ZLIB_OVERLAY, "zlib", &["zutil.c"])
        .file("zutil.c", FAKE_ZLIB_C)
        .overlay_manifest(ZLIB_OVERLAY)
        .build();
    let server = FakeArchiveServer::new().serve(&zlib.archive).start();
    let manifest = FakeConsumer {
        name: "fake-zlib-consumer",
        dep_name: "zlib",
        dep_version: "1.3.1",
        source_name: "main.c",
        source: ZLIB_MAIN_C,
        features: &[],
    }
    .write(tmp.path());
    run_port_build_then_run(&PortBuildRun {
        label: "fake zlib",
        manifest,
        build_dir: tmp.path().join("build"),
        cache_dir: tmp.path().join("cache"),
        expected_stdout: &["fake zlib: fake-zlib/1.3.1"],
    });
    assert_eq!(server.requests_for(zlib.archive.name()), 1);
}

#[test]
fn fake_cjson_port_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let cjson = repo
        .port("cJSON", "1.7.18")
        .archive_prefix("cJSON-1.7.18")
        .file("cJSON.h", FAKE_CJSON_H)
        .file("cJSON.c", FAKE_CJSON_C)
        .overlay_manifest(CJSON_OVERLAY)
        .build();
    let server = FakeArchiveServer::new().serve(&cjson.archive).start();
    let manifest = FakeConsumer {
        name: "fake-cjson-consumer",
        dep_name: "cJSON",
        dep_version: "1.7.18",
        source_name: "main.c",
        source: CJSON_MAIN_C,
        features: &[],
    }
    .write(tmp.path());
    run_port_build_then_run(&PortBuildRun {
        label: "fake cJSON",
        manifest,
        build_dir: tmp.path().join("build"),
        cache_dir: tmp.path().join("cache"),
        expected_stdout: &["fake cJSON: 1.7.18"],
    });
    assert_eq!(server.requests_for(cjson.archive.name()), 1);
}

#[test]
fn fake_xxhash_port_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let xxhash = repo
        .port("xxhash", "0.8.3")
        .archive_prefix("xxHash-0.8.3")
        .file("xxhash.h", FAKE_XXHASH_H)
        .file("xxhash.c", FAKE_XXHASH_C)
        .overlay_manifest(XXHASH_OVERLAY)
        .build();
    let server = FakeArchiveServer::new().serve(&xxhash.archive).start();
    let manifest = FakeConsumer {
        name: "fake-xxhash-consumer",
        dep_name: "xxhash",
        dep_version: "0.8.3",
        source_name: "main.c",
        source: XXHASH_MAIN_C,
        features: &[],
    }
    .write(tmp.path());
    run_port_build_then_run(&PortBuildRun {
        label: "fake xxhash",
        manifest,
        build_dir: tmp.path().join("build"),
        cache_dir: tmp.path().join("cache"),
        expected_stdout: &["fake xxhash: 803"],
    });
    assert_eq!(server.requests_for(xxhash.archive.name()), 1);
}

#[test]
fn fake_tinyxml2_port_builds_and_runs() {
    require_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let tinyxml2 = repo
        .port("tinyxml2", "11.0.0")
        .archive_prefix("tinyxml2-11.0.0")
        .file("tinyxml2.h", FAKE_TINYXML2_H)
        .file("tinyxml2.cpp", FAKE_TINYXML2_CPP)
        .overlay_manifest(TINYXML2_OVERLAY)
        .build();
    let server = FakeArchiveServer::new().serve(&tinyxml2.archive).start();
    let manifest = FakeConsumer {
        name: "fake-tinyxml2-consumer",
        dep_name: "tinyxml2",
        dep_version: "11.0.0",
        source_name: "main.cpp",
        source: TINYXML2_MAIN_CPP,
        features: &[],
    }
    .write(tmp.path());
    run_port_build_then_run(&PortBuildRun {
        label: "fake tinyxml2",
        manifest,
        build_dir: tmp.path().join("build"),
        cache_dir: tmp.path().join("cache"),
        expected_stdout: &["fake tinyxml2: 11.0.0"],
    });
    assert_eq!(server.requests_for(tinyxml2.archive.name()), 1);
}

#[test]
fn fake_sqlite3_port_builds_single_threaded_feature_and_runs() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let sqlite3 = repo
        .port("sqlite3", "3.53.2")
        .archive_prefix("sqlite-autoconf-3530200")
        .file("sqlite3.h", FAKE_SQLITE3_H)
        .file("sqlite3.c", FAKE_SQLITE3_C)
        .overlay_manifest(SQLITE3_OVERLAY)
        .build();
    let server = FakeArchiveServer::new().serve(&sqlite3.archive).start();
    let manifest = FakeConsumer {
        name: "fake-sqlite3-consumer",
        dep_name: "sqlite3",
        dep_version: "3.53.2",
        source_name: "main.c",
        source: SQLITE3_MAIN_C,
        features: &["single-threaded"],
    }
    .write(tmp.path());
    run_port_build_then_run(&PortBuildRun {
        label: "fake sqlite3",
        manifest,
        build_dir: tmp.path().join("build"),
        cache_dir: tmp.path().join("cache"),
        expected_stdout: &["fake sqlite3: 3.53.2", "fake sqlite3 threadsafe: 0"],
    });
    assert_eq!(server.requests_for(sqlite3.archive.name()), 1);
}
