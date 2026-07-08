//! Backend-compatibility validation for resolved tools.

use std::collections::BTreeSet;

use thiserror::Error;

use super::capabilities::{
    ArchiverCapabilities, CompilerCapabilities, c_standard_capability, cxx_standard_capability,
    standard_support_detail,
};
use super::identity::{ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind};
use crate::language_standard::{CStandard, CxxStandard};

/// Errors produced while validating a detection report against
/// the current C++ backend's required capability set.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolDetectionError {
    #[error("selected C++ compiler `{spec}` cannot be matched to a supported C++ backend")]
    UnsupportedCxxBackend { spec: String },

    #[error(
        "selected C++ compiler `{spec}` could not be identified and the current backend requires GCC-style flags"
    )]
    UnknownCxxRequiresGccStyle { spec: String },

    #[error(
        "selected C++ compiler `{spec}` ({kind}) does not support the requested C++ standard `{standard}`: {detail}"
    )]
    CxxLacksStandard {
        spec: String,
        kind: CompilerKind,
        standard: CxxStandard,
        detail: String,
    },

    #[error(
        "selected C++ compiler `{spec}` ({kind}) does not support the depfile flags required by the Ninja backend"
    )]
    CxxLacksDepfile { spec: String, kind: CompilerKind },

    #[error("selected C compiler `{spec}` cannot be matched to a supported C backend")]
    UnsupportedCBackend { spec: String },

    #[error(
        "selected C compiler `{spec}` could not be identified and the current backend requires GCC-style flags"
    )]
    UnknownCRequiresGccStyle { spec: String },

    #[error(
        "selected C compiler `{spec}` ({kind}) does not support the depfile flags required by the Ninja backend"
    )]
    CLacksDepfile { spec: String, kind: CompilerKind },

    #[error(
        "selected C compiler `{spec}` ({kind}) does not support the requested C standard `{standard}`: {detail}"
    )]
    CLacksStandard {
        spec: String,
        kind: CompilerKind,
        standard: CStandard,
        detail: String,
    },

    #[error("selected archiver `{spec}` is not supported by the static-library backend")]
    UnsupportedArchiver { spec: String },

    #[error(
        "selected archiver `{spec}` could not be identified and the current backend requires `ar crs`-compatible behavior"
    )]
    UnknownArchiverRequiresArCompatible { spec: String },
}

/// Validate that the resolved C++ compiler can drive one of
/// Cabin's two C++ backends.
///
/// An MSVC compiler drives the `cl.exe` backend, which speaks the
/// MSVC command-line dialect (`/std:`, `/showIncludes`,
/// `/D` / `/I` / `/c` / `/Fo`).  Every other recognized compiler
/// drives the GCC/Clang backend, which requires `-MMD -MF` and
/// GCC-style `-D` / `-I` / `-c` / `-o`.  A compiler that fits
/// neither contract is a hard error.  Support for the *requested*
/// language standards is validated separately by
/// [`validate_cxx_standards`] / [`validate_c_standards`].
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedCxxBackend`] when the compiler fits
/// no backend, [`ToolDetectionError::UnknownCxxRequiresGccStyle`] when an
/// unidentified compiler lacks GCC-style flags, and
/// [`ToolDetectionError::CxxLacksDepfile`] when `-MMD -MF` is unsupported.
pub fn validate_cxx_for_backend(
    spec_display: &str,
    identity: &CompilerIdentity,
    capabilities: &CompilerCapabilities,
) -> Result<(), ToolDetectionError> {
    // MSVC-dialect compilers (`cl`, `clang-cl`) drive the `cl.exe`
    // backend.
    if identity.kind.speaks_msvc_dialect() {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCxxBackend {
                spec: spec_display.to_owned(),
            });
        }
        return Ok(());
    }
    if !capabilities.gcc_style_flags.supported {
        if identity.kind == CompilerKind::Unknown {
            return Err(ToolDetectionError::UnknownCxxRequiresGccStyle {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedCxxBackend {
            spec: spec_display.to_owned(),
        });
    }
    if !capabilities.depfile_mmd_mf.supported {
        return Err(ToolDetectionError::CxxLacksDepfile {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    Ok(())
}

/// Validate that the C++ compiler accepts every requested C++
/// standard.  The whole set is checked, not the maximum: MSVC
/// support is non-monotonic (`/std:c++20` exists, `/std:c++11`
/// does not).
///
/// # Errors
/// Returns [`ToolDetectionError::CxxLacksStandard`] for the first
/// unsupported standard, with a version or no-stable-flag detail.
pub fn validate_cxx_standards(
    spec_display: &str,
    identity: &CompilerIdentity,
    requested: &BTreeSet<CxxStandard>,
) -> Result<(), ToolDetectionError> {
    for &standard in requested {
        let capability = cxx_standard_capability(identity, standard);
        if !capability.supported {
            return Err(ToolDetectionError::CxxLacksStandard {
                spec: spec_display.to_owned(),
                kind: identity.kind,
                standard,
                detail: standard_support_detail(capability, identity.kind),
            });
        }
    }
    Ok(())
}

/// C-side twin of [`validate_cxx_standards`].
///
/// # Errors
/// Returns [`ToolDetectionError::CLacksStandard`] for the first
/// unsupported standard, with a version or no-stable-flag detail.
pub fn validate_c_standards(
    spec_display: &str,
    identity: &CompilerIdentity,
    requested: &BTreeSet<CStandard>,
) -> Result<(), ToolDetectionError> {
    for &standard in requested {
        let capability = c_standard_capability(identity, standard);
        if !capability.supported {
            return Err(ToolDetectionError::CLacksStandard {
                spec: spec_display.to_owned(),
                kind: identity.kind,
                standard,
                detail: standard_support_detail(capability, identity.kind),
            });
        }
    }
    Ok(())
}

/// Validate that the resolved C compiler supports the C-side
/// command shape the active backend emits.  An MSVC compiler
/// drives the `cl.exe` backend; every other recognized compiler
/// drives the GCC/Clang backend, which needs GCC-style flags
/// plus `-MMD -MF` depfile generation.  Support for the requested
/// C standards is validated separately by
/// [`validate_c_standards`].
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedCBackend`] when the compiler fits
/// no backend, [`ToolDetectionError::UnknownCRequiresGccStyle`] when an
/// unidentified compiler lacks GCC-style flags, and
/// [`ToolDetectionError::CLacksDepfile`] when `-MMD -MF` is unsupported.
pub fn validate_cc_for_backend(
    spec_display: &str,
    identity: &CompilerIdentity,
    capabilities: &CompilerCapabilities,
) -> Result<(), ToolDetectionError> {
    // MSVC-dialect compilers (`cl`, `clang-cl`) drive the `cl.exe`
    // backend; the GCC/Clang contract below does not apply to them.
    if identity.kind.speaks_msvc_dialect() {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCBackend {
                spec: spec_display.to_owned(),
            });
        }
        return Ok(());
    }
    if !capabilities.gcc_style_flags.supported {
        if identity.kind == CompilerKind::Unknown {
            return Err(ToolDetectionError::UnknownCRequiresGccStyle {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedCBackend {
            spec: spec_display.to_owned(),
        });
    }
    if !capabilities.depfile_mmd_mf.supported {
        return Err(ToolDetectionError::CLacksDepfile {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    Ok(())
}

/// Validate that the resolved archiver can drive one of Cabin's
/// two static-library backends: `lib.exe` for MSVC
/// (`lib /OUT:<lib> <objs>`), or an `ar`-compatible archiver for
/// GCC/Clang (`ar crs <lib> <objs>`).
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedArchiver`] when a known archiver
/// lacks `ar crs` support, and
/// [`ToolDetectionError::UnknownArchiverRequiresArCompatible`] when an
/// unidentified archiver lacks `ar crs` support.
pub fn validate_ar_for_backend(
    spec_display: &str,
    identity: &ArchiverIdentity,
    capabilities: &ArchiverCapabilities,
) -> Result<(), ToolDetectionError> {
    // `lib.exe` is the MSVC static-library backend's archiver; it
    // produces the `.lib` the `cl.exe` link step consumes.
    if identity.kind == ArchiverKind::Lib {
        return Ok(());
    }
    if !capabilities.ar_crs.supported {
        if identity.kind == ArchiverKind::Unknown {
            return Err(ToolDetectionError::UnknownArchiverRequiresArCompatible {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedArchiver {
            spec: spec_display.to_owned(),
        });
    }
    Ok(())
}
