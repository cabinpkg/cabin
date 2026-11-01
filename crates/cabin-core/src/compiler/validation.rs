//! Backend-compatibility validation for resolved tools.

use std::collections::BTreeSet;

use thiserror::Error;

use super::capabilities::{
    c_standard_capability, cxx_standard_capability, standard_support_detail,
};
use super::identity::{ArchiverKind, CompilerIdentity, CompilerKind};
use crate::language_standard::{CStandard, CxxStandard};

/// Errors produced while validating detected tools against a
/// backend's required capabilities and the requested standards.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolDetectionError {
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
        "selected C compiler `{spec}` could not be identified and the current backend requires GCC-style flags"
    )]
    UnknownCRequiresGccStyle { spec: String },

    #[error(
        "selected C compiler `{spec}` ({kind}) does not support the requested C standard `{standard}`: {detail}"
    )]
    CLacksStandard {
        spec: String,
        kind: CompilerKind,
        standard: CStandard,
        detail: String,
    },

    #[error(
        "selected archiver `{spec}` could not be identified and the current backend requires `ar crs`-compatible behavior"
    )]
    UnknownArchiverRequiresArCompatible { spec: String },
}

/// Validate that the resolved C++ compiler can drive one of
/// Cabin's two C++ backends.
///
/// An MSVC-dialect compiler (`cl`, `clang-cl`) drives the `cl.exe`
/// backend; every other recognized compiler drives the GCC/Clang
/// backend.  Detection derives the backend capabilities from the
/// kind alone, so every recognized kind fits one backend and only an
/// unidentified compiler is rejected.  Support for the *requested*
/// language standards is validated separately by
/// [`validate_cxx_standards`] / [`validate_c_standards`].
///
/// # Errors
/// Returns [`ToolDetectionError::UnknownCxxRequiresGccStyle`] for
/// [`CompilerKind::Unknown`].
pub fn validate_cxx_for_backend(
    spec_display: &str,
    kind: CompilerKind,
) -> Result<(), ToolDetectionError> {
    if kind == CompilerKind::Unknown {
        return Err(ToolDetectionError::UnknownCxxRequiresGccStyle {
            spec: spec_display.to_owned(),
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

/// C-side twin of [`validate_cxx_for_backend`].
///
/// # Errors
/// Returns [`ToolDetectionError::UnknownCRequiresGccStyle`] for
/// [`CompilerKind::Unknown`].
pub fn validate_cc_for_backend(
    spec_display: &str,
    kind: CompilerKind,
) -> Result<(), ToolDetectionError> {
    if kind == CompilerKind::Unknown {
        return Err(ToolDetectionError::UnknownCRequiresGccStyle {
            spec: spec_display.to_owned(),
        });
    }
    Ok(())
}

/// Validate that the resolved archiver can drive one of Cabin's
/// two static-library backends: `lib.exe` for MSVC
/// (`lib /OUT:<lib> <objs>`), or an `ar`-compatible archiver for
/// GCC/Clang (`ar crs <lib> <objs>`).  Every recognized kind fits
/// one of them.
///
/// # Errors
/// Returns [`ToolDetectionError::UnknownArchiverRequiresArCompatible`]
/// for [`ArchiverKind::Unknown`].
pub fn validate_ar_for_backend(
    spec_display: &str,
    kind: ArchiverKind,
) -> Result<(), ToolDetectionError> {
    if kind == ArchiverKind::Unknown {
        return Err(ToolDetectionError::UnknownArchiverRequiresArCompatible {
            spec: spec_display.to_owned(),
        });
    }
    Ok(())
}
