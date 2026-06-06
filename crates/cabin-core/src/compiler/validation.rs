//! Backend-compatibility validation for resolved tools.

use thiserror::Error;

use super::capabilities::{ArchiverCapabilities, CompilerCapabilities};
use super::identity::{ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind};

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
        "selected C++ compiler `{spec}` ({kind}) does not support the required C++17 standard flag"
    )]
    CxxLacksStdCxx17 { spec: String, kind: CompilerKind },

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
        "selected C compiler `{spec}` ({kind}) does not support the required C11 standard flag (MSVC `/std:c11` needs VS2019 16.8 / `cl` 19.28 or newer)"
    )]
    CLacksStdC11 { spec: String, kind: CompilerKind },

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
/// MSVC command-line dialect (`/std:c++17`, `/showIncludes`,
/// `/D` / `/I` / `/c` / `/Fo`). Every other recognized compiler
/// drives the GCC/Clang backend, which requires `-std=c++17`,
/// `-MMD -MF`, and GCC-style `-D` / `-I` / `-c` / `-o`. A
/// compiler that fits neither contract is a hard error.
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedCxxBackend`] when the compiler fits
/// no backend, [`ToolDetectionError::UnknownCxxRequiresGccStyle`] when an
/// unidentified compiler lacks GCC-style flags,
/// [`ToolDetectionError::CxxLacksDepfile`] when `-MMD -MF` is unsupported, and
/// [`ToolDetectionError::CxxLacksStdCxx17`] when `-std=c++17` is unsupported.
pub fn validate_cxx_for_backend(
    spec_display: &str,
    identity: &CompilerIdentity,
    capabilities: &CompilerCapabilities,
) -> Result<(), ToolDetectionError> {
    // MSVC-dialect compilers (`cl`, `clang-cl`) drive the `cl.exe`
    // backend. They always report `msvc_style_flags`, but the planner
    // also emits `/std:c++17`, which a `cl` older than VS2017 15.3
    // rejects, so hold them to the C++17 capability too rather than
    // letting an old toolset fail at the first compile.
    if identity.kind.speaks_msvc_dialect() {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCxxBackend {
                spec: spec_display.to_owned(),
            });
        }
        if !capabilities.cxx_standard_17.supported {
            return Err(ToolDetectionError::CxxLacksStdCxx17 {
                spec: spec_display.to_owned(),
                kind: identity.kind,
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
    if !capabilities.cxx_standard_17.supported {
        return Err(ToolDetectionError::CxxLacksStdCxx17 {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    Ok(())
}

/// Validate that the resolved C compiler supports the C-side
/// command shape the active backend emits. An MSVC compiler
/// drives the `cl.exe` backend; every other recognized compiler
/// drives the GCC/Clang backend, which needs GCC-style flags
/// plus `-MMD -MF` depfile generation. Unlike
/// [`validate_cxx_for_backend`], the GCC/Clang path does **not**
/// require `-std=c++17` support — a pure-C driver that lacks
/// C++ mode is acceptable when the target only carries C
/// translation units.
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
    // The planner emits `/std:c11` for C compiles, which a `cl` older
    // than VS2019 16.8 rejects, so hold them to the C11 capability
    // rather than failing at the first compile.
    if identity.kind.speaks_msvc_dialect() {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCBackend {
                spec: spec_display.to_owned(),
            });
        }
        if !capabilities.c_standard_11.supported {
            return Err(ToolDetectionError::CLacksStdC11 {
                spec: spec_display.to_owned(),
                kind: identity.kind,
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
