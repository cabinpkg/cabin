//! The sysroot a GCC/Clang build passes: an explicit choice, or the
//! platform's Apple SDK for Apple Clang, made explicit so a switch of
//! SDK reaches the command lines and rebuilds.

use std::ffi::OsStr;
use std::path::Path;
use std::thread;

use cabin_core::{
    CompilerKind, ResolvedToolchain, Sysroot, TargetPlatform, ToolchainDetectionReport,
};
use camino::Utf8PathBuf;
use thiserror::Error;

use crate::detect::{DetectionError, ToolRunner, detect_toolchain};

/// A sysroot requested for a compiler that takes none.
#[derive(Debug, Error)]
#[error(
    "a sysroot applies to GCC and Clang, but the {slot} compiler is {kind}, which takes the Windows SDK from `INCLUDE` and `LIB`"
)]
pub struct SysrootDialectError {
    slot: &'static str,
    kind: CompilerKind,
}

/// The explicitly chosen sysroot for `platform`.
///
/// # Errors
/// [`SysrootDialectError`] when a detected compiler speaks the MSVC
/// dialect.  Without a `report`, the compilers go unchecked.
pub fn explicit_sysroot(
    path: Utf8PathBuf,
    platform: &TargetPlatform,
    report: Option<&ToolchainDetectionReport>,
) -> Result<Sysroot, SysrootDialectError> {
    if let Some(report) = report {
        let compilers =
            std::iter::once(("C++", &report.cxx)).chain(report.cc.as_ref().map(|cc| ("C", cc)));
        for (slot, detection) in compilers {
            let kind = detection.identity.kind;
            if kind.speaks_msvc_dialect() {
                return Err(SysrootDialectError { slot, kind });
            }
        }
    }
    Ok(Sysroot {
        path,
        apple: is_apple(platform),
    })
}

/// [`detect_toolchain`], and the SDK Apple Clang builds `platform`
/// against when no sysroot is chosen: `SDKROOT` when the driver would
/// honor it (an existing absolute path other than `/`), else what
/// `xcrun` names for the platform.  The SDK is `None` for other
/// platforms or compilers, or when `xcrun` names nothing, which leaves
/// the driver to its own default.
///
/// Finding the SDK needs the compilers only to decide whether it
/// applies, so on Apple platforms `xcrun` runs beside the version
/// probes, and its answer goes unused for any other compiler.
///
/// # Errors
/// Those of [`detect_toolchain`].
pub fn detect_toolchain_and_apple_sdk(
    toolchain: &ResolvedToolchain,
    platform: &TargetPlatform,
    sdkroot: Option<&OsStr>,
    runner: &dyn ToolRunner,
) -> Result<(ToolchainDetectionReport, Option<Sysroot>), DetectionError> {
    if !is_apple(platform) {
        return Ok((detect_toolchain(toolchain, runner)?, None));
    }
    thread::scope(|scope| {
        let sdk = scope.spawn(|| apple_sdk(platform, sdkroot, runner));
        let report = detect_toolchain(toolchain, runner)?;
        let sdk = sdk
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let apple_clang = |kind| kind == CompilerKind::AppleClang;
        let applies = apple_clang(report.cxx.identity.kind)
            && report
                .cc
                .as_ref()
                .is_none_or(|cc| apple_clang(cc.identity.kind));
        Ok((report, sdk.filter(|_| applies)))
    })
}

/// The SDK an Apple Clang build of `platform` uses, whatever the
/// compilers turn out to be.
fn apple_sdk(
    platform: &TargetPlatform,
    sdkroot: Option<&OsStr>,
    runner: &dyn ToolRunner,
) -> Option<Sysroot> {
    let path = match sdkroot.and_then(OsStr::to_str) {
        Some(sdkroot)
            if Path::new(sdkroot).is_absolute()
                && sdkroot != "/"
                && Path::new(sdkroot).exists() =>
        {
            sdkroot.to_owned()
        }
        _ => {
            // Mac Catalyst (`-macabi`) builds against the macOS SDK, and
            // Apple Clang builds any other x86 iOS triple for the simulator.
            let sdk = match (platform.os.as_str(), platform.abi.as_str()) {
                ("ios", "macabi") | ("macos", _) => "macosx",
                ("ios", "sim") => "iphonesimulator",
                ("ios", _) if matches!(platform.arch.as_str(), "x86" | "x86_64") => {
                    "iphonesimulator"
                }
                _ => "iphoneos",
            };
            let output = runner
                .run(Path::new("xcrun"), &["--sdk", sdk, "--show-sdk-path"])
                .ok()
                .filter(|output| output.status == 0)?;
            output.stdout.trim().to_owned()
        }
    };
    (!path.is_empty()).then(|| Sysroot {
        path: Utf8PathBuf::from(path),
        apple: true,
    })
}

fn is_apple(platform: &TargetPlatform) -> bool {
    matches!(platform.os.as_str(), "macos" | "ios")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::test_support::FakeRunner;
    use cabin_core::{
        ArchiverIdentity, ArchiverKind, CompilerIdentity, ResolvedTool, ToolDetection, ToolKind,
        ToolSource, ToolSpec,
    };

    fn report(cxx: CompilerKind, cc: Option<CompilerKind>) -> ToolchainDetectionReport {
        let compiler = |kind| {
            let identity = CompilerIdentity {
                kind,
                version: None,
                target: None,
                raw_version_line: String::new(),
            };
            ToolDetection {
                path: "/usr/bin/cc".into(),
                capabilities: cabin_core::derive_cxx_capabilities(&identity),
                identity,
            }
        };
        let ar = ArchiverIdentity {
            kind: ArchiverKind::Ar,
            version: None,
            raw_version_line: String::new(),
        };
        ToolchainDetectionReport {
            cxx: compiler(cxx),
            cc: cc.map(compiler),
            ar: ToolDetection {
                path: "/usr/bin/ar".into(),
                capabilities: cabin_core::derive_ar_capabilities(&ar),
                identity: ar,
            },
        }
    }

    fn platform(triple: &str) -> TargetPlatform {
        TargetPlatform::from_triple(triple).unwrap()
    }

    #[test]
    fn explicit_sysroot_refuses_an_msvc_style_compiler() {
        let linux = platform("aarch64-unknown-linux-gnu");
        for kind in [CompilerKind::Msvc, CompilerKind::ClangCl] {
            let report = report(CompilerKind::Gcc, Some(kind));
            let err = explicit_sysroot("/sysroot".into(), &linux, Some(&report)).unwrap_err();
            assert!(err.to_string().contains("the C compiler is"), "{err}");
        }
        let sysroot = explicit_sysroot(
            "/sysroot".into(),
            &linux,
            Some(&report(CompilerKind::Gcc, None)),
        )
        .unwrap();
        assert!(!sysroot.apple);
        let mac = platform("x86_64-apple-macos13");
        assert!(explicit_sysroot("/sdk".into(), &mac, None).unwrap().apple);
    }

    #[test]
    fn apple_clang_gets_the_sdk_xcrun_names_for_the_platform() {
        const APPLE: &str = "Apple clang version 17.0.0 (clang-1700.0.13.3)\n";
        let runner = |cxx: &str, cc: &str| {
            FakeRunner::new()
                .with("/bin/c++", &["--version"], cxx, "", 0)
                .with("/bin/cc", &["--version"], cc, "", 0)
                .with("/bin/ar", &["--version"], "GNU ar 2.40\n", "", 0)
                .with(
                    "xcrun",
                    &["--sdk", "macosx", "--show-sdk-path"],
                    "/sdks/MacOSX27.2.sdk\n",
                    "",
                    0,
                )
                .with(
                    "xcrun",
                    &["--sdk", "iphonesimulator", "--show-sdk-path"],
                    "/sdks/iPhoneSimulator.sdk\n",
                    "",
                    0,
                )
        };
        let tool = |kind, path: &str| ResolvedTool {
            kind,
            path: path.into(),
            spec: ToolSpec::Name(path.into()),
            source: ToolSource::Default,
        };
        let toolchain = ResolvedToolchain {
            cxx: tool(ToolKind::CxxCompiler, "/bin/c++"),
            cc: Some(tool(ToolKind::CCompiler, "/bin/cc")),
            ar: tool(ToolKind::Archiver, "/bin/ar"),
        };
        let discover = |triple, runner: &FakeRunner, sdkroot: Option<&str>| {
            let (_, sdk) = detect_toolchain_and_apple_sdk(
                &toolchain,
                &platform(triple),
                sdkroot.map(OsStr::new),
                runner,
            )
            .unwrap();
            sdk.map(|sysroot| sysroot.path)
        };
        let apple = runner(APPLE, APPLE);
        assert_eq!(
            discover("arm64-apple-macos13", &apple, None).unwrap(),
            "/sdks/MacOSX27.2.sdk"
        );
        assert_eq!(
            discover("arm64-apple-ios17.0-simulator", &apple, None).unwrap(),
            "/sdks/iPhoneSimulator.sdk"
        );
        assert_eq!(
            discover("x86_64-apple-ios17.0", &apple, None).unwrap(),
            "/sdks/iPhoneSimulator.sdk"
        );
        assert_eq!(
            discover("x86_64-apple-ios14.0-macabi", &apple, None).unwrap(),
            "/sdks/MacOSX27.2.sdk"
        );
        // An existing absolute `SDKROOT` wins, as in Apple Clang; a
        // name, a missing path, or `/` does not.
        let dir = assert_fs::TempDir::new().unwrap();
        let old = dir.path().to_str().unwrap();
        assert_eq!(
            discover("arm64-apple-macos13", &apple, Some(old)).unwrap(),
            old
        );
        let missing = dir.path().join("Missing.sdk");
        for sdkroot in ["macosx26", missing.to_str().unwrap(), "/"] {
            assert_eq!(
                discover("arm64-apple-macos13", &apple, Some(sdkroot)).unwrap(),
                "/sdks/MacOSX27.2.sdk"
            );
        }
        // Other compilers and platforms keep their own default; an SDK
        // `xcrun` cannot name (here, `iphoneos`) leaves the driver's.
        let gcc = runner("gcc (GCC) 13.2.0\n", "gcc (GCC) 13.2.0\n");
        let mixed = runner(APPLE, "clang version 17.0.6\n");
        assert_eq!(discover("arm64-apple-macos13", &gcc, None), None);
        assert_eq!(discover("arm64-apple-macos13", &mixed, None), None);
        assert_eq!(discover("aarch64-unknown-linux-gnu", &apple, None), None);
        assert_eq!(discover("arm64-apple-ios17.0", &apple, None), None);
    }
}
