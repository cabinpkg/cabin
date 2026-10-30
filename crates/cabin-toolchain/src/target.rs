//! Agreement between the platform a command builds for and the
//! platform its detected compilers build for.

use cabin_core::{
    CompilerCapabilities, CompilerIdentity, CompilerKind, TargetPlatform, ToolDetection,
    ToolchainDetectionReport,
};
use thiserror::Error;

use crate::detect::{RunError, ToolRunner, first_non_empty_line};

/// How the compilers reach the requested platform.
#[derive(Debug, Default)]
pub struct CompilerTargets {
    /// The triple every Clang-family compile, assemble, and link
    /// command passes as `--target=`; `None` for a native build and
    /// for compilers whose target is fixed.
    pub clang_target: Option<String>,
    /// Native builds only: a compiler that builds for another machine
    /// than the build machine whose `cfg(...)` conditions apply.
    pub warnings: Vec<String>,
}

/// A compiler that cannot build for the requested `--target`.
#[derive(Debug, Error)]
pub enum CompilerTargetError {
    #[error("{compiler} builds for {actual}, but `--target` requests {requested}")]
    Mismatch {
        compiler: String,
        actual: String,
        requested: String,
    },
    #[error(
        "cannot tell which platform {compiler} builds for; `--target` needs Clang, GCC, MSVC, or clang-cl"
    )]
    Unknown { compiler: String },
    #[error(
        "`--target` needs the C and C++ compilers from one family, but the C compiler is {cc} and the C++ compiler is {cxx}"
    )]
    MixedFamilies { cc: CompilerKind, cxx: CompilerKind },
    #[error("failed to ask {compiler} for its target")]
    Probe {
        compiler: String,
        #[source]
        source: RunError,
    },
}

/// Check the detected compilers against `platform`.
///
/// With `cross` (a `--target` build), Clang and Apple Clang build the
/// triple by construction, because every command they run passes
/// `--target=`; GCC, MSVC, and clang-cl build a fixed target - GCC's
/// `-dumpmachine`, the others' banner - that must name the same
/// machine.  Without `cross`, a banner naming another machine is a
/// warning: `cfg(...)` conditions still evaluate for the build machine.
///
/// # Errors
/// Returns [`CompilerTargetError`] in a `--target` build whose
/// compilers mix families, report another machine, report nothing
/// classifiable, or fail the `-dumpmachine` probe.
pub fn check_compiler_targets(
    report: &ToolchainDetectionReport,
    platform: &TargetPlatform,
    cross: bool,
    runner: &dyn ToolRunner,
) -> Result<CompilerTargets, CompilerTargetError> {
    let compilers: Vec<&ToolDetection<CompilerIdentity, CompilerCapabilities>> =
        std::iter::once(&report.cxx).chain(&report.cc).collect();
    let is_clang = |kind| matches!(kind, CompilerKind::Clang | CompilerKind::AppleClang);
    if !cross {
        let warnings = compilers
            .iter()
            .filter_map(|compiler| {
                let actual = banner_target(&compiler.identity)?;
                // The build machine's env is a guess (`gnu` on every Linux), so
                // only arch and OS can contradict it.
                (actual.arch != platform.arch || actual.os != platform.os).then(|| {
                    format!(
                        "{} builds for {}, not the build machine {}; `cfg(...)` conditions evaluate for the build machine unless `--target` names the compiler's platform",
                        compiler.path, actual.target, platform.target
                    )
                })
            })
            .collect();
        return Ok(CompilerTargets {
            clang_target: None,
            warnings,
        });
    }
    if let Some(cc) = &report.cc
        && is_clang(cc.identity.kind) != is_clang(report.cxx.identity.kind)
    {
        return Err(CompilerTargetError::MixedFamilies {
            cc: cc.identity.kind,
            cxx: report.cxx.identity.kind,
        });
    }
    if is_clang(report.cxx.identity.kind) {
        return Ok(CompilerTargets {
            clang_target: Some(platform.target.clone()),
            warnings: Vec::new(),
        });
    }
    for compiler in compilers {
        let name = compiler.path.to_string();
        let actual = match compiler.identity.kind {
            CompilerKind::Gcc => {
                let output = runner
                    .run(compiler.path.as_std_path(), &["-dumpmachine"])
                    .map_err(|source| CompilerTargetError::Probe {
                        compiler: name.clone(),
                        source,
                    })?;
                TargetPlatform::from_triple(&first_non_empty_line(&output.stdout)).ok()
            }
            _ => banner_target(&compiler.identity),
        }
        .ok_or_else(|| CompilerTargetError::Unknown {
            compiler: name.clone(),
        })?;
        if !actual.same_machine(platform) {
            return Err(CompilerTargetError::Mismatch {
                compiler: name,
                actual: actual.target,
                requested: platform.target.clone(),
            });
        }
    }
    Ok(CompilerTargets::default())
}

fn banner_target(identity: &CompilerIdentity) -> Option<TargetPlatform> {
    TargetPlatform::from_triple(identity.target.as_deref()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::test_support::FakeRunner;
    use cabin_core::{ArchiverIdentity, ArchiverKind};

    fn compiler(
        kind: CompilerKind,
        path: &str,
        target: Option<&str>,
    ) -> ToolDetection<CompilerIdentity, CompilerCapabilities> {
        let identity = CompilerIdentity {
            kind,
            version: None,
            target: target.map(str::to_owned),
            raw_version_line: String::new(),
        };
        ToolDetection {
            path: path.into(),
            capabilities: cabin_core::derive_cxx_capabilities(&identity),
            identity,
        }
    }

    fn report(
        cxx: ToolDetection<CompilerIdentity, CompilerCapabilities>,
        cc: Option<ToolDetection<CompilerIdentity, CompilerCapabilities>>,
    ) -> ToolchainDetectionReport {
        let ar = ArchiverIdentity {
            kind: ArchiverKind::Ar,
            version: None,
            raw_version_line: String::new(),
        };
        ToolchainDetectionReport {
            cxx,
            cc,
            ar: ToolDetection {
                path: "/bin/ar".into(),
                capabilities: cabin_core::derive_ar_capabilities(&ar),
                identity: ar,
            },
        }
    }

    fn linux_arm64() -> TargetPlatform {
        TargetPlatform::from_triple("aarch64-unknown-linux-gnu").unwrap()
    }

    #[test]
    fn clang_builds_the_requested_triple() {
        let report = report(
            compiler(
                CompilerKind::Clang,
                "/bin/clang++",
                Some("x86_64-pc-linux-gnu"),
            ),
            Some(compiler(
                CompilerKind::Clang,
                "/bin/clang",
                Some("x86_64-pc-linux-gnu"),
            )),
        );
        let targets =
            check_compiler_targets(&report, &linux_arm64(), true, &FakeRunner::new()).unwrap();
        assert_eq!(
            targets.clang_target.as_deref(),
            Some("aarch64-unknown-linux-gnu")
        );
    }

    #[test]
    fn gcc_must_report_the_requested_machine() {
        let report = report(compiler(CompilerKind::Gcc, "/bin/g++", None), None);
        let runner =
            FakeRunner::new().with("/bin/g++", &["-dumpmachine"], "aarch64-linux-gnu\n", "", 0);
        let targets = check_compiler_targets(&report, &linux_arm64(), true, &runner).unwrap();
        assert_eq!(targets.clang_target, None);

        let runner =
            FakeRunner::new().with("/bin/g++", &["-dumpmachine"], "x86_64-linux-gnu\n", "", 0);
        let err = check_compiler_targets(&report, &linux_arm64(), true, &runner).unwrap_err();
        assert!(
            matches!(&err, CompilerTargetError::Mismatch { actual, .. } if actual == "x86_64-linux-gnu"),
            "{err}"
        );
    }

    #[test]
    fn msvc_must_report_the_requested_machine() {
        let msvc = report(
            compiler(
                CompilerKind::Msvc,
                "/vc/cl.exe",
                Some("x86_64-pc-windows-msvc"),
            ),
            None,
        );
        let arm64 = TargetPlatform::from_triple("aarch64-pc-windows-msvc").unwrap();
        assert!(matches!(
            check_compiler_targets(&msvc, &arm64, true, &FakeRunner::new()),
            Err(CompilerTargetError::Mismatch { .. })
        ));
        let x64 = TargetPlatform::from_triple("x86_64-pc-windows-msvc").unwrap();
        check_compiler_targets(&msvc, &x64, true, &FakeRunner::new()).unwrap();
    }

    #[test]
    fn cross_rejects_mixed_and_unknown_compilers() {
        let mixed = report(
            compiler(CompilerKind::Clang, "/bin/clang++", None),
            Some(compiler(CompilerKind::Gcc, "/bin/gcc", None)),
        );
        assert!(matches!(
            check_compiler_targets(&mixed, &linux_arm64(), true, &FakeRunner::new()),
            Err(CompilerTargetError::MixedFamilies { .. })
        ));
        let unknown = report(compiler(CompilerKind::Unknown, "/bin/c++", None), None);
        assert!(matches!(
            check_compiler_targets(&unknown, &linux_arm64(), true, &FakeRunner::new()),
            Err(CompilerTargetError::Unknown { .. })
        ));
    }

    #[test]
    fn native_build_warns_about_a_compiler_for_another_machine() {
        let report = report(
            compiler(
                CompilerKind::Clang,
                "/bin/clang++",
                Some("x86_64-pc-linux-gnu"),
            ),
            Some(compiler(
                CompilerKind::Clang,
                "/bin/clang",
                Some("aarch64-alpine-linux-musl"),
            )),
        );
        let targets =
            check_compiler_targets(&report, &linux_arm64(), false, &FakeRunner::new()).unwrap();
        assert_eq!(targets.clang_target, None);
        assert_eq!(targets.warnings.len(), 1, "{:?}", targets.warnings);
        assert!(targets.warnings[0].contains("x86_64-pc-linux-gnu"));
    }
}
