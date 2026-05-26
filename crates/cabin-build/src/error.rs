use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while planning a build graph.
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("target dependency cycle detected: {}", format_cycle(.0))]
    DependencyCycle(Vec<String>),

    #[error("no target named {0:?} is defined in the package graph")]
    UnknownTargetReference(String),

    #[error("target {0:?} is ambiguous; use `package:target` (candidates: {})", format_candidates(.1))]
    AmbiguousTarget(String, Vec<String>),

    #[error("unknown package {package:?} in target selector {selector:?}")]
    UnknownPackageInTargetSelector { package: String, selector: String },

    #[error("package {package:?} has no target {target:?}")]
    UnknownTargetInPackage { package: String, target: String },

    #[error(
        "dependency {dep:?} resolves to package {package:?} which has no cpp_library target; use `{package}:<target>` to pick a specific target"
    )]
    DependencyHasNoLibrary { dep: String, package: String },

    #[error(
        "dependency {dep:?} resolves to package {package:?} which has multiple cpp_library targets; disambiguate with `{package}:<target>`"
    )]
    AmbiguousDefaultLibrary { dep: String, package: String },

    #[error("target {0:?} has no source files; nothing to build")]
    EmptyTargetSources(String),

    #[error("source path {} for target {target:?} is not supported: {reason}", path.display())]
    InvalidSourcePath {
        target: String,
        path: PathBuf,
        reason: String,
    },

    #[error("path {} is not valid UTF-8 and cannot be used in build commands", .0.display())]
    NonUtf8Path(PathBuf),

    #[error(
        "selected workspace packages declare no C/C++ targets to build; pick a package with at least one cpp_library or cpp_executable"
    )]
    EmptySelectedPackages,

    /// The detected toolchain cannot run the commands the C++
    /// backend emits. The wrapped error carries the specific
    /// missing capability or unsupported compiler family.
    #[error(transparent)]
    UnsupportedToolchain(#[from] cabin_core::ToolDetectionError),

    /// A target carries a source whose extension does not match
    /// any of Cabin's recognized C / C++ extensions.
    #[error(
        "target {target:?} has source `{}` with an unrecognized extension; supported extensions are .c (C) and .cc / .cpp / .cxx / .c++ / .C (C++)",
        path.display()
    )]
    UnrecognizedSourceExtension { target: String, path: PathBuf },

    /// A target carries `.c` source(s) but no C compiler is
    /// available. Set `CC`, pass `--cc`, or add `cc = ...` to
    /// `[toolchain]` so Cabin can compile C translation units.
    #[error(
        "target {target:?} has C source `{}` but no C compiler is available; set the `CC` environment variable, pass `--cc <path>`, or add `cc = ...` under [toolchain]",
        path.display()
    )]
    MissingCCompiler { target: String, path: PathBuf },
}

fn format_cycle(cycle: &[String]) -> String {
    cycle.join(" -> ")
}

fn format_candidates(candidates: &[String]) -> String {
    candidates.join(", ")
}
