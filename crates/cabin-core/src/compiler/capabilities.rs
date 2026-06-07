//! Capability model and capability derivation from tool identity.

use serde::{Deserialize, Serialize};

use super::identity::{
    ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind, CompilerVersion,
};

/// Where one capability decision came from. Recorded so
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
    /// capability (e.g. MSVC asked for GCC-style flags).
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

/// Capability set for a C/C++ compiler. Every field is decided
/// during detection so the planner can compare its required set
/// against the resolved set without re-running parsing logic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerCapabilities {
    /// Accepts GCC-style `-O<n>`, `-DNAME`, `-Idir`, `-c`, `-o`.
    pub gcc_style_flags: Capability,
    /// Accepts MSVC-style `/O<n>`, `/DNAME`, `/I dir`. Detection-
    /// only; the current backend never emits these.
    pub msvc_style_flags: Capability,
    /// Accepts `-MMD -MF <file>` to write a make-style depfile.
    pub depfile_mmd_mf: Capability,
    /// Accepts `-std=c++NN`.
    pub std_flag: Capability,
    /// Accepts `-std=c++17` specifically (the planner's current
    /// fixed C++ standard).
    pub cxx_standard_17: Capability,
    /// Accepts `-std=c11` specifically (the planner's current fixed
    /// C standard). For MSVC this is the `/std:c11` switch, which is
    /// only available from VS2019 16.8 (`cl` 19.28) onward.
    pub c_standard_11: Capability,
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
/// identity. Decisions are made from the recognized compiler
/// kind, with conservative defaults for [`CompilerKind::Unknown`].
/// No probe commands are run from this function — the caller's
/// detection layer already gathered everything we need.
/// Decide a version-gated capability for an MSVC `cl` whose minimum
/// supporting version is `(min_major, min_minor)`. A parsed `cl`
/// version at or above the threshold is `supported`; below it,
/// `unsupported`. An unparsed version (`None`) is `supported` as an
/// assumed default — a real `cl` always reports a version, so a parse
/// miss must not reject an otherwise-modern compiler (mirrors the GCC
/// `cxx_standard_17` gate's `None` policy).
fn msvc_versioned_capability(
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
    let std_flag = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind.speaks_msvc_dialect() {
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    // Every Clang we recognize supports `-std=c++17` / `/std:c++17`
    // regardless of its reported version, including `clang-cl` (whose
    // banner is a clang version, not a `cl` version). Any GCC modern
    // enough to print a major version supports it too (`g++ -std=c++17`
    // arrived in GCC 5). `cl` is version-gated separately below.
    let cxx_standard_17 = match identity.kind {
        CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::ClangCl => {
            Capability::supported_from(CapabilitySource::Version)
        }
        CompilerKind::Gcc => match identity.version.as_ref().map(|v| v.major) {
            Some(m) if m >= 5 => Capability::supported_from(CapabilitySource::Version),
            Some(_) => Capability::unsupported_from(CapabilitySource::Version),
            None => Capability::supported_from(CapabilitySource::AssumedDefault),
        },
        // `cl /std:c++17` is available from VS2017 15.3 (`cl` 19.11).
        CompilerKind::Msvc => msvc_versioned_capability(identity.version.as_ref(), 19, 11),
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    };
    // `-std=c11` (and `clang-cl /std:c11`) has been available far
    // longer than C++17 in GCC/Clang, so every recognized GCC/Clang
    // (incl. `clang-cl`) supports it. `cl`'s `/std:c11` is newer:
    // VS2019 16.8 (`cl` 19.28).
    let c_standard_11 = match identity.kind {
        CompilerKind::Clang
        | CompilerKind::AppleClang
        | CompilerKind::ClangCl
        | CompilerKind::Gcc => Capability::supported_from(CapabilitySource::Version),
        CompilerKind::Msvc => msvc_versioned_capability(identity.version.as_ref(), 19, 28),
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    };

    CompilerCapabilities {
        gcc_style_flags: gcc_style,
        msvc_style_flags: msvc_style,
        depfile_mmd_mf,
        std_flag,
        cxx_standard_17,
        c_standard_11,
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
    // `ar crs`, `lib.exe` via `lib /OUT:`. The `ar_crs` capability
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
