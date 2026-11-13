//! The environment a package's build script runs in.
//!
//! A build script is code a package supplies, so it sees none of
//! Cabin's own environment but the variables [`PASSED_THROUGH`] names
//! and those starting with [`CABIN_SDK_PREFIX`].  Everything else it
//! sees, [`build_script_env`] sets.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{CABIN_MANIFEST_DIR, CABIN_PACKAGE_NAME, CABIN_PACKAGE_VERSION, CABIN_PROFILE};

/// Absolute path of the directory a build script generates files into.
pub const CABIN_OUT_DIR: &str = "CABIN_OUT_DIR";

/// Absolute path of the file a build script writes its result to.
pub const CABIN_RESULT_FILE: &str = "CABIN_RESULT_FILE";

/// `os` of the platform the script's package is being built for, as a
/// `cfg(...)` condition spells it.  Its `arch`, `family`, and `env`
/// follow; one the platform leaves unset is the empty string.
pub const CABIN_TARGET_OS: &str = "CABIN_TARGET_OS";
pub const CABIN_TARGET_ARCH: &str = "CABIN_TARGET_ARCH";
pub const CABIN_TARGET_FAMILY: &str = "CABIN_TARGET_FAMILY";
pub const CABIN_TARGET_ENV: &str = "CABIN_TARGET_ENV";

/// `gnu` or `msvc`: the command dialect of the compiler building the
/// script's package.
pub const CABIN_TARGET_DIALECT: &str = "CABIN_TARGET_DIALECT";

/// Absolute path of the C compiler building the script's package;
/// `CABIN_CXX` is the C++ compiler's and `CABIN_AR` the archiver's.  A
/// compiler the build has none of leaves its variable unset.
pub const CABIN_CC: &str = "CABIN_CC";
pub const CABIN_CXX: &str = "CABIN_CXX";
pub const CABIN_AR: &str = "CABIN_AR";

/// Starts the variable set to `1` for each feature of the package this
/// build enables: `CABIN_FEATURE_` and the feature's name in upper case,
/// every other character than an ASCII letter or digit as `_`.
pub const CABIN_FEATURE_PREFIX: &str = "CABIN_FEATURE_";

/// Starts the variable holding the absolute path of each
/// build-dependency executable the script may run, named after its
/// target as [`CABIN_FEATURE_PREFIX`] names a feature.
pub const CABIN_BIN_PREFIX: &str = "CABIN_BIN_";

/// Starts the variables a user sets to point a build script at an SDK.
/// Each reaches every build script unchanged.
pub const CABIN_SDK_PREFIX: &str = "CABIN_SDK_";

/// The variables of Cabin's own environment a build script keeps: what
/// finds programs, picks a locale, and locates the platform's SDK and
/// system directories.  Windows matches the names in any case.
pub const PASSED_THROUGH: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "DEVELOPER_DIR",
    "SDKROOT",
    "SystemRoot",
    "windir",
    "SystemDrive",
    "ComSpec",
    "PATHEXT",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "INCLUDE",
    "LIB",
    "LIBPATH",
];

/// Inputs for [`build_script_env`].
#[derive(Debug, Clone)]
pub struct BuildScriptEnvInputs<'a> {
    pub out_dir: &'a Path,
    pub result_file: &'a Path,
    /// A directory of the script's own that stands in for the user's
    /// home.
    pub home: &'a Path,
    /// A directory of the script's own for temporary files.
    pub tmp: &'a Path,
    pub manifest_dir: &'a Path,
    pub package_name: &'a str,
    pub package_version: &'a str,
    pub profile: &'a str,
    pub target_os: &'a str,
    pub target_arch: &'a str,
    pub target_family: &'a str,
    pub target_env: &'a str,
    /// Whether the package's compiler takes MSVC-style commands.
    pub msvc: bool,
    pub toolchain: &'a BuildScriptToolchain,
    /// Every feature the package declares, and whether this build
    /// enables it.
    pub features: &'a [(&'a str, bool)],
    /// The build-dependency executables the script may run, by target
    /// name.
    pub tools: &'a [(&'a str, &'a Path)],
}

/// The tools building a script's package, as the script is told them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptToolchain {
    /// `None` for a compiler the build has none of.
    pub cc: Option<PathBuf>,
    pub cxx: Option<PathBuf>,
    pub ar: PathBuf,
    /// The Apple SDK Cabin names to the compilers, when it names one:
    /// the script's `SDKROOT`, whatever Cabin's own is.
    pub sdkroot: Option<PathBuf>,
}

/// Two names a build script could not tell apart.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BuildScriptEnvError {
    #[error(
        "features `{first}` and `{second}` both reach the build script as `{variable}`; rename one of them"
    )]
    FeaturesCollide {
        first: String,
        second: String,
        variable: String,
    },
    #[error(
        "the build-dependency executables `{first}` and `{second}` both reach the build script as `{variable}`; name the one it runs in `targets` on its build-dependency entry"
    )]
    ToolsCollide {
        first: String,
        second: String,
        variable: String,
    },
}

/// The whole environment of a build script, ordered by variable name:
/// what `inherited` (Cabin's own environment) may pass on, then what
/// Cabin sets.
///
/// # Errors
/// Returns a [`BuildScriptEnvError`] when two declared features, or two
/// executables, would share a variable.
pub fn build_script_env(
    inputs: &BuildScriptEnvInputs<'_>,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<BTreeMap<String, OsString>, BuildScriptEnvError> {
    let mut env = BTreeMap::new();
    for (name, value) in inherited {
        if let Ok(name) = name.into_string()
            && passes_through(&name)
        {
            env.insert(name, value);
        }
    }
    let path = |path: &Path| path.as_os_str().to_owned();
    for (name, value) in [
        (CABIN_OUT_DIR, path(inputs.out_dir)),
        (CABIN_RESULT_FILE, path(inputs.result_file)),
        (CABIN_MANIFEST_DIR, path(inputs.manifest_dir)),
        (CABIN_PACKAGE_NAME, inputs.package_name.into()),
        (CABIN_PACKAGE_VERSION, inputs.package_version.into()),
        (CABIN_PROFILE, inputs.profile.into()),
        (CABIN_TARGET_OS, inputs.target_os.into()),
        (CABIN_TARGET_ARCH, inputs.target_arch.into()),
        (CABIN_TARGET_FAMILY, inputs.target_family.into()),
        (CABIN_TARGET_ENV, inputs.target_env.into()),
        (
            CABIN_TARGET_DIALECT,
            if inputs.msvc { "msvc" } else { "gnu" }.into(),
        ),
        ("HOME", path(inputs.home)),
        ("USERPROFILE", path(inputs.home)),
        ("TMPDIR", path(inputs.tmp)),
        ("TMP", path(inputs.tmp)),
        ("TEMP", path(inputs.tmp)),
    ] {
        env.insert(name.to_owned(), value);
    }
    let toolchain = inputs.toolchain;
    for (name, tool) in [
        (CABIN_CC, toolchain.cc.as_ref()),
        (CABIN_CXX, toolchain.cxx.as_ref()),
        (CABIN_AR, Some(&toolchain.ar)),
        ("SDKROOT", toolchain.sdkroot.as_ref()),
    ] {
        if let Some(tool) = tool {
            env.insert(name.to_owned(), path(tool));
        }
    }

    // A feature the build leaves off still claims its variable: the
    // script reads an unset one as that feature being off.
    let mut features: BTreeMap<String, &str> = BTreeMap::new();
    for (feature, enabled) in inputs.features {
        let variable = variable(CABIN_FEATURE_PREFIX, feature);
        if let Some(first) = features.insert(variable.clone(), feature) {
            return Err(BuildScriptEnvError::FeaturesCollide {
                first: first.to_owned(),
                second: (*feature).to_owned(),
                variable,
            });
        }
        if *enabled {
            env.insert(variable, "1".into());
        }
    }
    let mut tools: BTreeMap<String, &Path> = BTreeMap::new();
    for (tool, executable) in inputs.tools {
        let variable = variable(CABIN_BIN_PREFIX, tool);
        if let Some(first) = tools.insert(variable.clone(), executable) {
            return Err(BuildScriptEnvError::ToolsCollide {
                first: first.display().to_string(),
                second: executable.display().to_string(),
                variable,
            });
        }
        env.insert(variable, path(executable));
    }
    Ok(env)
}

/// Whether a variable of Cabin's own environment reaches a build script.
fn passes_through(name: &str) -> bool {
    PASSED_THROUGH.iter().any(|passed| same(name, passed)) || names_sdk(name)
}

/// Whether `name` is one of the variables a user names an SDK's
/// directory to build scripts with: one starting with
/// [`CABIN_SDK_PREFIX`].
#[must_use]
pub fn names_sdk(name: &str) -> bool {
    name.get(..CABIN_SDK_PREFIX.len())
        .is_some_and(|prefix| same(prefix, CABIN_SDK_PREFIX))
}

/// Whether two variable names are one, as the platform has it.
fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn variable(prefix: &str, name: &str) -> String {
    let name = name.chars().map(|c| {
        if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase()
        } else {
            '_'
        }
    });
    prefix.chars().chain(name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolchain() -> &'static BuildScriptToolchain {
        static TOOLCHAIN: std::sync::LazyLock<BuildScriptToolchain> =
            std::sync::LazyLock::new(|| BuildScriptToolchain {
                cc: Some("/abs/bin/cc".into()),
                cxx: Some("/abs/bin/c++".into()),
                ar: "/abs/bin/ar".into(),
                sdkroot: None,
            });
        &TOOLCHAIN
    }

    fn inputs<'a>(
        features: &'a [(&'a str, bool)],
        tools: &'a [(&'a str, &'a Path)],
    ) -> BuildScriptEnvInputs<'a> {
        BuildScriptEnvInputs {
            out_dir: Path::new("/abs/run/out"),
            result_file: Path::new("/abs/run/result/result"),
            home: Path::new("/abs/run/home"),
            tmp: Path::new("/abs/run/tmp"),
            manifest_dir: Path::new("/abs/app"),
            package_name: "app",
            package_version: "1.2.3",
            profile: "dev",
            target_os: "linux",
            target_arch: "x86_64",
            target_family: "unix",
            target_env: "",
            msvc: false,
            toolchain: toolchain(),
            features,
            tools,
        }
    }

    #[test]
    fn a_build_script_sees_only_what_cabin_passes_and_sets() {
        let inherited = [
            ("PATH", "/usr/bin"),
            ("CABIN_SDK_ZLIB", "/opt/zlib"),
            ("HOME", "/home/user"),
            ("CABIN_REGISTRY_TOKEN", "secret"),
            ("GITHUB_TOKEN", "secret"),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "secret"),
            ("SSH_AUTH_SOCK", "/run/agent"),
            ("CABIN_BUILD_DIR", "/elsewhere"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let features = [("fast-math", true), ("tls", false)];
        let generator = Path::new("/abs/tools/gen-table.v2");
        let tools = [("gen-table.v2", generator)];
        let env = build_script_env(&inputs(&features, &tools), inherited).unwrap();
        let expected = [
            ("CABIN_AR", "/abs/bin/ar"),
            ("CABIN_BIN_GEN_TABLE_V2", "/abs/tools/gen-table.v2"),
            ("CABIN_CC", "/abs/bin/cc"),
            ("CABIN_CXX", "/abs/bin/c++"),
            ("CABIN_FEATURE_FAST_MATH", "1"),
            ("CABIN_MANIFEST_DIR", "/abs/app"),
            ("CABIN_OUT_DIR", "/abs/run/out"),
            ("CABIN_PACKAGE_NAME", "app"),
            ("CABIN_PACKAGE_VERSION", "1.2.3"),
            ("CABIN_PROFILE", "dev"),
            ("CABIN_RESULT_FILE", "/abs/run/result/result"),
            ("CABIN_SDK_ZLIB", "/opt/zlib"),
            ("CABIN_TARGET_ARCH", "x86_64"),
            ("CABIN_TARGET_DIALECT", "gnu"),
            ("CABIN_TARGET_ENV", ""),
            ("CABIN_TARGET_FAMILY", "unix"),
            ("CABIN_TARGET_OS", "linux"),
            ("HOME", "/abs/run/home"),
            ("PATH", "/usr/bin"),
            ("TEMP", "/abs/run/tmp"),
            ("TMP", "/abs/run/tmp"),
            ("TMPDIR", "/abs/run/tmp"),
            ("USERPROFILE", "/abs/run/home"),
        ]
        .map(|(name, value)| (name.to_owned(), OsString::from(value)));
        assert_eq!(env.into_iter().collect::<Vec<_>>(), expected);
    }

    /// The SDK Cabin builds against is the script's, over the user's,
    /// and a compiler the build has none of is not named.
    #[test]
    fn the_sdk_of_the_build_replaces_an_inherited_sdkroot() {
        let inherited = || [("SDKROOT".into(), "/sdks/mine".into())];
        let env = build_script_env(&inputs(&[], &[]), inherited()).unwrap();
        assert_eq!(env["SDKROOT"], "/sdks/mine");
        let toolchain = BuildScriptToolchain {
            cxx: None,
            sdkroot: Some("/sdks/chosen".into()),
            ..toolchain().clone()
        };
        let inputs = BuildScriptEnvInputs {
            toolchain: &toolchain,
            ..inputs(&[], &[])
        };
        let env = build_script_env(&inputs, inherited()).unwrap();
        assert_eq!(env["SDKROOT"], "/sdks/chosen");
        assert!(!env.contains_key("CABIN_CXX"));
    }

    /// Windows spells `PATH` as `Path`, and matches names in any case.
    #[test]
    fn windows_passes_a_variable_through_in_any_case() {
        let inherited = [(OsString::from("Path"), OsString::from("C:\\bin"))];
        let env = build_script_env(&inputs(&[], &[]), inherited).unwrap();
        assert_eq!(env.contains_key("Path"), cfg!(windows));
    }

    #[test]
    fn names_a_build_script_could_not_tell_apart_are_refused() {
        // Whether or not the build enables either.
        let features = [("fast-math", false), ("fast_math", false)];
        assert_eq!(
            build_script_env(&inputs(&features, &[]), []),
            Err(BuildScriptEnvError::FeaturesCollide {
                first: "fast-math".to_owned(),
                second: "fast_math".to_owned(),
                variable: "CABIN_FEATURE_FAST_MATH".to_owned(),
            })
        );
        let (one, two) = (Path::new("/abs/one/gen"), Path::new("/abs/two/gen"));
        let tools = [("gen", one), ("gen", two)];
        assert_eq!(
            build_script_env(&inputs(&[], &tools), []),
            Err(BuildScriptEnvError::ToolsCollide {
                first: "/abs/one/gen".to_owned(),
                second: "/abs/two/gen".to_owned(),
                variable: "CABIN_BIN_GEN".to_owned(),
            })
        );
    }
}
