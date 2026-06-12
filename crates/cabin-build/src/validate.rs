//! Validate that a detected toolchain can run the commands the
//! C++ backend emits.
//!
//! The planner currently emits GCC/Clang-style commands:
//!
//! - C++ compile: `cxx -std=<effective standard> -O… [-g] [-DNDEBUG]
//!   -MD -MF <depfile> -D<name> -I<dir> [-isystem <dir>]
//!   [extra-args] -c <src> -o <obj>`.
//! - Static-library archive: `ar crs <lib> <objs>`.
//! - Link: `cxx <objs> <libs> [extra-args] -o <exe>`.
//!
//! Any compiler / archiver that cannot run those exact shapes — or
//! that lacks the flag for a *requested* language standard — is
//! rejected up front rather than left to fail with a confusing
//! Ninja error.

use std::collections::{BTreeSet, HashMap};
use std::hash::BuildHasher;

use cabin_core::{
    ArchiverKind, CStandard, CxxStandard, ResolvedLanguageStandards, ResolvedToolchain,
    SourceLanguage, ToolchainDetectionReport, classify_source, effective_c, effective_cxx,
    validate_ar_for_backend, validate_c_standards, validate_cc_for_backend,
    validate_cxx_for_backend, validate_cxx_standards,
};
use cabin_workspace::PackageGraph;

use crate::error::BuildError;

/// The set of language standards a build's compiles request, per
/// language. A language's set is empty iff no compile of that
/// language exists, which is the signal the C-compiler checks key
/// on.
///
/// The authoritative form is [`requested_standards_of`], derived
/// from a planned [`crate::BuildGraph`] so it covers exactly the compiles
/// the build will run — no more (an unbuilt sibling target must not
/// gate validation) and no less. [`collect_requested_standards`]
/// is the pre-plan package-level approximation used only where a
/// value is needed *before* planning (the MSVC `/external:I`
/// fallback decision).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestedStandards {
    pub c: BTreeSet<CStandard>,
    pub cxx: BTreeSet<CxxStandard>,
}

impl RequestedStandards {
    /// Whether any selected target compiles a `.c` source — the
    /// signal that scopes the C-compiler checks.
    #[must_use]
    pub fn has_c_sources(&self) -> bool {
        !self.c.is_empty()
    }
}

/// The exact standards a planned build requests: every compile
/// action in the graph contributes its typed standard. This is the
/// set [`validate_toolchain_for_backend`] should be called with —
/// the planner has already narrowed targets to the requested
/// selection, so a sibling target that is not built cannot gate the
/// toolchain.
#[must_use]
pub fn requested_standards_of(graph: &crate::BuildGraph) -> RequestedStandards {
    use cabin_core::LanguageStandard;
    use cabin_driver::BuildAction;
    let mut out = RequestedStandards::default();
    for action in &graph.actions {
        if let BuildAction::Compile(compile) = action {
            match compile.standard {
                LanguageStandard::C(standard) => {
                    out.c.insert(standard);
                }
                LanguageStandard::Cxx(standard) => {
                    out.cxx.insert(standard);
                }
            }
        }
    }
    out
}

/// Pre-plan approximation of [`requested_standards_of`]: walk every
/// selected package's targets and classify each declared source.
/// Over-approximates per target (it cannot know which targets the
/// plan will reach), so it must not gate validation — use it only
/// for decisions needed before planning, like the MSVC
/// `/external:I` fallback. Dev-only targets (`test` / `example`)
/// contribute only for packages in `dev_for`, mirroring
/// dev-dependency activation.
#[must_use]
pub fn collect_requested_standards<S: BuildHasher>(
    graph: &PackageGraph,
    selected: &BTreeSet<usize>,
    standards: &HashMap<usize, ResolvedLanguageStandards, S>,
    dev_for: &BTreeSet<String>,
) -> RequestedStandards {
    let mut out = RequestedStandards::default();
    for &idx in selected {
        let Some(pkg) = graph.packages.get(idx) else {
            continue;
        };
        let resolved = standards.get(&idx).copied().unwrap_or_default();
        let include_dev = dev_for.contains(pkg.package.name.as_str());
        for target in &pkg.package.targets {
            if target.kind.is_dev_only() && !include_dev {
                continue;
            }
            for source in &target.sources {
                match classify_source(source) {
                    Some(SourceLanguage::C) => {
                        out.c.insert(effective_c(&resolved, target).standard);
                    }
                    Some(SourceLanguage::Cxx) => {
                        out.cxx.insert(effective_cxx(&resolved, target).standard);
                    }
                    None => {}
                }
            }
        }
    }
    out
}

/// Validate that every populated tool in `report` can execute the
/// command shapes emitted by the current backend. Returns the
/// first problem encountered so users see one actionable error,
/// not a wall of unrelated failures.
///
/// Each tool is held to its dialect's contract. The MSVC dialect
/// drives `cl.exe` / `lib.exe`; the GCC/Clang dialect drives a
/// GCC-style compiler (GCC-style flags plus depfile) and an
/// `ar`-compatible archiver. The optional C compiler and every
/// language-standard check are validated separately by
/// [`validate_toolchain_standards`], post-plan, gated on the
/// compiles the plan actually emits — a `cc` slot (or a sibling
/// target's C source) that no planned compile uses must not gate
/// the build.
///
/// Beyond the per-tool checks, the C++ compiler and archiver must
/// belong to the *same* dialect: Cabin emits one command-line
/// dialect per build, so an MSVC compiler paired with a GNU `ar`
/// (or the reverse) is rejected rather than left to fail
/// mid-build.
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
) -> Result<(), BuildError> {
    let cxx_spec = toolchain.cxx.spec.display();
    validate_cxx_for_backend(&cxx_spec, &report.cxx.identity, &report.cxx.capabilities)?;
    let ar_spec = toolchain.ar.spec.display();
    validate_ar_for_backend(&ar_spec, &report.ar.identity, &report.ar.capabilities)?;

    // Both tools individually run; now require them to share a
    // dialect. The C++ compiler picks it (MSVC `cl` vs GCC/Clang)
    // and the archiver must match.
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
    Ok(())
}

/// Validate the plan-dependent half of the toolchain contract:
/// every language standard in `requested`, plus — when the plan
/// actually compiles C — the C compiler's backend shape and
/// dialect coherence. `requested` is derived from the *final*
/// planned graph via [`requested_standards_of`] (after the
/// `cabin check` rewrite when applicable), so only compiles the
/// command will run participate: an unbuilt sibling target, an
/// unrelated C executable in a dependency package, or a dependency
/// compile that `cabin check` drops can never gate the build.
/// Runs after `plan()` and before any Ninja file is written; the
/// plan-independent C++/archiver shape checks in
/// [`validate_toolchain_for_backend`] stay pre-plan so a broken
/// toolchain still fails fast.
///
/// # Errors
/// Returns [`BuildError::UnsupportedToolchain`] wrapping the first
/// failing check ([`cabin_core::ToolDetectionError::CxxLacksStandard`],
/// [`cabin_core::ToolDetectionError::CLacksStandard`], or a C
/// backend-shape error) and [`BuildError::MixedToolchainDialects`]
/// when a C compile exists but the C compiler's dialect differs
/// from the C++ compiler's.
pub fn validate_toolchain_standards(
    toolchain: &ResolvedToolchain,
    report: &ToolchainDetectionReport,
    requested: &RequestedStandards,
) -> Result<(), BuildError> {
    let cxx_spec = toolchain.cxx.spec.display();
    validate_cxx_standards(&cxx_spec, &report.cxx.identity, &requested.cxx)?;
    // The C compiler is only validated when a C compile was actually
    // planned. The resolver fills the optional `cc` slot from the
    // host default fallback (`cl` on Windows) even for a C++-only
    // build, so checking it unconditionally would reject e.g.
    // `CXX=clang++` against a never-used default `cc=cl`.
    if requested.has_c_sources()
        && let (Some(cc_tool), Some(cc_detection)) = (toolchain.cc.as_ref(), report.cc.as_ref())
    {
        let cc_spec = cc_tool.spec.display();
        validate_cc_for_backend(&cc_spec, &cc_detection.identity, &cc_detection.capabilities)?;
        let cxx_is_msvc = report.cxx.identity.kind.speaks_msvc_dialect();
        let cc_is_msvc = cc_detection.identity.kind.speaks_msvc_dialect();
        if cc_is_msvc != cxx_is_msvc {
            return Err(BuildError::MixedToolchainDialects {
                detail: format!(
                    "C++ compiler `{cxx_spec}` is {}, but C compiler `{cc_spec}` is {}",
                    dialect_label(cxx_is_msvc),
                    dialect_label(cc_is_msvc),
                ),
            });
        }
        validate_c_standards(&cc_spec, &cc_detection.identity, &requested.c)?;
    }
    Ok(())
}

/// Surface the first standards violation the planner recorded that
/// survived into the final graph (after the `cabin check` rewrite,
/// when applicable): an MSVC no-stable-flag compile or an
/// escape-hatch flag conflict on a planned compile. Must run before
/// anything is lowered or written; commands that skip the
/// toolchain-validation pass (`cabin tidy`'s fail-soft path) must
/// still call this so a violating compile cannot be silently
/// dropped from the compile database.
///
/// # Errors
/// Returns [`BuildError::StandardUnsupportedOnMsvcDialect`] or
/// [`BuildError::StandardFlagConflict`] for the first surviving
/// violation.
pub fn validate_planned_standards(graph: &crate::BuildGraph) -> Result<(), BuildError> {
    match graph.standard_violations.first() {
        Some(crate::StandardViolation::MsvcSpelling {
            target,
            language,
            standard,
            ..
        }) => Err(BuildError::StandardUnsupportedOnMsvcDialect {
            target: target.clone(),
            language,
            standard,
        }),
        Some(crate::StandardViolation::FlagConflict { conflict, .. }) => {
            Err(BuildError::StandardFlagConflict(Box::new(conflict.clone())))
        }
        None => Ok(()),
    }
}

/// Human-readable dialect name for the mixed-toolchain diagnostic.
fn dialect_label(is_msvc: bool) -> &'static str {
    if is_msvc { "MSVC" } else { "GCC/Clang-style" }
}

/// Whether every compiler that will run a compile in this build
/// accepts the MSVC-dialect distinct system-include spelling
/// (`/external:W0` + `/external:I <dir>`). Mirrors
/// [`validate_toolchain_for_backend`]'s contract for the optional
/// `cc` slot: the C compiler only weighs in when a C source exists,
/// because the planner never invokes it otherwise.
///
/// The planner consults the answer through
/// [`crate::PlanRequest::msvc_external_includes`] and falls back to
/// plain `/I` when it is `false`. On a GCC/Clang build the value is
/// ignored — `-isystem` is part of the base dialect.
#[must_use]
pub fn msvc_external_includes_supported(
    report: &ToolchainDetectionReport,
    has_c_sources: bool,
) -> bool {
    let cxx_ok = report.cxx.capabilities.external_include_dirs.supported;
    let cc_ok = !has_c_sources
        || report
            .cc
            .as_ref()
            .is_none_or(|cc| cc.capabilities.external_include_dirs.supported);
    cxx_ok && cc_ok
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

    fn requested(c: &[CStandard], cxx: &[CxxStandard]) -> RequestedStandards {
        RequestedStandards {
            c: c.iter().copied().collect(),
            cxx: cxx.iter().copied().collect(),
        }
    }

    /// The historic defaults: a C++17 build, optionally with C11.
    fn requested_defaults(with_c: bool) -> RequestedStandards {
        if with_c {
            requested(&[CStandard::C11], &[CxxStandard::Cxx17])
        } else {
            requested(&[], &[CxxStandard::Cxx17])
        }
    }

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
        validate_toolchain_for_backend(&toolchain, &report).unwrap();
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
        validate_toolchain_for_backend(&toolchain, &report).unwrap();
    }

    #[test]
    fn msvc_external_includes_follow_the_cl_version() {
        let lib = || ArchiverIdentity {
            kind: ArchiverKind::Lib,
            version: None,
            raw_version_line: "Microsoft Library Manager".into(),
        };
        let cl = |version: &str| CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse(version),
            target: None,
            raw_version_line: format!("Microsoft Optimizing Compiler {version}"),
        };
        let modern = report_for(cl("19.29.30133"), lib());
        assert!(msvc_external_includes_supported(&modern, false));
        let old = report_for(cl("19.20.27508"), lib());
        assert!(!msvc_external_includes_supported(&old, false));
    }

    #[test]
    fn msvc_external_includes_consider_cc_only_with_c_sources() {
        // A modern C++ `cl` paired with an old C `cl`: the C compiler
        // only matters when a C source will actually be compiled.
        let lib = ArchiverIdentity {
            kind: ArchiverKind::Lib,
            version: None,
            raw_version_line: "Microsoft Library Manager".into(),
        };
        let modern_cl = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse("19.39.33523"),
            target: None,
            raw_version_line: "Microsoft Optimizing Compiler 19.39".into(),
        };
        let old_cl = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse("19.20.27508"),
            target: None,
            raw_version_line: "Microsoft Optimizing Compiler 19.20".into(),
        };
        let mut report = report_for(modern_cl, lib);
        let old_caps = derive_cxx_capabilities(&old_cl);
        report.cc = Some(ToolDetection {
            path: Utf8PathBuf::from("/bin/cc"),
            identity: old_cl,
            capabilities: old_caps,
        });
        assert!(msvc_external_includes_supported(&report, false));
        assert!(!msvc_external_includes_supported(&report, true));
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
        validate_toolchain_for_backend(&make_toolchain("clang-cl", "lib.exe"), &report).unwrap();

        let gnu_ar = ArchiverIdentity {
            kind: ArchiverKind::Ar,
            version: CompilerVersion::parse("2.40"),
            raw_version_line: "GNU ar".into(),
        };
        let mixed = report_for(clang_cl, gnu_ar);
        let err =
            validate_toolchain_for_backend(&make_toolchain("clang-cl", "ar"), &mixed).unwrap_err();
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
        let err = validate_toolchain_for_backend(&toolchain, &report).unwrap_err();
        assert!(
            matches!(err, BuildError::MixedToolchainDialects { .. }),
            "expected mixed-dialect rejection, got: {err}"
        );
    }

    #[test]
    fn defers_cc_dialect_check_until_c_compiles_are_planned() {
        // A C++-only build that selects GNU `clang++` but whose
        // optional `cc` slot defaulted to MSVC `cl` must not be
        // rejected: with no planned C compile the planner never
        // invokes that `cc`. Once a C compile is planned, the mixed
        // C/C++ dialect is a real problem and the check fires.
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

        // The backend-shape pass never consults `cc`.
        validate_toolchain_for_backend(&toolchain, &report).unwrap();
        // No planned C compile: the defaulted MSVC `cc` is never
        // used, so the C++-only GNU toolchain validates.
        validate_toolchain_standards(&toolchain, &report, &requested_defaults(false)).unwrap();
        // A C compile is planned: the mixed C/C++ dialect is rejected.
        let err = validate_toolchain_standards(&toolchain, &report, &requested_defaults(true))
            .unwrap_err();
        assert!(
            matches!(err, BuildError::MixedToolchainDialects { .. }),
            "expected mixed-dialect rejection once C compiles are planned, got: {err}"
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
        let err = validate_toolchain_for_backend(&toolchain, &report).unwrap_err();
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
                language: Default::default(),
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
        let standards = HashMap::new();
        let no_dev = BTreeSet::new();
        let cpp_only =
            collect_requested_standards(&graph, &BTreeSet::from([1usize]), &standards, &no_dev);
        assert!(!cpp_only.has_c_sources());
        assert_eq!(
            cpp_only.cxx,
            BTreeSet::from([cabin_core::DEFAULT_CXX_STANDARD])
        );
        // Selecting the C package, or the whole workspace, does.
        let with_c =
            collect_requested_standards(&graph, &BTreeSet::from([0usize]), &standards, &no_dev);
        assert!(with_c.has_c_sources());
        assert_eq!(with_c.c, BTreeSet::from([cabin_core::DEFAULT_C_STANDARD]));
        let both = collect_requested_standards(
            &graph,
            &BTreeSet::from([0usize, 1usize]),
            &standards,
            &no_dev,
        );
        assert!(both.has_c_sources());
    }

    #[test]
    fn requested_standards_skip_dev_only_targets_unless_activated() {
        use cabin_core::{
            CxxStandard, LanguageStandardSettings, Package, PackageName, Target, TargetKind,
            TargetName,
        };
        use cabin_workspace::{PackageKind, WorkspacePackage};

        let test_target = Target {
            name: TargetName::new("t").unwrap(),
            kind: TargetKind::Test,
            sources: vec![Utf8PathBuf::from("t.cc")],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            language: LanguageStandardSettings {
                cxx_standard: Some(CxxStandard::Cxx20),
                ..Default::default()
            },
        };
        let package = Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![test_target],
            Vec::new(),
        )
        .unwrap();
        let dir = std::path::PathBuf::from("/tmp/demo");
        let graph = PackageGraph {
            root_manifest_path: dir.join("cabin.toml"),
            root_dir: dir.clone(),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![WorkspacePackage {
                package,
                manifest_path: dir.join("cabin.toml"),
                manifest_dir: dir,
                deps: Vec::new(),
                kind: PackageKind::Local,
                is_port: false,
            }],
        };
        let standards = HashMap::new();
        // `cabin build` (no dev activation): the test target's c++20
        // does not gate the build.
        let plain = collect_requested_standards(
            &graph,
            &BTreeSet::from([0usize]),
            &standards,
            &BTreeSet::new(),
        );
        assert!(plain.cxx.is_empty());
        // `cabin test` activates the package's dev targets.
        let dev = collect_requested_standards(
            &graph,
            &BTreeSet::from([0usize]),
            &standards,
            &BTreeSet::from(["demo".to_owned()]),
        );
        assert_eq!(dev.cxx, BTreeSet::from([CxxStandard::Cxx20]));
    }

    #[test]
    fn standards_validation_rejects_unsupported_requested_standard() {
        use cabin_core::CxxStandard;
        let toolchain = make_toolchain("cl.exe", "lib.exe");
        let report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Msvc,
                version: CompilerVersion::parse("19.39.33523"),
                target: None,
                raw_version_line: "Microsoft Optimizing Compiler 19.39".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Lib,
                version: None,
                raw_version_line: "Microsoft Library Manager".into(),
            },
        );
        // The backend shape is fine, and the default request passes...
        validate_toolchain_for_backend(&toolchain, &report).unwrap();
        validate_toolchain_standards(&toolchain, &report, &requested_defaults(false)).unwrap();
        // ...but a planned c++23 compile is rejected: cl has no
        // stable flag for it.
        let err = validate_toolchain_standards(
            &toolchain,
            &report,
            &requested(&[], &[CxxStandard::Cxx23]),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("c++23"),
            "expected the offending standard to be named, got: {err}"
        );
    }
}
