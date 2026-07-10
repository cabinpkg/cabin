//! The benign counterpart to the hostile corpus: every archive
//! `cabin package` can legitimately produce must extract, with wide
//! margin under every extraction cap.
//!
//! This runs on every platform CI builds (the workspace test job is
//! a matrix over Linux, macOS, and Windows), which is what covers the
//! platform-specific half of extraction: Windows resolves `..`,
//! drive prefixes, and `\` differently from POSIX, so the path rules
//! must hold on both.

use std::fs;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use cabin_artifact::{SafeExtractOptions, safe_extract_tar_gz};
use cabin_package::archive::{PackageFile, build_tar_gz, collect_package_files};

/// The production caps `cabin-artifact` enforces, restated here so
/// the margin assertions read against the real contract.  A change to
/// either side has to be a deliberate edit to both.
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_PATH_BYTES: usize = 256;
const RATIO_FLOOR_BYTES: u64 = 64 * 1024 * 1024;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

/// Every directory under `examples/` that holds a `cabin.toml`,
/// including workspace members (a workspace root is packageable as a
/// tree even when `cabin package` would want `--manifest-path`; the
/// archive writer and the extractor are what this test exercises).
fn example_packages() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(examples_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.join("cabin.toml").is_file() {
            out.push(path);
        }
    }
    out.sort();
    assert!(out.len() > 10, "expected the examples tree, found {out:?}");
    out
}

/// Metrics of one archive, measured against the caps.
struct Metrics {
    entries: usize,
    longest_path: usize,
    largest_entry: u64,
    decompressed: u64,
    compressed: u64,
}

fn measure(files: &[PackageFile], bytes: &[u8]) -> Metrics {
    let decompressed = files
        .iter()
        .map(|f| fs::metadata(&f.abs_path).unwrap().len())
        .sum();
    Metrics {
        entries: files.len(),
        longest_path: files.iter().map(|f| f.rel_path.len()).max().unwrap_or(0),
        largest_entry: files
            .iter()
            .map(|f| fs::metadata(&f.abs_path).unwrap().len())
            .max()
            .unwrap_or(0),
        decompressed,
        compressed: bytes.len() as u64,
    }
}

#[test]
fn every_example_round_trips_with_wide_margin_under_the_caps() {
    for package in example_packages() {
        let name = package.file_name().unwrap().to_string_lossy().into_owned();
        let files = collect_package_files(&package, None).unwrap();
        let bytes = build_tar_gz(&files, None).unwrap();
        let metrics = measure(&files, &bytes);

        // Each cap, checked directly against the archive's own
        // metric rather than by re-running extraction with scaled
        // limits: the ratio floor would otherwise mask the ratio.
        assert!(
            metrics.entries * 10 <= MAX_ENTRIES,
            "{name}: {} entries leaves under 10x margin below {MAX_ENTRIES}",
            metrics.entries
        );
        assert!(
            metrics.longest_path * 2 <= MAX_PATH_BYTES,
            "{name}: longest path {} leaves under 2x margin below {MAX_PATH_BYTES}",
            metrics.longest_path
        );
        assert!(
            metrics.largest_entry * 100 <= MAX_ENTRY_BYTES,
            "{name}: largest entry {} leaves under 100x margin below {MAX_ENTRY_BYTES}",
            metrics.largest_entry
        );
        // The whole-stream cap is `max(32 x compressed, 64 MiB)`, so
        // the floor is what a small archive is measured against.  A
        // real package sits orders of magnitude below it.
        assert!(
            metrics.decompressed * 100 <= RATIO_FLOOR_BYTES,
            "{name}: {} decompressed bytes leaves under 100x margin below the {RATIO_FLOOR_BYTES}-byte floor",
            metrics.decompressed
        );
        assert!(metrics.compressed > 0, "{name}: empty archive");

        // The proof that matters: the archive the packager wrote
        // extracts under the production caps.
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("pkg.tar.gz");
        fs::write(&archive, &bytes).unwrap();
        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        safe_extract_tar_gz(&archive, &dest, SafeExtractOptions::default())
            .unwrap_or_else(|err| panic!("{name}: packaged archive failed to extract: {err}"));

        assert!(
            dest.join("cabin.toml").is_file(),
            "{name}: extracted tree has no root cabin.toml"
        );
        for file in &files {
            assert!(
                dest.join(&file.rel_path).is_file(),
                "{name}: {} missing from the extracted tree",
                file.rel_path
            );
        }
    }
}
