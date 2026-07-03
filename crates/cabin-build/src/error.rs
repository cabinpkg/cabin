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

    /// A target's `deps` entry names a package dependency by its
    /// bare name, but the dependency declares no library or
    /// header-only target with that name.  A bare name is shorthand
    /// for a same-named linkable target (`foo` means `foo:foo`);
    /// packages never export a *default* target, so anything else -
    /// including a same-named executable - must be spelled
    /// `package:target`.
    #[error(
        "`deps` entry {dep:?} matches package dependency {package:?}, but that package declares no library or header-only target named {dep:?}; a bare name is shorthand for a same-named linkable target (`{dep}:{dep}`), not a default library - use a local target name or an explicit `package:target`{}",
        format_target_suggestions(.package, .candidates)
    )]
    NoSameNameTargetInDependency {
        dep: String,
        package: String,
        /// Qualified `package:target` spellings of the dependency
        /// package's library and header-only targets.
        candidates: Vec<String>,
    },

    /// A target's `deps` entry names a package that the owning
    /// manifest only declares under `[dev-dependencies]`, but no
    /// active dev edge backs it: either the referencing target is
    /// not a dev-only kind (`test` / `example`), or the invocation
    /// did not activate dev-deps for the owning package (`cabin
    /// test` activates them for the selected packages only).
    #[error(
        "dependency {dep:?} is declared under `[dev-dependencies]` of package {package:?} but is not active here; dev dependencies link only into `test` / `example` targets, and only `cabin test` activates them for the selected packages"
    )]
    DevDependencyNotActive { dep: String, package: String },

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
    /// backend emits.  The wrapped error carries the specific
    /// missing capability or unsupported compiler family.
    #[error(transparent)]
    UnsupportedToolchain(#[from] cabin_core::ToolDetectionError),

    /// The selected tools individually run, but belong to
    /// different command-line dialects (MSVC `cl.exe` / `lib.exe`
    /// vs.  GCC/Clang).  Cabin emits one dialect per build, so a
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
    /// available.  Set `CC`, pass `--cc`, or add `cc = ...` to
    /// `[toolchain]` so Cabin can compile C translation units.
    #[error(
        "target {target:?} has C source `{path}` but no C compiler is available; set the `CC` environment variable, pass `--cc <path>`, or add `cc = ...` under [toolchain]"
    )]
    MissingCCompiler { target: String, path: Utf8PathBuf },

    /// A planned compile has no effective language standard: the
    /// target compiles sources of a language neither its
    /// `[target.<name>]` table nor its `[package]` declares a
    /// standard for.  Manifest loading rejects this before planning,
    /// so this fires only for packages constructed outside the
    /// manifest parser.
    #[error(
        "target `{target}` compiles {language} sources but no {language} standard is declared; add `{field} = \"<standard>\"` to its `[package]` or `[target.<name>]` table, or opt into a workspace default with `{field} = {{ workspace = true }}`"
    )]
    MissingLanguageStandard {
        target: String,
        language: &'static str,
        field: &'static str,
    },

    /// A consuming target's effective implementation standard is
    /// below a reachable library-like dependency's interface
    /// requirement for the same language.  The planner records the
    /// incompatibility on the consumer's compile;
    /// `validate_planned_standards` surfaces the first survivor
    /// after the `cabin check` rewrite has pruned dependency
    /// compiles.
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
    /// standard `cl.exe` has no stable `/std:` flag for.  The planner
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

    /// A planned compile carries both a first-class standard
    /// declaration and an explicit `-std=` / `/std:` token in its
    /// manifest-derived flag list.  Boxed to keep the enum small;
    /// `#[source]` keeps the typed conflict reachable on the error
    /// chain so the diagnostic registry can attach its stable code.
    #[error("the manifest declares conflicting standard selections")]
    StandardFlagConflict(#[source] Box<cabin_core::StandardFlagConflict>),
}

fn format_cycle(cycle: &[String]) -> String {
    cycle.join(" -> ")
}

fn format_candidates(candidates: &[String]) -> String {
    candidates.join(", ")
}

fn format_target_suggestions(package: &str, candidates: &[String]) -> String {
    if candidates.is_empty() {
        format!(" (package {package:?} declares no library or header-only targets)")
    } else {
        format!(", e.g. `{}`", candidates.join("` or `"))
    }
}
