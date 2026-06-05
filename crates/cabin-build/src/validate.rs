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

use cabin_core::{
    ArchiverKind, ResolvedToolchain, ToolchainDetectionReport, validate_ar_for_backend,
    validate_cc_for_backend, validate_cxx_for_backend,
};

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
) -> Result<(), BuildError> {
    let cxx_spec = toolchain.cxx.spec.display();
    validate_cxx_for_backend(&cxx_spec, &report.cxx.identity, &report.cxx.capabilities)?;
    if let (Some(cc_tool), Some(cc_detection)) = (toolchain.cc.as_ref(), report.cc.as_ref()) {
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
    if let Some(cc_detection) = report.cc.as_ref() {
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
}
