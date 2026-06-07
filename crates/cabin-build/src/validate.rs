//! Validate that a detected toolchain can run the commands the
//! C++ backend emits.
//!
//! The planner currently emits GCC/Clang-style commands:
//!
//! - C++ compile: `cxx -std=c++17 -O… [-g] [-DNDEBUG] -MMD -MF
//!   <depfile> -D<name> -I<dir> [extra-args] -c <src> -o <obj>`.
//! - Static-library archive: `ar crs <lib> <objs>`.
//! - Link: `cxx <objs> <libs> [extra-args] -o <exe>`.
//!
//! Any compiler / archiver that cannot run those exact shapes is
//! rejected up front rather than left to fail with a confusing
//! Ninja error.

use std::collections::BTreeSet;

use cabin_core::{
    ArchiverKind, ResolvedToolchain, SourceLanguage, ToolchainDetectionReport, classify_source,
    validate_ar_for_backend, validate_cc_for_backend, validate_cxx_for_backend,
};
use cabin_workspace::PackageGraph;

use crate::error::BuildError;

/// Validate that every populated tool in `report` can execute the
/// command shapes emitted by the current backend. Returns the
/// first problem encountered so users see one actionable error,
/// not a wall of unrelated failures.
///
/// Each tool is held to its dialect's contract. The MSVC dialect
/// drives `cl.exe` / `lib.exe`; the GCC/Clang dialect drives a
/// GCC-style compiler (full C++ contract: GCC-style flags,
/// depfile, `-std=c++17`) plus an `ar`-compatible archiver. The C
/// compiler's contract is laxer than the C++ one because a pure-C
/// driver may not accept C++ mode at all.
///
/// Beyond the per-tool checks, the tools must all belong to the
/// *same* dialect: Cabin emits one command-line dialect per build,
/// so an MSVC compiler paired with a GNU `ar` (or the reverse)
/// is rejected rather than left to fail mid-build.
///
/// `toolchain` is the matching [`ResolvedToolchain`] — we use it
/// to recover the user-visible spec strings (`clang++`,
/// `/opt/llvm/bin/clang++`) for the error messages.
///
/// # Errors
/// Returns [`BuildError::UnsupportedToolchain`] (wrapping the
/// [`cabin_core::ToolDetectionError`] from the first failing
/// `validate_*_for_backend` check) when a tool cannot run its
/// dialect's command shapes, and [`BuildError::MixedToolchainDialects`]
/// when the resolved tools span both dialects.
pub fn validate_toolchain_for_backend(
    toolchain: &ResolvedToolchain,
    report: &ToolchainDetectionReport,
    has_c_sources: bool,
) -> Result<(), BuildError> {
    let cxx_spec = toolchain.cxx.spec.display();
    validate_cxx_for_backend(&cxx_spec, &report.cxx.identity, &report.cxx.capabilities)?;
    // The C compiler is only validated when a C source actually exists.
    // The resolver fills the optional `cc` slot from the host default
    // fallback (`cl` on Windows) even for a C++-only build, so checking
    // it unconditionally would reject e.g. `CXX=clang++` against a
    // never-used default `cc=cl` — the planner would never invoke that
    // `cc`. Mirror the planner's "cc is needed only for `.c` sources"
    // contract.
    if has_c_sources
        && let (Some(cc_tool), Some(cc_detection)) = (toolchain.cc.as_ref(), report.cc.as_ref())
    {
        let cc_spec = cc_tool.spec.display();
        validate_cc_for_backend(&cc_spec, &cc_detection.identity, &cc_detection.capabilities)?;
    }
    let ar_spec = toolchain.ar.spec.display();
    validate_ar_for_backend(&ar_spec, &report.ar.identity, &report.ar.capabilities)?;

    // Every tool individually runs; now require them to share a
    // dialect. The C++ compiler picks it (MSVC `cl` vs GCC/Clang),
    // and the archiver and optional C compiler must match.
    let cxx_is_msvc = report.cxx.identity.kind.speaks_msvc_dialect();
    let ar_is_msvc = report.ar.identity.kind == ArchiverKind::Lib;
    if cxx_is_msvc != ar_is_msvc {
        return Err(BuildError::MixedToolchainDialects {
            detail: format!(
                "C++ compiler `{cxx_spec}` is {}, but archiver `{ar_spec}` is {}",
                dialect_label(cxx_is_msvc),
                dialect_label(ar_is_msvc),
            ),
        });
    }
    if has_c_sources && let Some(cc_detection) = report.cc.as_ref() {
        let cc_is_msvc = cc_detection.identity.kind.speaks_msvc_dialect();
        if cc_is_msvc != cxx_is_msvc {
            let cc_spec = toolchain
                .cc
                .as_ref()
                .map_or_else(|| "cc".to_owned(), |t| t.spec.display());
            return Err(BuildError::MixedToolchainDialects {
                detail: format!(
                    "C++ compiler `{cxx_spec}` is {}, but C compiler `{cc_spec}` is {}",
                    dialect_label(cxx_is_msvc),
                    dialect_label(cc_is_msvc),
                ),
            });
        }
    }
    Ok(())
}

/// Human-readable dialect name for the mixed-toolchain diagnostic.
fn dialect_label(is_msvc: bool) -> &'static str {
    if is_msvc { "MSVC" } else { "GCC/Clang-style" }
}

/// Whether any *selected* package carries a C (`.c`) source, i.e. the
/// build will actually invoke the C compiler. Used to decide whether
/// [`validate_toolchain_for_backend`] holds the optional `cc` slot to
/// the backend contract — a C++-only build never compiles C, so a
/// defaulted `cc` it will not use must not gate the build.
///
/// `selected` is the index closure of the packages this command builds
/// (selected members plus their local path-dependency closure). An
/// unselected workspace member's `.c` file must not gate
/// `cabin build -p <cpp-only>`, so the scan is restricted to `selected`
/// rather than the whole graph.
#[must_use]
pub fn graph_has_c_sources(graph: &PackageGraph, selected: &BTreeSet<usize>) -> bool {
    selected
        .iter()
        .filter_map(|&idx| graph.packages.get(idx))
        .flat_map(|pkg| &pkg.package.targets)
        .flat_map(|target| &target.sources)
        .any(|source| classify_source(source) == Some(SourceLanguage::C))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{
        ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind, CompilerVersion,
        ResolvedTool, ResolvedToolchain, ToolDetection, ToolKind, ToolSource, ToolSpec,
        derive_ar_capabilities, derive_cxx_capabilities,
    };
    use camino::Utf8PathBuf;

    fn make_toolchain(cxx_spec: &str, ar_spec: &str) -> ResolvedToolchain {
        ResolvedToolchain {
            cxx: ResolvedTool {
                kind: ToolKind::CxxCompiler,
                path: Utf8PathBuf::from("/bin").join(cxx_spec),
                spec: ToolSpec::Name(cxx_spec.into()),
                source: ToolSource::Default,
            },
            ar: ResolvedTool {
                kind: ToolKind::Archiver,
                path: Utf8PathBuf::from("/bin").join(ar_spec),
                spec: ToolSpec::Name(ar_spec.into()),
                source: ToolSource::Default,
            },
            cc: None,
        }
    }

    fn report_for(cxx: CompilerIdentity, ar: ArchiverIdentity) -> ToolchainDetectionReport {
        let cxx_caps = derive_cxx_capabilities(&cxx);
        let ar_caps = derive_ar_capabilities(&ar);
        ToolchainDetectionReport {
            cxx: ToolDetection {
                path: Utf8PathBuf::from("/bin/cxx"),
                identity: cxx,
                capabilities: cxx_caps,
            },
            cc: None,
            ar: ToolDetection {
                path: Utf8PathBuf::from("/bin/ar"),
                identity: ar,
                capabilities: ar_caps,
            },
        }
    }

    #[test]
    fn accepts_clang_with_gnu_ar() {
        let toolchain = make_toolchain("clang++", "ar");
        let report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Clang,
                version: CompilerVersion::parse("17.0.6"),
                target: None,
                raw_version_line: "clang version 17.0.6".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Ar,
                version: CompilerVersion::parse("2.40"),
                raw_version_line: "GNU ar".into(),
            },
        );
        validate_toolchain_for_backend(&toolchain, &report, true).unwrap();
    }

    #[test]
    fn accepts_full_msvc_toolchain() {
        // `cl` + `lib` is a coherent MSVC toolchain and is now a
        // first-class supported backend.
        let toolchain = make_toolchain("cl.exe", "lib.exe");
        let report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Msvc,
                version: None,
                target: None,
                raw_version_line: "Microsoft Optimizing Compiler".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Lib,
                version: None,
                raw_version_line: "Microsoft Library Manager".into(),
            },
        );
        validate_toolchain_for_backend(&toolchain, &report, true).unwrap();
    }

    #[test]
    fn accepts_clang_cl_with_lib_and_rejects_it_with_gnu_ar() {
        // `clang-cl` speaks the MSVC dialect, so it pairs with
        // `lib.exe` (coherent) but not with GNU `ar` (mixed).
        let clang_cl = CompilerIdentity {
            kind: CompilerKind::ClangCl,
            version: CompilerVersion::parse("17.0.6"),
            target: None,
            raw_version_line: "clang version 17.0.6".into(),
        };
        let lib = ArchiverIdentity {
            kind: ArchiverKind::Lib,
            version: None,
            raw_version_line: "Microsoft Library Manager".into(),
        };
        let report = report_for(clang_cl.clone(), lib);
        validate_toolchain_for_backend(&make_toolchain("clang-cl", "lib.exe"), &report, true)
            .unwrap();

        let gnu_ar = ArchiverIdentity {
            kind: ArchiverKind::Ar,
            version: CompilerVersion::parse("2.40"),
            raw_version_line: "GNU ar".into(),
        };
        let mixed = report_for(clang_cl, gnu_ar);
        let err = validate_toolchain_for_backend(&make_toolchain("clang-cl", "ar"), &mixed, true)
            .unwrap_err();
        assert!(
            matches!(err, BuildError::MixedToolchainDialects { .. }),
            "clang-cl + GNU ar should be rejected as mixed-dialect, got: {err}"
        );
    }

    #[test]
    fn rejects_mixed_dialect_toolchain() {
        // A GCC/Clang compiler with an MSVC archiver runs each tool
        // individually but cannot be driven as one dialect.
        let toolchain = make_toolchain("clang++", "lib.exe");
        let report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Clang,
                version: CompilerVersion::parse("17.0.6"),
                target: None,
                raw_version_line: "clang version 17.0.6".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Lib,
                version: None,
                raw_version_line: "Microsoft Library Manager".into(),
            },
        );
        let err = validate_toolchain_for_backend(&toolchain, &report, true).unwrap_err();
        assert!(
            matches!(err, BuildError::MixedToolchainDialects { .. }),
            "expected mixed-dialect rejection, got: {err}"
        );
    }

    #[test]
    fn defers_cc_dialect_check_until_c_sources_exist() {
        // A C++-only build that selects GNU `clang++` but whose
        // optional `cc` slot defaulted to MSVC `cl` must not be
        // rejected: with no `.c` source the planner never invokes that
        // `cc`. Once a C source exists, the mixed C/C++ dialect is a
        // real problem and the check fires.
        let mut toolchain = make_toolchain("clang++", "ar");
        toolchain.cc = Some(ResolvedTool {
            kind: ToolKind::CCompiler,
            path: Utf8PathBuf::from("/bin/cl.exe"),
            spec: ToolSpec::Name("cl".into()),
            source: ToolSource::Default,
        });
        let cc_identity = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse("19.39.0"),
            target: None,
            raw_version_line: "Microsoft Optimizing Compiler".into(),
        };
        let mut report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Clang,
                version: CompilerVersion::parse("17.0.6"),
                target: None,
                raw_version_line: "clang version 17.0.6".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Ar,
                version: CompilerVersion::parse("2.40"),
                raw_version_line: "GNU ar".into(),
            },
        );
        report.cc = Some(ToolDetection {
            path: Utf8PathBuf::from("/bin/cl.exe"),
            capabilities: derive_cxx_capabilities(&cc_identity),
            identity: cc_identity,
        });

        // No C sources: the defaulted MSVC `cc` is never used, so the
        // C++-only GNU toolchain validates.
        validate_toolchain_for_backend(&toolchain, &report, false).unwrap();
        // A C source exists: the mixed C/C++ dialect is rejected.
        let err = validate_toolchain_for_backend(&toolchain, &report, true).unwrap_err();
        assert!(
            matches!(err, BuildError::MixedToolchainDialects { .. }),
            "expected mixed-dialect rejection once C sources exist, got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_compiler_clearly() {
        let toolchain = make_toolchain("custom-cxx", "ar");
        let report = report_for(
            CompilerIdentity::unknown("???"),
            ArchiverIdentity {
                kind: ArchiverKind::Ar,
                version: None,
                raw_version_line: "GNU ar".into(),
            },
        );
        let err = validate_toolchain_for_backend(&toolchain, &report, true).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("could not be identified"),
            "expected unknown-compiler error, got: {message}"
        );
    }

    #[test]
    fn c_source_detection_is_scoped_to_the_selected_closure() {
        use cabin_core::{Package, PackageName, Target, TargetKind, TargetName};
        use cabin_workspace::{PackageKind, WorkspacePackage};

        fn pkg(name: &str, source: &str) -> WorkspacePackage {
            let target = Target {
                name: TargetName::new("lib").unwrap(),
                kind: TargetKind::Library,
                sources: vec![Utf8PathBuf::from(source)],
                include_dirs: Vec::new(),
                defines: Vec::new(),
                deps: Vec::new(),
            };
            let package = Package::new(
                PackageName::new(name).unwrap(),
                semver::Version::parse("0.1.0").unwrap(),
                vec![target],
                Vec::new(),
            )
            .unwrap();
            let dir = std::path::PathBuf::from("/tmp").join(name);
            WorkspacePackage {
                package,
                manifest_path: dir.join("cabin.toml"),
                manifest_dir: dir,
                deps: Vec::new(),
                kind: PackageKind::Local,
                is_port: false,
            }
        }

        // Package 0 carries a C source; package 1 is C++-only.
        let graph = PackageGraph {
            root_manifest_path: std::path::PathBuf::from("/tmp/cabin.toml"),
            root_dir: std::path::PathBuf::from("/tmp"),
            is_workspace_root: true,
            root_package: None,
            root_settings: Default::default(),
            primary_packages: vec![0, 1],
            default_members: vec![0, 1],
            excluded_members: Vec::new(),
            packages: vec![pkg("with_c", "a.c"), pkg("cpp_only", "b.cc")],
        };

        // Selecting only the C++-only package must not observe the
        // sibling's `.c` source — the over-broad case Codex reported.
        assert!(!graph_has_c_sources(&graph, &BTreeSet::from([1usize])));
        // Selecting the C package, or the whole workspace, does.
        assert!(graph_has_c_sources(&graph, &BTreeSet::from([0usize])));
        assert!(graph_has_c_sources(
            &graph,
            &BTreeSet::from([0usize, 1usize])
        ));
    }
}
