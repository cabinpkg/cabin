use std::path::PathBuf;

use camino::Utf8PathBuf;
use thiserror::Error;

/// Errors produced while planning a build graph.
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("target dependency cycle detected: {}", format_cycle(.0))]
    DependencyCycle(Vec<String>),

    #[error("no target named {0:?} is defined in the package graph")]
    UnknownTargetReference(String),

    #[error("target {:?} is ambiguous; use `package:target` (candidates: {})", .0, format_candidates(.1))]
    AmbiguousTarget(String, Vec<String>),

    #[error("unknown package {package:?} in target selector {selector:?}")]
    UnknownPackageInTargetSelector { package: String, selector: String },

    #[error("package {package:?} has no target {target:?}")]
    UnknownTargetInPackage { package: String, target: String },

    #[error(
        "dependency {dep:?} resolves to package {package:?} which has no library or header-only target; use `{package}:<target>` to pick a specific target"
    )]
    DependencyHasNoLibrary { dep: String, package: String },

    #[error(
        "dependency {dep:?} resolves to package {package:?} which has multiple library or header-only targets; disambiguate with `{package}:<target>`"
    )]
    AmbiguousDefaultLibrary { dep: String, package: String },

    #[error("target {0:?} has no source files; nothing to build")]
    EmptyTargetSources(String),

    #[error("source path {path} for target {target:?} is not supported: {reason}")]
    InvalidSourcePath {
        target: String,
        path: Utf8PathBuf,
        reason: String,
    },

    #[error("path {} is not valid UTF-8 and cannot be used in build commands", .0.display())]
    NonUtf8Path(PathBuf),

    #[error(
        "selected workspace packages declare no C/C++ targets to build; pick a package with at least one library or executable"
    )]
    EmptySelectedPackages,

    /// The detected toolchain cannot run the commands the C++
    /// backend emits. The wrapped error carries the specific
    /// missing capability or unsupported compiler family.
    #[error(transparent)]
    UnsupportedToolchain(#[from] cabin_core::ToolDetectionError),

    /// The selected tools individually run, but belong to
    /// different command-line dialects (MSVC `cl.exe` / `lib.exe`
    /// vs. GCC/Clang). Cabin emits one dialect per build, so a
    /// mixed toolchain cannot be driven coherently.
    #[error(
        "selected toolchain mixes MSVC and GCC/Clang tools, which Cabin cannot drive together: {detail}"
    )]
    MixedToolchainDialects { detail: String },

    /// A target carries a source whose extension does not match
    /// any of Cabin's recognized C/C++ extensions.
    #[error(
        "target {target:?} has source `{path}` with an unrecognized extension; supported extensions are .c (C) and .cc / .cpp / .cxx / .c++ / .C (C++)"
    )]
    UnrecognizedSourceExtension { target: String, path: Utf8PathBuf },

    /// A target carries `.c` source(s) but no C compiler is
    /// available. Set `CC`, pass `--cc`, or add `cc = ...` to
    /// `[toolchain]` so Cabin can compile C translation units.
    #[error(
        "target {target:?} has C source `{path}` but no C compiler is available; set the `CC` environment variable, pass `--cc <path>`, or add `cc = ...` under [toolchain]"
    )]
    MissingCCompiler { target: String, path: Utf8PathBuf },

    /// A consuming target's effective implementation standard is
    /// below a reachable library-like dependency's interface
    /// requirement for the same language.
    #[error(
        "target `{consumer}` compiles {language} as `{consumer_standard}`, but its dependency `{dependency}` requires `{required}` for consumers of its public interface (from {requirement_source}); raise `{consumer}`'s {language} standard to at least `{required}`, or lower the dependency's interface standard if its public headers permit"
    )]
    IncompatibleLanguageStandard {
        consumer: String,
        dependency: String,
        language: &'static str,
        consumer_standard: &'static str,
        required: &'static str,
        requirement_source: &'static str,
    },

    /// A compile that survived into the final build graph requests a
    /// standard `cl.exe` has no stable `/std:` flag for. The planner
    /// records such compiles as violations (it cannot lower them);
    /// `validate_planned_standards` surfaces the first survivor after
    /// the `cabin check` rewrite has pruned dependency compiles.
    #[error(
        "target `{target}` requests {language} standard `{standard}`, which has no stable MSVC `/std:` flag; use a standard cl.exe supports (c11, c17, c++14, c++17, c++20) or build with a GCC/Clang toolchain"
    )]
    StandardUnsupportedOnMsvcDialect {
        target: String,
        language: &'static str,
        standard: &'static str,
    },
}

fn format_cycle(cycle: &[String]) -> String {
    cycle.join(" -> ")
}

fn format_candidates(candidates: &[String]) -> String {
    candidates.join(", ")
}
