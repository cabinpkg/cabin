use std::io;
use std::path::PathBuf;

use cabin_manifest::ManifestError;
use miette::Diagnostic;
use thiserror::Error;

/// Errors produced while loading a workspace, a single package, or its
/// transitive local path dependencies.
#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceError {
    /// No `cabin.toml` was found at the requested path. Distinct
    /// from [`WorkspaceError::Io`] so the diagnostic layer can
    /// emit a single, deduplicated `manifest_not_found` report
    /// with help text instead of leaking the underlying
    /// `io::ErrorKind::NotFound` chain.
    #[error("could not find a Cabin workspace at {path}", path = path.display())]
    #[diagnostic(
        code(cabin::workspace::manifest_not_found),
        help(
            "run `cabin init` in the current directory to create a new package, or pass `--manifest-path <path>` to point at an existing `cabin.toml`"
        )
    )]
    ManifestNotFound { path: PathBuf },

    /// The manifest exists but Cabin could not read it. Captures
    /// permission denied, `IsADirectory`, and similar failures —
    /// anything except plain `NotFound`, which uses
    /// [`WorkspaceError::ManifestNotFound`].
    #[error("could not read the Cabin manifest at {path}: {source}", path = path.display())]
    #[diagnostic(code(cabin::workspace::manifest_unreadable))]
    ManifestUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to read {path}: {source}", path = path.display())]
    #[diagnostic(code(cabin::workspace::load_failed))]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to load manifest at {path}: {source}", path = path.display())]
    #[diagnostic(code(cabin::workspace::load_failed))]
    Manifest {
        path: PathBuf,
        #[source]
        source: Box<ManifestError>,
    },

    #[error("manifest at {path} contains neither [package] nor [workspace]", path = path.display())]
    EmptyManifest { path: PathBuf },

    #[error(
        "local dependency {dep_name:?} expects a cabin.toml at {expected}, but no such file exists",
        expected = expected.display()
    )]
    LocalDependencyManifestMissing { dep_name: String, expected: PathBuf },

    #[error(
        "local dependency {dep_name:?} resolves to a workspace root at {path}, but path dependencies must point at a single package",
        path = path.display()
    )]
    LocalDependencyIsWorkspace { dep_name: String, path: PathBuf },

    #[error(
        "dependency {dep_name:?} points to package {actual_name:?} at {path}; local dependency aliases are not supported",
        path = path.display()
    )]
    DependencyNameMismatch {
        dep_name: String,
        actual_name: String,
        path: PathBuf,
    },

    #[error("duplicate package name {name:?} in workspace (manifests: {first} and {second})",
        first = first.display(), second = second.display())]
    DuplicatePackageName {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("package dependency cycle detected: {}", format_cycle(.0))]
    PackageDependencyCycle(Vec<String>),

    #[error(
        "workspace member pattern {pattern:?} does not match any directory containing a cabin.toml under {root}",
        root = root.display()
    )]
    WorkspaceMemberMissing { pattern: String, root: PathBuf },

    #[error(
        "workspace member pattern {pattern:?} is not supported; only exact paths and a single trailing '*' (for example: 'packages/*') are supported"
    )]
    UnsupportedWorkspacePattern { pattern: String },

    #[error(
        "{field} entry {pattern:?} must be relative to the workspace root; absolute paths and `..` components are rejected"
    )]
    WorkspacePatternEscapesRoot {
        field: &'static str,
        pattern: String,
    },

    #[error(
        "registry dependency {dep_name:?} declared by package {parent:?} is not in the resolved set"
    )]
    UnresolvedRegistryDependency { dep_name: String, parent: String },

    #[error(
        "foundation-port dependency {dep_name:?} declared by package {parent:?} has not been prepared; this is an internal invariant violation — the CLI orchestration layer must call `cabin_port::prepare` before the workspace loader runs"
    )]
    PortDependencyNotPrepared {
        dep_name: String,
        parent: String,
        port_dir: PathBuf,
    },

    #[error(
        "bundled foundation-port dependency {dep_name:?} declared by package {parent:?} has not been prepared; this is an internal invariant violation — the CLI orchestration layer must call `cabin_port::prepare` before the workspace loader runs"
    )]
    BuiltinPortDependencyNotPrepared { dep_name: String, parent: String },

    #[error(
        "foundation-port directory {} declared by package {parent:?} does not exist",
        port_dir.display()
    )]
    PortDirectoryMissing {
        dep_name: String,
        parent: String,
        port_dir: PathBuf,
    },

    #[error(
        "registry package source {path} is named {actual_name:?} {actual_version}, but the resolver expected {name:?} {version}",
        path = path.display()
    )]
    RegistryPackageMismatch {
        name: String,
        version: String,
        actual_name: String,
        actual_version: String,
        path: PathBuf,
    },

    #[error(
        "dependency `{dep_name}` uses workspace = true under {section} in package `{parent}`, but {workspace_section} does not define `{dep_name}`",
        section = kind.manifest_section(),
        workspace_section = workspace_section_for(*kind),
    )]
    UnresolvedWorkspaceDependency {
        dep_name: String,
        parent: String,
        kind: cabin_core::DependencyKind,
    },

    #[error("workspace default member `{member}` is not listed in workspace.members")]
    DefaultMemberNotInMembers { member: String },

    #[error(
        "workspace exclude pattern {pattern:?} does not match any directory under {root}",
        root = root.display()
    )]
    UnusedExcludePattern { pattern: String, root: PathBuf },

    #[error(
        "nested workspace at {path}: a workspace member must not declare its own [workspace] table",
        path = path.display()
    )]
    NestedWorkspace { path: PathBuf },

    #[error(
        "workspace dependency `{name}` declared under [workspace.dependencies] is not a valid version requirement: {source}"
    )]
    InvalidWorkspaceDependency {
        name: String,
        #[source]
        source: Box<cabin_manifest::ManifestError>,
    },

    #[error(
        "package `{name}` is not a member of this workspace; available members: {}",
        members.join(", ")
    )]
    PackageNotInWorkspace { name: String, members: Vec<String> },

    #[error("--exclude requires --workspace or --default-members")]
    ExcludeWithoutWorkspaceSelection,

    #[error("--default-members requires a workspace root")]
    DefaultMembersWithoutWorkspace,

    #[error("package selection is ambiguous in this workspace; pass --package <name>")]
    AmbiguousPackageSelection,

    #[error("incompatible workspace requirements for `{name}`: {requirements}: {source}")]
    IncompatibleWorkspaceRequirements {
        name: String,
        requirements: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "registry package source {path} declares package `{actual_name}`, but the resolver expected `{name}`",
        path = path.display()
    )]
    RegistryPackageNameMismatch {
        name: String,
        actual_name: String,
        path: PathBuf,
    },

    #[error(
        "package `{name}` is not path-safe for registry publishing; package names cannot contain `/`, `\\`, `..`, or path-prefix-like forms"
    )]
    UnsafeRegistryPackageName { name: String },

    #[error(
        "nested workspace at {nested} cannot be the entry point because it is already a member of the workspace at {parent}",
        nested = nested.display(),
        parent = parent.display()
    )]
    NestedWorkspaceFromInside { nested: PathBuf, parent: PathBuf },

    #[error(
        "nested workspace detected: nearest workspace is {nearest} but outer workspace is {outer}",
        nearest = nearest.display(),
        outer = outer.display()
    )]
    NestedWorkspaceDiscovery { nearest: PathBuf, outer: PathBuf },

    #[error(
        "package `{package}` at {path} declares `[profile.*]` tables, but profile tables may only appear in the workspace root manifest",
        path = path.display()
    )]
    MemberDeclaresProfiles { package: String, path: PathBuf },

    #[error(
        "package `{package}` at {path} declares a `[toolchain]` table, but toolchain selection may only appear in the workspace root manifest",
        path = path.display()
    )]
    MemberDeclaresToolchain { package: String, path: PathBuf },

    #[error(
        "package `{package}` at {path} declares a `[profile.cache]` or `[target.'cfg(...)'.profile.cache]` table, but compiler-cache wrapper settings may only appear in the workspace root manifest",
        path = path.display()
    )]
    MemberDeclaresCompilerWrapper { package: String, path: PathBuf },

    #[error(
        "package `{package}` at {path} declares a `[patch]` table, but patch declarations may only appear in the workspace root manifest",
        path = path.display()
    )]
    MemberDeclaresPatches { package: String, path: PathBuf },

    #[error(
        "registry package `{package}` at {path} declares a `path` dependency on `{dep_name}`, but a downloaded registry package may only depend on other packages by version",
        path = path.display()
    )]
    RegistryPackageDeclaresPathDependency {
        package: String,
        dep_name: String,
        path: PathBuf,
    },

    #[error(
        "registry package `{package}` at {path} declares a port dependency on `{dep_name}`, but a downloaded registry package may only depend on other packages by version",
        path = path.display()
    )]
    RegistryPackageDeclaresPortDependency {
        package: String,
        dep_name: String,
        path: PathBuf,
    },

    #[error(
        "patch for package `{package}` collides with a registry entry at {path}; remove the duplicate registry source or the patch declaration",
        path = path.display()
    )]
    PatchConflictsWithRegistry { package: String, path: PathBuf },
}

fn format_cycle(cycle: &[String]) -> String {
    cycle.join(" -> ")
}

fn workspace_section_for(kind: cabin_core::DependencyKind) -> &'static str {
    use cabin_core::DependencyKind::{Dev, Normal};
    match kind {
        Normal => "[workspace.dependencies]",
        Dev => "[workspace.dev-dependencies]",
    }
}
