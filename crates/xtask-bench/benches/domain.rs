//! Criterion micro-benchmarks of the domain crates' hot paths, on the
//! same generated inputs as the `cargo perf` CLI scenarios.
//!
//! `cargo bench -p xtask-bench` measures; `cargo nextest run
//! --all-targets` runs each benchmark once as a test.

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cabin_core::PackageName;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use xtask_bench::fixture::{self, Lang, ProjectShape};

/// Fixtures live in Cargo's per-target scratch directory, shared by
/// every benchmark process and kept until the benchmarks are rebuilt.
fn fixtures() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        fixture::cache_root(&Path::new(env!("CARGO_TARGET_TMPDIR")).join("xtask-bench")).unwrap()
    })
}

fn registry() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        fixture::cached(&fixtures().join("registry"), |dir| {
            fixture::write_registry(&dir.join("registry"), &dir.join("staging"))
        })
        .unwrap()
        .join("registry")
    })
}

fn workspace() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        fixture::cached(&fixtures().join("workspace"), |dir| {
            fixture::write_workspace(dir, 40)
        })
        .unwrap()
    })
}

fn index() -> &'static cabin_index::PackageIndex {
    static INDEX: OnceLock<cabin_index::PackageIndex> = OnceLock::new();
    // The packages the resolver benchmarks' roots reach.
    INDEX.get_or_init(|| {
        let roots: Vec<_> = (0..fixture::WIDE.packages)
            .map(|i| name(&fixture::WIDE.name(i)))
            .chain([name(&fixture::DEEP.name(0))])
            .collect();
        cabin_index::FileIndex::open(registry())
            .unwrap()
            .load_package_index(&roots, &BTreeSet::new())
            .unwrap()
    })
}

fn name(s: &str) -> PackageName {
    PackageName::new(s).unwrap()
}

fn manifest(c: &mut Criterion) {
    let mut group = c.benchmark_group("manifest");
    for lang in [Lang::C, Lang::Cxx] {
        group.bench_function(format!("parse-{}", lang.suffix()), |b| {
            let dir = fixture::cached(
                &fixtures().join(format!("project-{}", lang.suffix())),
                |dir| {
                    fixture::write_project(
                        dir,
                        "bench",
                        ProjectShape {
                            lang,
                            sources: 24,
                            tests: 12,
                        },
                    )
                },
            )
            .unwrap();
            let text = std::fs::read_to_string(dir.join("cabin.toml")).unwrap();
            b.iter(|| cabin_manifest::parse_manifest_str(black_box(&text)).unwrap());
        });
    }
    group.finish();
}

fn workspace_and_features(c: &mut Criterion) {
    c.bench_function("workspace/load-40", |b| {
        let manifest = workspace().join("cabin.toml");
        b.iter(|| cabin_workspace::load_workspace(black_box(&manifest)).unwrap());
    });
    c.bench_function("features/resolve-40", |b| {
        let graph = cabin_workspace::load_workspace(workspace().join("cabin.toml")).unwrap();
        let roots: Vec<usize> = (0..graph.packages.len()).collect();
        let request = cabin_feature::RootFeatureRequest {
            include_defaults: true,
            all_features: false,
            explicit_features: BTreeSet::new(),
        };
        b.iter(|| {
            cabin_feature::resolve_features(&graph, &roots, &request, None, &BTreeSet::new())
                .unwrap()
        });
    });
}

fn index_and_resolver(c: &mut Criterion) {
    let wide: BTreeMap<_, _> = (0..fixture::WIDE.packages)
        .map(|i| {
            (
                name(&fixture::WIDE.name(i)),
                semver::VersionReq::parse("^1").unwrap(),
            )
        })
        .collect();
    // What resolution reads: the documents the wide roots reach.
    c.bench_function("index/load-file-registry", |b| {
        let index = cabin_index::FileIndex::open(registry()).unwrap();
        let roots: Vec<_> = wide.keys().cloned().collect();
        b.iter(|| {
            index
                .load_package_index(black_box(&roots), &BTreeSet::new())
                .unwrap()
        });
    });
    let mut group = c.benchmark_group("resolver");
    let deep = BTreeMap::from([(
        name(&fixture::DEEP.name(0)),
        semver::VersionReq::parse("^1").unwrap(),
    )]);
    for (label, roots) in [("wide", wide), ("deep", deep)] {
        group.bench_function(label, |b| {
            let input = cabin_resolver::ResolveInput::new(
                name("consumer"),
                semver::Version::new(0, 1, 0),
                roots.clone(),
            );
            b.iter(|| cabin_resolver::resolve(black_box(&input), index()).unwrap());
        });
    }
    group.finish();
}

fn artifact(c: &mut Criterion) {
    c.bench_function("artifact/extract-zip", |b| {
        let archive = std::fs::read_dir(registry().join("artifacts/bench/b000"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "zip"))
            .unwrap();
        b.iter_batched(
            || tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap(),
            |dest| {
                cabin_artifact::safe_extract_zip(
                    &archive,
                    dest.path(),
                    cabin_artifact::SafeExtractOptions::default(),
                )
                .unwrap();
                dest
            },
            BatchSize::PerIteration,
        );
    });
}

criterion_group!(
    benches,
    manifest,
    workspace_and_features,
    index_and_resolver,
    artifact
);
criterion_main!(benches);
