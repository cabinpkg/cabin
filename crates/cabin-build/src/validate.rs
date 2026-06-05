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
    ResolvedToolchain, ToolchainDetectionReport, validate_ar_for_backend, validate_cc_for_backend,
    validate_cxx_for_backend,
};

use crate::error::BuildError;

/// Validate that every populated tool in `report` can execute the
/// command shapes emitted by the current backend. Returns the
/// first problem encountered so users see one actionable error,
/// not a wall of unrelated failures.
///
/// The C++ compiler is held to the full C++-backend contract
/// (GCC-style flags, depfile, `-std=c++17`). The C compiler is
/// held to the *C-side* contract (GCC-style flags, depfile) —
/// this is laxer because a pure-C driver may not accept C++ mode
/// at all. The archiver gets its own narrow contract.
///
/// `toolchain` is the matching [`ResolvedToolchain`] — we use it
/// to recover the user-visible spec strings (`clang++`,
/// `/opt/llvm/bin/clang++`) for the error messages.
///
/// # Errors
/// Returns [`BuildError::UnsupportedToolchain`] (wrapping the
/// [`cabin_core::ToolDetectionError`] from the first failing
/// `validate_*_for_backend` check) when the C++ compiler, the
/// optional C compiler, or the archiver cannot run the backend's
/// command shapes — e.g. an MSVC-family tool or a compiler missing
/// a required capability such as depfile support.
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
    Ok(())
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
    fn rejects_msvc_compiler_clearly() {
        let toolchain = make_toolchain("cl.exe", "ar");
        let report = report_for(
            CompilerIdentity {
                kind: CompilerKind::Msvc,
                version: None,
                target: None,
                raw_version_line: "Microsoft Optimizing Compiler".into(),
            },
            ArchiverIdentity {
                kind: ArchiverKind::Ar,
                version: None,
                raw_version_line: "GNU ar".into(),
            },
        );
        let err = validate_toolchain_for_backend(&toolchain, &report).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("MSVC") || message.contains("GCC- or Clang-like"),
            "expected MSVC rejection, got: {message}"
        );
    }

    #[test]
    fn rejects_msvc_archiver_clearly() {
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
        let message = err.to_string();
        assert!(
            message.contains("ar-compatible") || message.contains("not supported"),
            "expected unsupported archiver, got: {message}"
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
