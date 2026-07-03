//! Capability model and capability derivation from tool identity.

use serde::{Deserialize, Serialize};

use super::identity::{
    ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind, CompilerVersion,
};
use crate::language_standard::{CStandard, CxxStandard};

/// Where one capability decision came from.  Recorded so
/// `cabin metadata` can show whether Cabin trusted the version
/// alone, ran a probe, or fell back to a conservative default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySource {
    /// Inferred from a recognized compiler kind/version.
    Version,
    /// Conservative default applied when the compiler kind is
    /// `Unknown` or detection failed.
    AssumedDefault,
    /// The selected tool is recognizably unable to provide this
    /// capability (e.g.  MSVC asked for GCC-style flags).
    Unsupported,
}

impl CapabilitySource {
    pub fn as_key(self) -> &'static str {
        match self {
            CapabilitySource::Version => "version",
            CapabilitySource::AssumedDefault => "assumed-default",
            CapabilitySource::Unsupported => "unsupported",
        }
    }
}

/// One typed capability decision: whether the tool supports it,
/// and where the answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub supported: bool,
    pub source: CapabilitySource,
}

impl Capability {
    pub fn supported_from(source: CapabilitySource) -> Self {
        Self {
            supported: true,
            source,
        }
    }
    pub fn unsupported_from(source: CapabilitySource) -> Self {
        Self {
            supported: false,
            source,
        }
    }
}

/// Capability set for a C/C++ compiler.  Every field is decided
/// during detection so the planner can compare its required set
/// against the resolved set without re-running parsing logic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerCapabilities {
    /// Accepts GCC-style `-O<n>`, `-DNAME`, `-Idir`, `-c`, `-o`.
    pub gcc_style_flags: Capability,
    /// Accepts MSVC-style `/O<n>`, `/DNAME`, `/I dir`.  Detection-
    /// only; the current backend never emits these.
    pub msvc_style_flags: Capability,
    /// Accepts `-MMD -MF <file>` to write a make-style depfile.
    pub depfile_mmd_mf: Capability,
    /// Can mark dependency include directories as *system* search
    /// paths so diagnostics inside their headers are suppressed.
    /// GCC/Clang spell this `-isystem` (part of the base command
    /// line); `cl` spells it `/external:I` (+ `/external:W0`),
    /// non-experimental from VS2019 16.10 (`cl` 19.29); `clang-cl`
    /// accepts the `/external:` block since clang 13.
    pub external_include_dirs: Capability,
}

/// Capability set for a static-library archiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiverCapabilities {
    /// Accepts the `crs` mode flags (the planner's archive form).
    pub ar_crs: Capability,
    /// Produces a `.a` static library archive.
    pub static_library_output: Capability,
}

/// Derive a [`CompilerCapabilities`] set from the detected
/// identity.  Decisions are made from the recognized compiler
/// kind, with conservative defaults for [`CompilerKind::Unknown`].
/// No probe commands are run from this function - the caller's
/// detection layer already gathered everything we need.
/// Decide a version-gated capability for a recognized compiler whose
/// minimum supporting version is `(min_major, min_minor)`.  A parsed
/// version at or above the threshold is `supported`; below it,
/// `unsupported`.  An unparsed version (`None`) is `supported` as an
/// assumed default - a recognized compiler always reports a version,
/// so a parse miss must not reject an otherwise-modern compiler,
/// matching the per-standard validation policy.
fn version_gated_capability(
    version: Option<&CompilerVersion>,
    min_major: u32,
    min_minor: u32,
) -> Capability {
    match version.map(|v| (v.major, v.minor.unwrap_or(0))) {
        Some((major, minor)) if major > min_major || (major == min_major && minor >= min_minor) => {
            Capability::supported_from(CapabilitySource::Version)
        }
        Some(_) => Capability::unsupported_from(CapabilitySource::Version),
        None => Capability::supported_from(CapabilitySource::AssumedDefault),
    }
}

pub fn derive_cxx_capabilities(identity: &CompilerIdentity) -> CompilerCapabilities {
    let gcc_style = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind.speaks_msvc_dialect() {
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let msvc_style = if identity.kind.speaks_msvc_dialect() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let depfile_mmd_mf = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind.speaks_msvc_dialect() {
        // MSVC-dialect compilers discover headers with `/showIncludes`,
        // not a make-style depfile.
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    // `-isystem` is part of the base GCC/Clang command line, so every
    // recognized GNU-dialect compiler can mark dependency includes as
    // system search paths. `cl /external:I` left
    // `/experimental:external` in VS2019 16.10 (`cl` 19.29);
    // `clang-cl` understands the `/external:` block since clang 13.
    // Unknown compilers stay conservative so the planner never
    // assumes `-isystem` semantics for a compiler it cannot identify.
    let external_include_dirs = match identity.kind {
        CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::Gcc => {
            Capability::supported_from(CapabilitySource::Version)
        }
        CompilerKind::ClangCl => version_gated_capability(identity.version.as_ref(), 13, 0),
        CompilerKind::Msvc => version_gated_capability(identity.version.as_ref(), 19, 29),
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    };

    CompilerCapabilities {
        gcc_style_flags: gcc_style,
        msvc_style_flags: msvc_style,
        depfile_mmd_mf,
        external_include_dirs,
    }
}

/// Whether `identity` accepts the exact flag spelling for a C
/// `standard` (`-std=<std>` on the GNU dialect, `/std:<std>` on
/// MSVC).  Version thresholds follow the audited table in
/// `docs/language-standards.md`: unknown versions fail open
/// (assumed-default), recognized-but-old versions fail closed, and
/// MSVC-dialect gaps (no stable flag at any version) are
/// `Unsupported`.  Unknown compiler kinds are rejected earlier by
/// the backend validation, so their entry here is conservative.
#[must_use]
pub fn c_standard_capability(identity: &CompilerIdentity, standard: CStandard) -> Capability {
    use CStandard::{C11, C17, C23, C89, C99};
    let version = identity.version.as_ref();
    let always = Capability::supported_from(CapabilitySource::Version);
    let no_flag = Capability::unsupported_from(CapabilitySource::Unsupported);
    match identity.kind {
        CompilerKind::Gcc => match standard {
            C89 | C99 | C11 => always,
            // `-std=c17` spelling: GCC 8; `-std=c23`: GCC 14.
            C17 => version_gated_capability(version, 8, 0),
            C23 => version_gated_capability(version, 14, 0),
        },
        CompilerKind::Clang => match standard {
            C89 | C99 | C11 => always,
            // `-std=c17`: Clang 6; `-std=c23`: Clang 18.
            C17 => version_gated_capability(version, 6, 0),
            C23 => version_gated_capability(version, 18, 0),
        },
        CompilerKind::AppleClang => match standard {
            C89 | C99 | C11 => always,
            // Apple clang 10 (Xcode 10) is LLVM-6-based; Apple
            // clang 17 (Xcode 16.3) is LLVM-19-based (>= 18).
            C17 => version_gated_capability(version, 10, 0),
            C23 => version_gated_capability(version, 17, 0),
        },
        // `clang-cl` gained `/std:c11` / `/std:c17` in Clang 13
        // (LLVM D95575); there is no `/std:` spelling for the rest.
        CompilerKind::ClangCl => match standard {
            C11 | C17 => version_gated_capability(version, 13, 0),
            C89 | C99 | C23 => no_flag,
        },
        // `cl /std:c11` and `/std:c17` arrived together in VS2019
        // 16.8 (`cl` 19.28); C89/C99/C23 have no selection flag.
        CompilerKind::Msvc => match standard {
            C11 | C17 => version_gated_capability(version, 19, 28),
            C89 | C99 | C23 => no_flag,
        },
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    }
}

/// C++ twin of [`c_standard_capability`].
#[must_use]
pub fn cxx_standard_capability(identity: &CompilerIdentity, standard: CxxStandard) -> Capability {
    use CxxStandard::{Cxx11, Cxx14, Cxx17, Cxx20, Cxx23, Cxx26, Cxx98};
    let version = identity.version.as_ref();
    let always = Capability::supported_from(CapabilitySource::Version);
    let no_flag = Capability::unsupported_from(CapabilitySource::Unsupported);
    match identity.kind {
        CompilerKind::Gcc => match standard {
            Cxx98 | Cxx11 => always,
            // GCC >= 5 for the c++14 / c++17 spellings (the
            // repository's long-standing c++17 gate); `-std=c++20`:
            // GCC 10; `-std=c++23`: GCC 11; `-std=c++26`: GCC 14.
            Cxx14 | Cxx17 => version_gated_capability(version, 5, 0),
            Cxx20 => version_gated_capability(version, 10, 0),
            Cxx23 => version_gated_capability(version, 11, 0),
            Cxx26 => version_gated_capability(version, 14, 0),
        },
        CompilerKind::Clang => match standard {
            Cxx98 | Cxx11 | Cxx14 | Cxx17 => always,
            // `-std=c++20` spelling shipped in Clang 10 (the
            // `release/10.x` LangStandards.def already names
            // `c++20`, with `c++2a` as the deprecated alias; Clang
            // 9 only had `c++2a`); `-std=c++23` and `-std=c++26`
            // both landed in Clang 17.
            Cxx20 => version_gated_capability(version, 10, 0),
            Cxx23 | Cxx26 => version_gated_capability(version, 17, 0),
        },
        CompilerKind::AppleClang => match standard {
            Cxx98 | Cxx11 | Cxx14 | Cxx17 => always,
            // Xcode <-> LLVM mapping: every Apple clang 12 is at
            // least LLVM-10-based, and LLVM 10 already spells
            // `-std=c++20`.  Apple clang 16 (Xcode 16) is
            // LLVM-17-based for the c++23 / c++26 spellings.
            Cxx20 => version_gated_capability(version, 12, 0),
            Cxx23 | Cxx26 => version_gated_capability(version, 16, 0),
        },
        CompilerKind::ClangCl => match standard {
            Cxx14 | Cxx17 => always,
            // Conservative: `/std:c++20` is present by Clang 13.
            Cxx20 => version_gated_capability(version, 13, 0),
            // No stable `/std:c++23` / `/std:c++26` exists as of
            // Clang 22 (only `c++23preview` / `c++latest`), and no
            // `/std:` spelling for the pre-C++14 standards.
            Cxx98 | Cxx11 | Cxx23 | Cxx26 => no_flag,
        },
        CompilerKind::Msvc => match standard {
            // `/std:` selection starts at C++14 (VS2017 / `cl`
            // 19.10); `/std:c++17` from `cl` 19.11; `/std:c++20`
            // became stable in VS2019 16.11 (`cl` 19.29 - 16.10
            // shares the minor, so this slightly over-accepts it).
            Cxx14 => version_gated_capability(version, 19, 10),
            Cxx17 => version_gated_capability(version, 19, 11),
            Cxx20 => version_gated_capability(version, 19, 29),
            // No stable flag: C++98/11 predate `/std:`, and
            // `/std:c++23` / `/std:c++26` only exist as
            // `c++23preview` / `c++latest`.
            Cxx98 | Cxx11 | Cxx23 | Cxx26 => no_flag,
        },
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    }
}

/// Human-readable reason for an unsupported standard capability,
/// used by the validation errors: either the compiler has no stable
/// flag for the standard at any version, or the detected version
/// predates the flag.
#[must_use]
pub fn standard_support_detail(capability: Capability, kind: CompilerKind) -> String {
    match capability.source {
        CapabilitySource::Unsupported => {
            format!("{kind} has no stable flag selecting this standard")
        }
        _ => "the detected compiler version predates support for this standard's flag".to_owned(),
    }
}

/// Derive an [`ArchiverCapabilities`] set from the detected
/// identity.
pub fn derive_ar_capabilities(identity: &ArchiverIdentity) -> ArchiverCapabilities {
    let ar_crs = if identity.kind.supports_ar_crs() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind == ArchiverKind::Lib {
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    // Honest across both dialects: `ar` / `llvm-ar` archive via
    // `ar crs`, `lib.exe` via `lib /OUT:`.  The `ar_crs` capability
    // above stays GNU-specific (`lib.exe` does not accept `crs`),
    // but both shapes do produce a static library.
    let static_library_output = if identity.kind.produces_static_library() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    ArchiverCapabilities {
        ar_crs,
        static_library_output,
    }
}
