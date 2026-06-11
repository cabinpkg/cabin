use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use cabin_artifact::{ArtifactCache, FetchEntry, FetchOptions, FetchPlan, FetchedPackage};
use cabin_build::{PlanRequest, plan};
use cabin_core::PackageName;
use cabin_index::PackageIndex;
use cabin_lockfile::{LockedPackage, LockedSource, Lockfile};
use cabin_package::scaffold;
use cabin_resolver::{LockedVersion, ResolveInput, ResolveMode, ResolveOutput, ResolvedSource};
use cabin_workspace::{PackageGraph, RegistryPackageSource, collect_patched_versioned_deps};

use crate::completions::CompgenArgs;
use crate::manpages::MangenArgs;

pub(crate) mod add;
pub(crate) mod build_prep;
pub(crate) mod config;
pub(crate) mod env_flags;
pub(crate) mod explain;
pub(crate) mod fetch_output;
pub(crate) mod fmt;
pub(crate) mod metadata;
pub(crate) mod ninja;
pub(crate) mod patch;
pub(crate) mod port;
pub(crate) mod remove;
pub(crate) mod run;
pub(crate) mod source_tooling;
pub(crate) mod system_deps;
pub(crate) mod term_color;
pub(crate) mod term_verbosity;
pub(crate) mod test;
pub(crate) mod tidy;
pub(crate) mod tree;
pub(crate) mod vendor;
pub(crate) mod version;

mod build;
mod clean;
mod init;
mod manifest_edit;
mod package;
mod resolve;

use self::build::{BuildMode, build};
use self::clean::clean;
use self::init::{init, new};
use self::package::{package, publish};
use self::resolve::{fetch, resolve, update};

use crate::cli::fetch_output::emit_fetch_output;
use crate::cli::term_color::CliColorChoice;
use crate::cli::term_verbosity::Reporter;

/// Cargo-style color palette for clap's help / error
/// rendering.  Mirrors the ANSI sequences `cargo --help
/// --color always` emits today: bold + bright green for the
/// section headings and the `Usage:` line, bold + bright cyan
/// for literal tokens (the binary name, flag and subcommand
/// names), plain cyan for value placeholders such as
/// `<NAME>` / `[OPTIONS]`, bold + bright red for `error:`
/// labels, and bold + yellow for the highlighted-invalid
/// token inside diagnostic messages.
fn cli_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Color, Style};

    let header_usage = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::BrightGreen)));
    let literal = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::BrightCyan)));
    let placeholder = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
    let error = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::BrightRed)));
    let invalid = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
    let valid = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::BrightGreen)));

    clap::builder::Styles::styled()
        .usage(header_usage)
        .header(header_usage)
        .literal(literal)
        .placeholder(placeholder)
        .error(error)
        .invalid(invalid)
        .valid(valid)
}

/// Top-level help template — mirrors `cargo --help`:
///
/// - the `Options:` block comes before the `Commands:` block
///   so the short list of global flags is on screen first;
/// - the section headings (`Options:`, `Commands:`) carry the
///   same bold + bright-green styling clap applies to
///   `Usage:`.  The embedded ANSI escapes are stripped by
///   anstream when color is disabled (`--color never`,
///   `NO_COLOR`, or a non-TTY stdout).
///
/// `{options}` renders the options block body only.  The
/// subcommand block is omitted because the default `[aliases:
/// x]` rendering does not match cargo's `name, alias` style;
/// the dispatcher in `lib.rs::run` rebuilds the subcommand
/// rows manually and feeds them in via `after_help`.
const HELP_TEMPLATE: &str = concat!(
    "{about-with-newline}\n",
    "{usage-heading} {usage}\n",
    "\n",
    // Bold + bright green, like clap's auto `Usage:` style.
    "\x1b[1m\x1b[92mOptions:\x1b[0m\n",
    "{options}",
    "{after-help}",
);

/// Top-level Cabin CLI parser.
#[derive(Debug, Parser)]
#[command(
    name = "cabin",
    about = "A package manager and build system for C/C++",
    disable_version_flag = true,
    styles = cli_styles(),
    help_template = HELP_TEMPLATE,
    // Compact, cargo-style option rows: keep the description
    // inline with the flag name rather than dropping it to
    // its own line for every entry.
    next_line_help = false,
)]
pub struct Cli {
    /// Use verbose output (-vv very verbose output).
    //
    // `ArgAction::Count` collects repeated `-v` occurrences;
    // counts of two or more clamp to `Verbosity::VeryVerbose`.
    #[arg(
        short = 'v',
        long = "verbose",
        global = true,
        action = clap::ArgAction::Count,
        conflicts_with = "quiet",
        display_order = 1,
    )]
    pub(crate) verbose: u8,

    /// Do not print cabin log messages.
    #[arg(
        short = 'q',
        long = "quiet",
        global = true,
        conflicts_with = "verbose",
        display_order = 2
    )]
    pub(crate) quiet: bool,

    /// Coloring: auto, always, never [default: auto]
    //
    // Single-line rustdoc keeps `cabin --help` compact.  The
    // literal "[default: auto]" is part of the description
    // because clap does not render a `default_value` for
    // `Option<...>` enum flags.
    //
    // Precedence is `--color` > `CABIN_TERM_COLOR` >
    // `[term] color` config > `auto`; see
    // `docs/environment-variables.md` for the full table.
    #[arg(
        long,
        value_name = "WHEN",
        value_enum,
        global = true,
        hide_possible_values = true,
        display_order = 3
    )]
    pub(crate) color: Option<CliColorChoice>,

    /// List installed commands.
    //
    // The dispatcher short-circuits on this flag before
    // touching `cli.command`, so combining it with a
    // subcommand silently ignores the subcommand.  The flag
    // intentionally co-exists with global flags like
    // `--color` so `cabin --color always --list` renders the
    // listing with the requested color treatment.
    #[arg(long, display_order = 4)]
    pub(crate) list: bool,

    /// Print version info and exit.
    //
    // Replaces clap's auto `--version` so the flag can route
    // through `cabin version`'s dispatcher: `cabin --version`
    // prints the concise line and `cabin --version --verbose`
    // prints the same key/value block `cabin version -v`
    // emits.  Display order keeps the `-h, -V` pair adjacent.
    #[arg(
        short = 'V',
        long = "version",
        global = true,
        action = clap::ArgAction::SetTrue,
        display_order = 6,
    )]
    pub(crate) version: bool,

    // The subcommand is `Option<...>` so `cabin --list` and
    // `cabin --version` keep working without one.  The
    // dispatcher prints the curated help and exits cleanly when
    // both `--list` is unset and `command` is `None`.
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

// `cabin --help` is the curated, day-to-day surface and
// closely mirrors `cargo --help`.  Subcommands tagged
// `#[command(hide = true)]` below stay fully functional but
// surface only through `cabin --list`, `cabin <sub> --help`,
// shell completions, and per-subcommand man pages.
//
// Curation pattern (matching cargo --help):
// - hide inspection-only commands (`metadata`, `tree`,
//   `explain`) — useful for scripts / CI, rarely typed
//   day-to-day;
// - hide low-level / scripting commands (`resolve`) —
//   `cabin metadata` and `cabin update` are the user-facing
//   paths;
// - hide offline / networking helpers (`fetch`, `vendor`) —
//   triggered automatically when needed;
// - hide pre-publish packaging (`package`) — `publish` is
//   the user-facing entry;
// - hide distribution helpers (`compgen`, `mangen`) — aimed
//   at downstream packagers.
//
// `version` stays visible because it is a direct user-facing
// command; `cabin --version` and `cabin version`
// agree on the concise wording.
// Each subcommand's rustdoc has two paragraphs: the first is
// the short summary clap renders in `cabin --help` / `cabin
// --list`, and the rest becomes the long help shown by `cabin
// <sub> --help`.  The split keeps the top-level surface
// skimmable while preserving the existing detailed prose.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create a new cabin package in an existing directory.
    Init(InitArgs),
    /// Create a new cabin package.
    ///
    /// Scaffolds a new package at `<PATH>`.  The directory must
    /// not already exist.
    New(NewArgs),
    /// Add a dependency to a cabin.toml manifest file.
    ///
    /// Edits the manifest in place, preserving its existing
    /// formatting and comments. Use `--port <name>` to add a bundled
    /// foundation port or `--path <dir>` to add a local package;
    /// registry dependencies are not supported yet.
    Add(crate::cli::add::AddArgs),
    /// Remove a dependency from a cabin.toml manifest file.
    ///
    /// Deletes the named `[dependencies]` (or `[dev-dependencies]`,
    /// with `--dev`) entry, leaving the rest of the manifest intact.
    Remove(crate::cli::remove::RemoveArgs),
    /// Output workspace metadata as JSON.
    ///
    /// Prints the loaded workspace graph, selected build
    /// configuration view, and lockfile state (if any) in
    /// machine-readable form. Use this for tooling / scripts;
    /// the human-facing inspection commands are `cabin tree`
    /// and `cabin explain`.
    #[command(hide = true)]
    Metadata(ManifestArgs),
    /// Compile a local package and all of its dependencies.
    ///
    /// Plans the build, writes `build.ninja` plus a
    /// Clang-compatible `compile_commands.json`, and invokes
    /// Ninja.
    //
    // `visible_alias = "b"` matches cargo's `build, b`
    // rendering: clap auto-renders the alias next to the
    // canonical name in `cabin --help` / `cabin --list`, and
    // `cabin b` is parsed identically to `cabin build`.
    #[command(visible_alias = "b")]
    Build(BuildArgs),
    /// Check a local package and its dependencies for errors.
    ///
    /// Type-checks the workspace's C/C++ sources with the compiler's
    /// `-fsyntax-only` mode, reusing the same build graph as
    /// `cabin build` but skipping code generation, archiving, and
    /// linking. No object files or binaries are produced. Faster than
    /// a full build for catching compile errors.
    Check(BuildArgs),
    /// Remove the built directory.
    ///
    /// Deletes Cabin-generated build artifacts under the
    /// resolved `--build-dir`.  Source files are never
    /// touched.
    Clean(CleanArgs),
    /// Run a binary of the local package.
    ///
    /// Builds the selected `executable` target and executes
    /// it. Arguments after `--` are forwarded verbatim to the
    /// executed program.
    #[command(visible_alias = "r")]
    Run(crate::cli::run::RunArgs),
    /// Run the tests of a local package.
    ///
    /// Builds the workspace's `test` targets and executes
    /// each one with a deterministic per-test `CABIN_*`
    /// environment overlay.
    #[command(visible_alias = "t")]
    Test(crate::cli::test::TestArgs),
    /// Resolve versioned dependencies.
    ///
    /// Resolves the manifest's versioned dependencies against
    /// a local JSON package index and prints the result.
    /// Most users prefer `cabin metadata` or `cabin update`.
    #[command(hide = true)]
    Resolve(ResolveArgs),
    /// Update dependencies as recorded in `cabin.lock`.
    Update(UpdateArgs),
    /// Fetch registry dependencies into the artifact cache.
    ///
    /// Fetches, verifies, and extracts the source archives of
    /// resolved registry dependencies. Triggered
    /// automatically by `cabin build`, `cabin run`, and
    /// `cabin test`; use this command to warm the cache.
    #[command(hide = true)]
    Fetch(FetchArgs),
    /// Vendor external versioned dependencies locally.
    ///
    /// Materializes the selected external registry dependency
    /// closure into a deterministic local file-registry directory
    /// for offline use. Local path dependencies stay local.
    /// Combine with `--offline --index-path <vendor-dir>` on
    /// subsequent commands.
    #[command(hide = true)]
    Vendor(crate::cli::vendor::VendorArgs),
    /// Display the dependency tree.
    ///
    /// Renders the loaded workspace / local-path dependency
    /// graph as a tree (human or JSON). Workspace, feature,
    /// kind-filter, and patch flags affect this view; option and
    /// variant selectors are build-configuration inputs and do
    /// not change the tree.
    #[command(hide = true)]
    Tree(crate::cli::tree::TreeArgs),
    /// Explain a loaded package, target, source, or feature.
    ///
    /// Package, target, source, and feature subcommands map to
    /// the typed explanation model in `cabin-explain`.
    /// `build-config` reuses the same resolved configuration
    /// shape as `cabin metadata`.
    #[command(hide = true)]
    Explain(crate::cli::explain::ExplainArgs),
    /// Assemble the local package into a distributable archive.
    ///
    /// Builds a deterministic source archive plus canonical
    /// metadata for the package at `--manifest-path`.
    /// Typically driven by `cabin publish`.
    #[command(hide = true)]
    Package(PackageArgs),
    /// Publish a package to a local file registry.
    ///
    /// With `--registry-dir <PATH>`, writes the archive plus
    /// canonical metadata into a Cabin file registry. With
    /// `--dry-run` alone, stages the same artifacts under
    /// `--output-dir` without touching any registry. Remote
    /// registry protocols are not supported.
    Publish(PublishArgs),
    /// Format codes using clang-format.
    ///
    /// Walks the workspace's C/C++ sources and rewrites
    /// them in place using the user's `clang-format`.
    Fmt(crate::cli::fmt::FmtArgs),
    /// Run clang-tidy.
    ///
    /// Drives `run-clang-tidy` over the workspace's C/C++
    /// sources using the generated `compile_commands.json`.
    Tidy(crate::cli::tidy::TidyArgs),
    /// List or inspect bundled foundation-port recipes.
    Port(crate::port_subcommand::PortArgs),
    /// Generate shell completion scripts for the `cabin` CLI.
    #[command(hide = true)]
    Compgen(CompgenArgs),
    /// Generate man pages for the `cabin` CLI.
    #[command(hide = true)]
    Mangen(MangenArgs),
    /// Show version information.
    ///
    /// Without flags, prints the concise release name (same
    /// wording as `cabin --version`). With `-v` /
    /// `--verbose`, prints a stable key/value block describing
    /// the build (`release`, `commit-hash`, `commit-date`,
    /// `host`, `os`); rows whose underlying value is unknown
    /// are omitted.
    Version(crate::cli::version::VersionArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Package name. Defaults to the current directory name.
    #[arg(long)]
    pub name: Option<String>,

    /// Use a binary (application) template [default].
    ///
    /// Conflicts with `--lib`.
    #[arg(short = 'b', long, group = "init_scaffold_kind")]
    pub bin: bool,

    /// Use a library template.
    ///
    /// Conflicts with `--bin`.
    #[arg(short = 'l', long, group = "init_scaffold_kind")]
    pub lib: bool,
}

#[derive(Debug, Args)]
pub(crate) struct NewArgs {
    /// Path of the new package directory. The directory must not already exist.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Package name. Defaults to the final component of `<PATH>`.
    #[arg(long)]
    pub name: Option<String>,

    /// Use a binary (application) template [default].
    ///
    /// Conflicts with `--lib`.
    #[arg(short = 'b', long, group = "new_scaffold_kind")]
    pub bin: bool,

    /// Use a library template.
    ///
    /// Conflicts with `--bin`.
    #[arg(short = 'l', long, group = "new_scaffold_kind")]
    pub lib: bool,
}

#[derive(Debug, Args)]
pub(crate) struct CleanArgs {
    /// Path to the cabin.toml manifest.  Same precedence rules
    /// as `cabin build`.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build output directory.  Same precedence rules as
    /// `cabin build`: `--build-dir` > `CABIN_BUILD_DIR` >
    /// `[paths] build-dir` config setting > built-in default
    /// `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Compatibility alias for `--profile release`.  Cannot be
    /// used together with `--profile`.
    #[arg(long, conflicts_with = "profile")]
    pub release: bool,

    /// Limit the clean to the named build profile.  Without this
    /// flag every known profile sub-tree is in scope.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Print the deletion plan without removing anything.  Output
    /// lists the paths that would be removed in deterministic
    /// order.
    #[arg(long)]
    pub dry_run: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ManifestArgs {
    /// Path to the cabin.toml manifest. May be a single-package manifest
    /// or a workspace root.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Feature selection flags. Empty by default. When any
    /// selection flag is passed, `cabin metadata --format json`
    /// adds a `configuration` block to each primary package
    /// describing the resolved configuration.
    #[command(flatten)]
    pub selection: ConfigSelectionArgs,

    /// Workspace package-selection flags. The metadata view
    /// always reports every loaded package; selection flags only
    /// narrow the `selected_packages` list.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Output format. `human` is a readable summary; `json`
    /// produces a machine-parseable document. Defaults to `json`
    /// for back-compat with scripts that pipe the metadata output
    /// into `jq`.
    #[arg(long, value_name = "FORMAT", default_value = "json")]
    pub format: ResolveFormat,

    /// Profile to evaluate for the metadata view. Defaults to
    /// `dev`. The view always lists every available profile in
    /// the `profiles.available` array regardless of which one is
    /// selected.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Toolchain-selection flags. Same precedence rules as
    /// `cabin build` so the metadata view reflects exactly the
    /// toolchain a build would use.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation. Manifest `[patch]` tables and
    /// config `[patch]` / `[source-replacement]` declarations
    /// are ignored; ordinary `path = "..."` dependency edges
    /// and dependency declarations stay active.
    #[arg(long)]
    pub no_patches: bool,

    /// Forbid network access. `cabin metadata` rejects an HTTP
    /// `--index-url` (or a `[registry] index-url` in the active
    /// config) when this flag is set so the metadata view stays
    /// fully local.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Debug, Args)]
pub(crate) struct BuildArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for build outputs (build.ninja, object files, binaries).
    /// Defaults to `build/`; a config-provided `paths.build-dir`
    /// overrides this default.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Build with optimizations.
    ///
    /// Use release flags (-O3 -DNDEBUG) instead of debug flags
    /// (-g -O0).  Compatibility alias for `--profile release`;
    /// cannot be used together with `--profile`.
    #[arg(short = 'r', long, conflicts_with = "profile")]
    pub release: bool,

    /// Select the build profile (`dev`, `release`, or any custom
    /// profile declared in `[profile.<name>]`). Defaults to `dev`.
    /// Mutually exclusive with `--release`.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Path to a directory containing the local JSON package index.
    /// Required when the manifest declares any versioned dependencies
    /// and `--index-url` is not given. Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`. Static sparse HTTP
    /// serving of the file-registry layout is supported
    /// (`<url>/config.json`, `<url>/packages/<name>.json`).
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`. Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side effects:
    /// The lockfile must not change and the artifact cache will not be
    /// populated. Already-cached artifacts may be reused.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access. Cabin refuses to use an HTTP index URL
    /// (`--index-url` or a `[registry] index-url` config setting) and
    /// expects every needed artifact to be available from a local
    /// index (`--index-path`) or already in the artifact cache.
    /// Combine with `cabin vendor` to consume a self-contained vendor
    /// directory.
    #[arg(long)]
    pub offline: bool,

    /// Enable named features. May be passed multiple times; values
    /// may also be comma-separated (`--features simd,ssl`). The
    /// selection applies to the root package being built.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every feature declared by the root package. Combines
    /// with `--features` (the union is the same as `--all-features`)
    /// and overrides `--no-default-features`.
    #[arg(long)]
    pub all_features: bool,

    /// Disable the package's default features. Without this flag, the
    /// names listed under `[features].default` are enabled.
    #[arg(long)]
    pub no_default_features: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Toolchain-selection flags. Each flag (when supplied)
    /// overrides any `CC`/`CXX`/`AR` environment variable and
    /// any `[toolchain]` table in the workspace root manifest.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation. See `docs/patch-overrides.md`.
    #[arg(long)]
    pub no_patches: bool,

    /// Number of parallel jobs to use for building.
    ///
    /// Precedence: this flag > `CABIN_BUILD_JOBS` env var >
    /// `[build] jobs` config setting > backend default.  The
    /// value must be a positive integer; `0` is rejected.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<cabin_core::BuildJobs>,
}

/// Toolchain-selection flag bundle shared by `cabin build` and
/// `cabin metadata`. Each flag accepts either a bare command name
/// (`clang++`, resolved against `PATH`) or an explicit path
/// (`/opt/llvm/bin/clang++`).
#[derive(Debug, Args, Default)]
pub(crate) struct ToolchainSelectionArgs {
    /// Override the C compiler. Accepts a bare command name or a
    /// path. Highest precedence — also overrides `CC` and
    /// `[toolchain].cc`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub cc: Option<String>,

    /// Override the C++ compiler. Accepts a bare command name or
    /// a path. Highest precedence — also overrides `CXX` and
    /// `[toolchain].cxx`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub cxx: Option<String>,

    /// Override the static-library archiver. Accepts a bare
    /// command name or a path. Highest precedence — also
    /// overrides `AR` and `[toolchain].ar`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub ar: Option<String>,

    /// Select a compiler-cache wrapper that prefixes every C++
    /// compile command. Accepts `none`, `ccache`, or `sccache`.
    /// Highest precedence — also overrides
    /// `CABIN_COMPILER_WRAPPER`, config `[build.cache]`, and
    /// any manifest `[profile.cache]` or
    /// `[target.'cfg(...)'.profile.cache]` declaration.
    /// Mutually exclusive with `--no-compiler-wrapper`.
    #[arg(long, value_name = "WRAPPER", conflicts_with = "no_compiler_wrapper")]
    pub compiler_wrapper: Option<String>,

    /// Disable the compiler-cache wrapper for this invocation,
    /// regardless of any environment variable or manifest
    /// declaration. Equivalent to `--compiler-wrapper none` but
    /// shorter to type. Mutually exclusive with
    /// `--compiler-wrapper`.
    #[arg(long)]
    pub no_compiler_wrapper: bool,
}

/// Selection-flag bundle shared by `cabin build` and `cabin metadata`.
#[derive(Debug, Args, Default)]
pub(crate) struct ConfigSelectionArgs {
    /// Enable named features. May be repeated and/or comma-separated.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every declared feature.
    #[arg(long)]
    pub all_features: bool,

    /// Disable default features.
    #[arg(long)]
    pub no_default_features: bool,
}

/// Workspace selection flags for `cabin update`.
///
/// `cabin update` reserves `--package <name>` for its
/// "refresh just this direct registry dep" semantic, so this
/// bundle deliberately omits `-p / --package`. Members can still
/// be scoped by `--workspace`, `--default-members`, and
/// `--exclude`. Adding a separate long flag (e.g.
/// `--scope-package`) for member-name selection is a deferred
/// improvement.
#[derive(Debug, Args, Default)]
pub(crate) struct WorkspaceSelectionArgsForUpdate {
    /// Operate on every workspace member, then apply `--exclude`.
    #[arg(long, conflicts_with = "default_members")]
    pub workspace: bool,

    /// Operate on `[workspace.default-members]`. Errors when the
    /// Workspace declares no default-members.
    #[arg(long, conflicts_with = "workspace")]
    pub default_members: bool,

    /// Drop the named package from the selection. Only valid in
    /// combination with `--workspace` or `--default-members`.
    #[arg(long, value_name = "PACKAGE")]
    pub exclude: Vec<String>,
}

/// Workspace package-selection flags shared across the commands
/// that operate on a (possibly multi-member) workspace.
///
/// Empty by default, in which case the documented "current
/// package" fallback applies (single-package builds keep working
/// unchanged; workspace builds use `[workspace.default-members]`
/// if declared, otherwise every member).
#[derive(Debug, Args, Default)]
pub(crate) struct WorkspaceSelectionArgs {
    /// Operate on every workspace member, then apply `--exclude`.
    /// Mutually exclusive with `--package` / `--default-members`.
    #[arg(
        long,
        conflicts_with_all = &["package", "default_members"],
    )]
    pub workspace: bool,

    /// Operate on the named workspace package. Repeat the flag to
    /// select multiple packages. Errors when a name is not a workspace
    /// member or appears twice in the workspace.
    #[arg(long = "package", short = 'p', value_name = "PACKAGE")]
    pub package: Vec<String>,

    /// Operate on `[workspace.default-members]`. Errors when the
    /// workspace declares no default-members.
    #[arg(long, conflicts_with_all = &["workspace", "package"])]
    pub default_members: bool,

    /// Drop the named package from the selection. Only valid in
    /// combination with `--workspace` or `--default-members`, or with
    /// the no-flag default-member fallback.
    #[arg(long, value_name = "PACKAGE")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FetchArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Path to a directory containing the local JSON package index.
    /// Required when the manifest declares any versioned dependencies
    /// and `--index-url` is not given. Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`. Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side effects.
    /// The lockfile is not written and the artifact cache will not be
    /// populated. Already-cached artifacts may be reused.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access. Cabin refuses to use an HTTP index
    /// URL (`--index-url` or a `[registry] index-url` config setting)
    /// and expects every needed input to be local or already cached.
    #[arg(long)]
    pub offline: bool,

    /// Output format. `human` is a readable summary; `json` produces a
    /// machine-parseable document. Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation. See `docs/patch-overrides.md`.
    #[arg(long)]
    pub no_patches: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PackageArgs {
    /// Path to the cabin.toml manifest. Must point at a single
    /// package; pure-workspace roots are rejected unless the
    /// Workspace selects exactly one member with `--package`.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the generated archive and metadata.
    #[arg(long, default_value = "dist")]
    pub output_dir: PathBuf,

    /// Output format. `human` is a readable summary; `json` produces
    /// A machine-parseable document. Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags. In a workspace with
    /// multiple members, `cabin package` requires a single
    /// `--package <name>` selection.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PublishArgs {
    /// Path to the cabin.toml manifest. Must point at a single
    /// package; pure-workspace roots are rejected.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the dry-run's archive and metadata when
    /// `--registry-dir` is not given. Defaults to `dist/`. Mutually
    /// exclusive with `--registry-dir`.
    #[arg(long, value_name = "PATH")]
    pub output_dir: Option<PathBuf>,

    /// Run a publish dry-run only. With `--registry-dir`, validates
    /// what would happen against the registry without mutating it.
    /// Without `--registry-dir`, runs the staging-only dry-run that
    /// writes the archive + metadata to `--output-dir`.
    #[arg(long)]
    pub dry_run: bool,

    /// Local file-registry root to publish into. Without
    /// `--dry-run`, the registry is mutated; with `--dry-run`, every
    /// pre-write check runs but the registry is left untouched.
    #[arg(long, value_name = "PATH")]
    pub registry_dir: Option<PathBuf>,

    /// Output format for the publish or dry-run report.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags. In a workspace with
    /// multiple members, `cabin publish` requires a single
    /// `--package <name>` selection.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ResolveArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Path to a directory containing the local JSON package index.
    /// Required when the manifest declares any versioned dependencies
    /// and `--index-url` is not given. Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Output format. `human` is a readable summary; `json` produces a
    /// machine-parseable document. Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Require an existing, current `cabin.lock`. Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    /// Implies that `cabin.lock` will not be written.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects any state-writing side
    /// effects.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access. Cabin refuses to use an HTTP index
    /// URL (`--index-url` or a `[registry] index-url` config setting)
    /// and expects every needed input to be local or already cached.
    #[arg(long)]
    pub offline: bool,

    /// Workspace package-selection flags. The resolver is
    /// workspace-flat (every member shares one resolution), so
    /// selection only narrows the diagnostic output, not the
    /// resolution itself.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Feature names to enable on selected root packages.
    /// Repeatable; values may also be comma-separated.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every declared feature on selected root packages.
    /// Combines with `--features` (the union is requested).
    #[arg(long)]
    pub all_features: bool,

    /// Disable selected root packages' `default` feature.
    #[arg(long)]
    pub no_default_features: bool,

    /// Disable every active patch and source-replacement entry
    /// for this invocation. See `docs/patch-overrides.md`.
    #[arg(long)]
    pub no_patches: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Path to a directory containing the local JSON package index.
    /// Required when the manifest declares any versioned dependencies
    /// and `--index-url` is not given. Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Update only the named **dependency** (and any of its
    /// transitive deps that must change to satisfy the new
    /// constraints). Without this flag every locked package is
    /// re-resolved.
    ///
    /// `--package` here means "refresh this direct versioned
    /// dependency", *not* "scope to this workspace member".
    /// Workspace members can still be scoped through
    /// `--workspace`, `--default-members`, and `--exclude`; the
    /// workspace-selection bundle on `cabin update` deliberately
    /// omits `-p` / `--package` to avoid the name collision.
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,

    /// Output format for the resulting resolution.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Forbid network access. Cabin refuses to use an HTTP index
    /// URL (`--index-url` or a `[registry] index-url` config setting)
    /// and expects every needed input to be local or already cached.
    #[arg(long)]
    pub offline: bool,

    /// Workspace package-selection flags scoped to
    /// `cabin update`'s flag space (no `-p / --package`; see the
    /// docstring on `package` above).
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgsForUpdate,

    /// Disable every active patch and source-replacement entry
    /// for this invocation. See `docs/patch-overrides.md`.
    #[arg(long)]
    pub no_patches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ResolveFormat {
    Human,
    Json,
}

/// Default manifest filename used by every command.
const MANIFEST_FILENAME: &str = scaffold::MANIFEST_FILENAME;

/// Dispatch a parsed CLI invocation. Returns the exit code the
/// process should propagate. Most commands return
/// `ExitCode::SUCCESS` on the happy path; `cabin run` forwards
/// the spawned program's exit status so a non-zero exit from the
/// program becomes Cabin's own exit status.
///
/// The `cli.color` field carries the user's `--color` choice;
/// the resolved [`cabin_core::ColorChoice`] for top-level
/// error rendering is computed in `main.rs` against the env
/// and the user-level config. Subcommands today produce
/// uncolored status output and so do not consume the resolved
/// color; when a subcommand learns to emit styled output, it
/// should accept the resolved choice as an explicit argument
/// rather than re-deriving it here.
pub(crate) fn run(
    cli: Cli,
    reporter: Reporter,
    color: cabin_core::ColorChoice,
) -> Result<std::process::ExitCode> {
    use std::process::ExitCode;
    // `--version` (and the short `-V`) routes through the same
    // formatter `cabin version` uses, so `cabin --version -v`
    // produces the verbose key/value block instead of the
    // concise single line clap's auto-flag would emit.  The
    // flag wins over any subcommand and over `--list`, matching
    // cargo's precedence.
    if cli.version {
        crate::cli::version::version(crate::cli::version::VersionArgs {}, reporter.verbosity())?;
        return Ok(ExitCode::SUCCESS);
    }
    // `--list` is mutually exclusive with every other input;
    // clap rejects `cabin --list <subcommand>` for us.  Print
    // the full subcommand list and exit successfully.  The
    // listing is written through a `termcolor::StandardStream`
    // tuned to the caller-resolved color choice so the
    // cargo-style palette (green heading, cyan subcommand
    // names) appears whenever `--color` says it should.
    if cli.list {
        let mut stdout =
            termcolor::StandardStream::stdout(cabin_diagnostics::termcolor_choice(color));
        crate::command_list::print_list(&mut stdout)?;
        return Ok(ExitCode::SUCCESS);
    }
    let Some(command) = cli.command else {
        // `cabin` with no subcommand prints the curated help
        // and exits zero, matching the prior implicit behavior
        // (clap's auto help) but routed through the dispatcher
        // so the exit code is documented here.
        let mut cmd = <Cli as clap::CommandFactory>::command();
        cmd.print_help().context("failed to print top-level help")?;
        // Cargo prints help and exits 0 when invoked with no
        // arguments.  Cabin matches that.
        return Ok(ExitCode::SUCCESS);
    };
    match command {
        Command::Init(args) => init(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::New(args) => new(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Add(args) => crate::cli::add::add(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Remove(args) => {
            crate::cli::remove::remove(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Metadata(args) => {
            crate::cli::metadata::metadata(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Build(args) => {
            build(&args, reporter, BuildMode::Build).map(|()| ExitCode::SUCCESS)
        }
        Command::Check(args) => {
            build(&args, reporter, BuildMode::Check).map(|()| ExitCode::SUCCESS)
        }
        Command::Clean(args) => clean(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Run(args) => crate::cli::run::run(&args, reporter),
        Command::Test(args) => crate::cli::test::test(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Resolve(args) => resolve(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Update(args) => update(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Fetch(args) => fetch(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Vendor(args) => {
            crate::cli::vendor::vendor(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Tree(args) => crate::cli::tree::tree(&args).map(|()| ExitCode::SUCCESS),
        Command::Explain(args) => {
            crate::cli::explain::explain(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Package(args) => package(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Publish(args) => publish(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Fmt(args) => crate::cli::fmt::fmt(&args, reporter),
        Command::Tidy(args) => crate::cli::tidy::tidy(&args, reporter),
        Command::Port(args) => {
            crate::port_subcommand::port(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Compgen(args) => crate::completions::run(&args).map(|()| ExitCode::SUCCESS),
        Command::Mangen(args) => crate::manpages::run(&args).map(|()| ExitCode::SUCCESS),
        Command::Version(args) => {
            crate::cli::version::version(args, reporter.verbosity()).map(|()| ExitCode::SUCCESS)
        }
    }
}

/// Render the optimization / debuginfo descriptor that follows
/// the profile name in the `Finished` status line, matching
/// cargo's own banner:
///
/// - `unoptimized + debuginfo` for `dev` and any other `O0` +
///   debug build,
/// - `optimized` for `release` and other non-zero opt levels,
/// - `optimized + debuginfo` when both flags are on.
pub(crate) fn profile_descriptor(profile: &cabin_core::ResolvedProfile) -> String {
    let opt = if matches!(profile.opt_level, cabin_core::OptLevel::O0) {
        "unoptimized"
    } else {
        "optimized"
    };
    if profile.debug {
        format!("{opt} + debuginfo")
    } else {
        opt.to_owned()
    }
}

/// Translate `cabin build`'s `--profile` / `--release` flags into
/// a typed [`cabin_core::ProfileSelection`].
///
/// `--release` is preserved as a compatibility alias for
/// `--profile release`. clap's `conflicts_with` already rejects
/// the both-set combination so this helper only sees one of the
/// three possible inputs.
fn profile_selection_for_build(
    args: &BuildArgs,
    config: &cabin_config::EffectiveConfig,
) -> Result<cabin_core::ProfileSelection> {
    profile_selection_from_flags(args.profile.as_deref(), args.release, config)
}

/// Shared profile-selection precedence: explicit `--profile NAME`
/// wins, then the legacy `--release` alias, then any config-
/// supplied default, then the built-in `dev` profile. Used by
/// `cabin build` and `cabin test`.
pub(crate) fn profile_selection_from_flags(
    profile: Option<&str>,
    release: bool,
    config: &cabin_config::EffectiveConfig,
) -> Result<cabin_core::ProfileSelection> {
    if let Some(name) = profile {
        let pname = cabin_core::ProfileName::new(name.to_owned())
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        return Ok(cabin_core::ProfileSelection::from_name(pname));
    }
    if release {
        return Ok(cabin_core::ProfileSelection::release_alias());
    }
    if let Some((selection, _source)) = crate::cli::config::config_profile_selection(config)? {
        return Ok(selection);
    }
    Ok(cabin_core::ProfileSelection::default_dev())
}

/// `cabin metadata` accepts a `--profile` flag but no `--release`
/// alias (metadata is read-only and doesn't need the legacy spelling).
/// Falls back to a config-supplied default when the user did not
/// pass `--profile`; otherwise the built-in `dev` profile applies.
pub(crate) fn profile_selection_for_metadata(
    name: Option<&str>,
    config: &cabin_config::EffectiveConfig,
) -> Result<cabin_core::ProfileSelection> {
    profile_selection_from_flags(name, false, config)
}

/// Look up the profile-definition table that should drive
/// resolution. Profiles are workspace-wide: only the entry-point
/// manifest's `[profile.*]` tables count, so we read them off the
/// graph's root package (workspace root or single-package root).
pub(crate) fn workspace_profile_definitions(
    graph: &PackageGraph,
) -> BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition> {
    graph.root_settings.profiles.clone()
}

/// Workspace-root manifest's `[toolchain]` plus any
/// `[target.'cfg(...)'.toolchain]` overrides. Workspace member
/// manifests cannot declare a `[toolchain]` table — the workspace
/// loader rejects them — so reading off the root is sufficient.
pub(crate) fn workspace_toolchain_settings(graph: &PackageGraph) -> cabin_core::ToolchainSettings {
    graph.root_settings.toolchain.clone()
}

/// Translate `cabin build`'s / `cabin metadata`'s tool-selection
/// CLI flags into a typed [`cabin_core::ToolchainSelection`].
pub(crate) fn toolchain_selection_from_args(
    args: &ToolchainSelectionArgs,
) -> Result<cabin_core::ToolchainSelection> {
    let mut sel = cabin_core::ToolchainSelection::default();
    if let Some(raw) = &args.cc {
        sel = sel.with_cli(cabin_core::ToolKind::CCompiler, parse_cli_tool(raw)?);
    }
    if let Some(raw) = &args.cxx {
        sel = sel.with_cli(cabin_core::ToolKind::CxxCompiler, parse_cli_tool(raw)?);
    }
    if let Some(raw) = &args.ar {
        sel = sel.with_cli(cabin_core::ToolKind::Archiver, parse_cli_tool(raw)?);
    }
    Ok(sel)
}

fn parse_cli_tool(raw: &str) -> Result<cabin_core::ToolSpec> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("tool argument must be a non-empty path or command name");
    }
    Ok(cabin_core::ToolSpec::parse(trimmed.to_owned()))
}

/// Resolve a toolchain by layering manifest settings, the
/// optional `[toolchain]` config layer, and process-discovered
/// defaults on top of `selection` (already-parsed CLI overrides
/// or `ToolchainSelection::default()`).
pub(crate) fn resolve_toolchain_layered(
    graph: &PackageGraph,
    selection: &cabin_core::ToolchainSelection,
    effective_config: &cabin_config::EffectiveConfig,
    host_platform: &cabin_core::TargetPlatform,
) -> Result<cabin_core::ResolvedToolchain> {
    let manifest_toolchain_settings = workspace_toolchain_settings(graph);
    let config_toolchain_layer = crate::cli::config::toolchain_layer(effective_config);
    let mut toolchain_inputs = cabin_toolchain::ResolveInputs::from_process(
        selection,
        &manifest_toolchain_settings,
        host_platform,
    );
    if let Some(layer) = config_toolchain_layer.as_ref() {
        toolchain_inputs = toolchain_inputs.with_config(layer);
    }
    Ok(cabin_toolchain::resolve_toolchain(&toolchain_inputs)?)
}

/// Translate the `--compiler-wrapper` / `--no-compiler-wrapper`
/// CLI flag pair into a typed
/// [`cabin_core::CompilerWrapperRequest`] override. Clap already
/// rejects passing both flags simultaneously; this helper only
/// validates the value passed to `--compiler-wrapper`.
pub(crate) fn compiler_wrapper_override_from_args(
    args: &ToolchainSelectionArgs,
) -> Result<Option<cabin_core::CompilerWrapperRequest>> {
    if args.no_compiler_wrapper {
        return Ok(Some(cabin_core::CompilerWrapperRequest::Disabled));
    }
    let Some(raw) = args.compiler_wrapper.as_deref() else {
        return Ok(None);
    };
    let parsed = cabin_core::CompilerWrapperRequest::parse(raw)
        .with_context(|| format!("invalid --compiler-wrapper value `{raw}`"))?;
    Ok(Some(parsed))
}

/// Resolve the compiler-cache wrapper by layering the CLI
/// override (`--compiler-wrapper` / `--no-compiler-wrapper`), the
/// manifest's `[build.cache]` settings, the optional config
/// `[build.cache.compiler-wrapper]` layer, and process-detected
/// version metadata. Returns the typed resolution on success;
/// callers that want fail-soft behavior (e.g. `cabin metadata`)
/// call `resolve_compiler_wrapper` directly.
pub(crate) fn resolve_compiler_wrapper_layered(
    cli_override: Option<cabin_core::CompilerWrapperRequest>,
    manifest_settings: &cabin_core::CompilerWrapperManifestSettings,
    effective_config: &cabin_config::EffectiveConfig,
    host_platform: &cabin_core::TargetPlatform,
) -> Result<Option<cabin_core::ResolvedCompilerWrapper>> {
    let mut wrapper_inputs = cabin_toolchain::WrapperInputs::from_process(
        cli_override,
        manifest_settings,
        host_platform,
    );
    if let Some(layer) = crate::cli::config::wrapper_layer(effective_config) {
        wrapper_inputs = wrapper_inputs.with_config(layer);
    }
    cabin_toolchain::resolve_compiler_wrapper(
        &wrapper_inputs,
        Some(&cabin_toolchain::ProcessRunner),
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))
}

/// Workspace-root manifest's compiler-wrapper settings. Mirrors
/// [`workspace_toolchain_settings`] — the workspace loader rejects
/// non-empty declarations on member manifests so reading the root
/// is sufficient.
pub(crate) fn workspace_compiler_wrapper_settings(
    graph: &PackageGraph,
) -> cabin_core::CompilerWrapperManifestSettings {
    graph.root_settings.compiler_wrapper.clone()
}

/// Compute per-package `ResolvedProfileFlags` for every package in
/// the graph. The result is keyed by package index so callers
/// (planner, metadata view) can read them without rerunning the
/// merge per package.
pub(crate) fn resolve_per_package_build_flags(
    graph: &PackageGraph,
    profile_build: Option<&cabin_core::ProfileFlags>,
    host_platform: &cabin_core::TargetPlatform,
    feature_resolution: &cabin_feature::FeatureResolution,
    detection: Option<&cabin_core::ToolchainDetectionReport>,
) -> HashMap<usize, cabin_core::ResolvedProfileFlags> {
    // Detected compiler identities gate `[target.'cfg(cc/cxx = ...)'.profile]`
    // layers. `None` (fail-soft commands where detection failed) evaluates
    // those layers as family `unknown` with no version.
    let (cc_identity, cxx_identity) = match detection {
        Some(report) => (
            report.cc.as_ref().map(|tool| &tool.identity),
            Some(&report.cxx.identity),
        ),
        None => (None, None),
    };
    let mut out = HashMap::with_capacity(graph.packages.len());
    for (idx, pkg) in graph.packages.iter().enumerate() {
        // A registry/downloaded dependency's own `[profile]` build flags are
        // untrusted: only local packages (the workspace root, its members, and
        // `path` dependencies) may contribute raw compiler/linker flags.
        // `resolve_build_flags` drops the dependency's cflags/cxxflags/ldflags
        // when this is false, so a malicious dependency cannot smuggle a
        // code-executing compiler flag (e.g. `-fplugin=`) onto its build line.
        let package_trusted = matches!(pkg.kind, cabin_workspace::PackageKind::Local);
        // The package's resolved enabled features gate its
        // `[target.'cfg(feature = "...")'.profile]` flag layers. cabin-core
        // stays feature-vocabulary-only (it must not depend on cabin-feature),
        // so the cli pulls the name set out of the resolution and hands core
        // a bare `&BTreeSet<String>`.
        let package_features = feature_resolution.for_package(idx);
        let ctx = cabin_core::ConditionContext::with_features(
            host_platform,
            &package_features.enabled_features,
        )
        .with_compilers(cc_identity, cxx_identity);
        let resolved = cabin_core::resolve_build_flags(
            &pkg.package.build,
            profile_build,
            &ctx,
            package_trusted,
        );
        out.insert(idx, resolved);
    }
    out
}

/// Apply the documented post-profile build-flag layers — `pkg-config`
/// probes for active system dependencies, then `CPPFLAGS` / `CFLAGS`
/// / `CXXFLAGS` / `LDFLAGS` from the process environment — in the
/// order both layers must run for the resulting
/// `BuildConfiguration::fingerprint` to stay stable across commands.
/// Reports from both layers are intentionally discarded; callers that
/// need them invoke the individual `crate::cli::system_deps` /
/// `crate::cli::env_flags` helpers directly.
pub(crate) fn augment_build_flags(
    graph: &PackageGraph,
    host_platform: &cabin_core::TargetPlatform,
    dev_for: &BTreeSet<String>,
    build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    reporter: Reporter,
) -> Result<HashMap<usize, cabin_core::ResolvedProfileFlags>> {
    let (build_flags, _system_dep_reports) =
        crate::cli::system_deps::augment_build_flags_with_system_deps(
            graph,
            host_platform,
            dev_for,
            build_flags,
            reporter,
        )?;
    let (build_flags, _env_build_flags) = crate::cli::env_flags::augment_build_flags_with_env(
        graph,
        build_flags,
        |k| std::env::var_os(k),
        reporter,
    )?;
    Ok(build_flags)
}

/// Convert raw `--features` flag values into a `SelectionRequest`.
/// Validation against package declarations happens later in
/// `BuildConfiguration::resolve`.
pub(crate) fn build_selection_request(
    feature_args: &[String],
    all_features: bool,
    no_default_features: bool,
) -> cabin_core::SelectionRequest {
    let mut features: BTreeSet<String> = BTreeSet::new();
    for raw in feature_args {
        for token in raw.split(',') {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                continue;
            }
            features.insert(trimmed.to_owned());
        }
    }
    cabin_core::SelectionRequest {
        features,
        all_features,
        no_default_features,
    }
}

/// Resolve a `BuildConfiguration` for every package in the graph.
/// CLI feature selection requests apply to primary packages only —
/// non-primary packages (transitive path / registry deps) fall back
/// to their declared defaults until per-dependency feature requests
/// land.
pub(crate) fn resolve_build_configurations(
    graph: &PackageGraph,
    request: &cabin_core::SelectionRequest,
    selected: &[usize],
    profile: &cabin_core::ResolvedProfile,
    toolchain: &cabin_core::ToolchainSummary,
    build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
) -> Result<HashMap<usize, cabin_core::BuildConfiguration>> {
    use HashMap;
    let selected_set: HashSet<usize> = selected.iter().copied().collect();
    let mut out: HashMap<usize, cabin_core::BuildConfiguration> = HashMap::new();
    for (idx, pkg) in graph.packages.iter().enumerate() {
        // CLI feature requests apply only to *selected* packages.
        // Non-selected packages — including workspace siblings the
        // user did not pick — fall back to their declared defaults
        // so an unrelated package's missing feature does not fail
        // an unrelated build.
        let pkg_request = if selected_set.contains(&idx) {
            request.clone()
        } else {
            cabin_core::SelectionRequest::default()
        };
        let pkg_flags = build_flags.get(&idx).cloned().unwrap_or_default();
        let cfg = cabin_core::BuildConfiguration::resolve(cabin_core::BuildConfigurationInput {
            package: pkg.package.name.as_str(),
            features: &pkg.package.features,
            request: &pkg_request,
            profile: profile.clone(),
            toolchain: toolchain.clone(),
            build_flags: pkg_flags,
        })
        .with_context(|| {
            format!(
                "invalid configuration selection for package `{}`",
                pkg.package.name.as_str()
            )
        })?;
        out.insert(idx, cfg);
    }
    Ok(out)
}

/// Resolve the manifest the user is operating on. When the
/// user did not pass `--manifest-path` (the option is `None`), walk
/// upward from the current directory looking for a workspace root
/// and prefer it. When the user passed `--manifest-path`
/// Explicitly — even with the value `cabin.toml` — the supplied
/// Path is honored as-is so callers can intentionally target a
/// specific manifest from any directory.
pub(crate) fn resolve_invocation_manifest(args_path: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    match args_path {
        Some(path) => {
            if path.is_absolute() {
                Ok(path.to_path_buf())
            } else {
                Ok(cwd.join(path))
            }
        }
        None => {
            if let Some(found) = cabin_workspace::discover_workspace_root(&cwd)? {
                Ok(found.manifest_path)
            } else {
                Ok(cwd.join(MANIFEST_FILENAME))
            }
        }
    }
}

/// Convert CLI workspace-selection flags into a
/// `cabin_workspace::PackageSelection`. The mode mirrors the order
/// of `WorkspaceSelectionArgs`'s field comments.
pub(crate) fn build_workspace_selection(
    args: &WorkspaceSelectionArgs,
) -> cabin_workspace::PackageSelection {
    use cabin_workspace::SelectionMode;
    let mode = if args.workspace {
        SelectionMode::WholeWorkspace
    } else if !args.package.is_empty() {
        SelectionMode::ExplicitPackages(args.package.clone())
    } else if args.default_members {
        SelectionMode::DefaultMembers
    } else {
        SelectionMode::CurrentPackage
    };
    cabin_workspace::PackageSelection {
        mode,
        exclude: args.exclude.clone(),
    }
}

/// Build the selection's closure once and adapt a
/// [`cabin_feature::FeatureResolution`] handle into the
/// `Fn(usize, &str) -> bool` optional-dep filter the workspace
/// versioned-dep helpers consume. Shared by the collect / has shims
/// below so the closure build + filter adapter live in one place.
fn closure_and_optional_filter<'a>(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &'a cabin_feature::FeatureResolution,
) -> (BTreeSet<usize>, impl Fn(usize, &str) -> bool + 'a) {
    (selection.closure(graph), move |idx, name| {
        features.is_optional_dep_enabled(idx, name)
    })
}

/// Collect every versioned dependency reachable from `selection`
/// after dropping patched names. Thin shim around the typed API
/// in `cabin-workspace`.
pub(crate) fn collect_closure_versioned_deps_excluding_patches(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &cabin_feature::FeatureResolution,
    patched_names: &BTreeSet<String>,
    dev_for: &BTreeSet<String>,
) -> Result<BTreeMap<PackageName, semver::VersionReq>> {
    let (closure, is_optional_dep_enabled) =
        closure_and_optional_filter(graph, selection, features);
    cabin_workspace::collect_closure_versioned_deps_excluding_with_dev(
        graph,
        &closure,
        is_optional_dep_enabled,
        patched_names,
        dev_for,
    )
    .map_err(Into::into)
}

/// Merge `extra` into `into`, joining version requirements for
/// names that appear in both so the resolver sees a single
/// requirement per package. Mirrors the join-and-reparse pattern
/// the workspace closure walker uses.
fn merge_versioned_deps(
    into: &mut BTreeMap<PackageName, semver::VersionReq>,
    extra: BTreeMap<PackageName, semver::VersionReq>,
) -> Result<()> {
    for (name, req) in extra {
        match into.entry(name.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(req);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let parsed = cabin_workspace::combine_version_reqs(&[
                    slot.get().to_string(),
                    req.to_string(),
                ])
                .map_err(|(joined, err)| {
                    anyhow::anyhow!(
                        "conflicting dependency requirements for {}: {}: {}",
                        name.as_str(),
                        joined,
                        err
                    )
                })?;
                slot.insert(parsed);
            }
        }
    }
    Ok(())
}

/// Whether the selected closure carries any versioned
/// (registry-bound) dependency that the artifact pipeline would
/// need to fetch. Thin shim around the typed API in
/// `cabin-workspace`.
pub(crate) fn closure_has_versioned_deps_excluding_patches(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &cabin_feature::FeatureResolution,
    patched_names: &BTreeSet<String>,
    dev_for: &BTreeSet<String>,
) -> bool {
    let (closure, is_optional_dep_enabled) =
        closure_and_optional_filter(graph, selection, features);
    cabin_workspace::closure_has_versioned_deps_excluding_with_dev(
        graph,
        &closure,
        is_optional_dep_enabled,
        patched_names,
        dev_for,
    )
}

/// Resolve features for the selected closure. Roots receive the
/// caller-provided request; non-root reachable packages inherit
/// requests through dependency edges per the documented feature
/// model. The returned [`cabin_feature::FeatureResolution`] is
/// then threaded into the dependency-iteration helpers so
/// disabled optional dependencies disappear from the resolver /
/// fetch / build planning.
pub(crate) fn compute_feature_resolution(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    request: &cabin_core::SelectionRequest,
) -> Result<cabin_feature::FeatureResolution> {
    let root_request: cabin_feature::RootFeatureRequest = request.into();
    let platform = cabin_core::TargetPlatform::current();
    cabin_feature::resolve_features(graph, &selection.packages, &root_request, &platform)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Pick the primary packages that contribute versioned
/// deps to a resolve / fetch / update run. When the user passed
/// workspace-selection flags, only their selected packages
/// contribute. Otherwise the documented default applies (root
/// package or every primary).
fn selected_resolution_packages(
    graph: &PackageGraph,
    selection: &cabin_workspace::PackageSelection,
) -> Result<cabin_workspace::ResolvedSelection> {
    cabin_workspace::resolve_package_selection(graph, selection).map_err(std::convert::Into::into)
}

/// Pick the single package manifest path that
/// `cabin package` / `cabin publish` should operate on. When the
/// invocation manifest is a workspace root, the user must supply
/// exactly one explicit `--package <name>` selection. Otherwise we
/// honor the existing single-package contract.
/// Result of selecting a single package manifest for a
/// workspace-aware `cabin package` / `cabin publish` invocation.
/// Carries both the manifest path and the pre-resolved `Package`,
/// so member manifests with `dep = { workspace = true }` reach
/// `cabin-package` after the workspace loader has substituted the
/// inherited requirement.
struct SinglePackageSelection {
    manifest_path: PathBuf,
    /// `Some` when the manifest was loaded through a workspace
    /// (so `cabin-workspace` resolved any `workspace = true` deps).
    /// `None` when the user passed a standalone manifest path; in
    /// that case `cabin-package`'s own validator decides what to do
    /// with any unresolved workspace dep it sees.
    resolved_project: Option<cabin_core::Package>,
}

fn select_single_package_manifest(
    invocation: &Path,
    selection: &WorkspaceSelectionArgs,
    command: &'static str,
) -> Result<SinglePackageSelection> {
    let parsed = cabin_manifest::load_manifest(invocation)
        .with_context(|| format!("failed to load manifest at {}", invocation.display()))?;
    if parsed.workspace.is_none() {
        // Single-package manifest: the existing behavior applies
        // unchanged. Reject workspace-selection flags so the user
        // never gets the impression Cabin honored them silently.
        if selection.workspace
            || selection.default_members
            || !selection.package.is_empty()
            || !selection.exclude.is_empty()
        {
            bail!(
                "workspace package-selection flags are not valid for `cabin {command}` against a non-workspace manifest"
            );
        }
        return Ok(SinglePackageSelection {
            manifest_path: invocation.to_path_buf(),
            resolved_project: None,
        });
    }
    if selection.package.len() != 1 || selection.workspace || selection.default_members {
        bail!(
            "`cabin {command}` requires a single `--package <name>` selection inside a workspace; use `--package <name>` to pick the package to {command}"
        );
    }
    if !selection.exclude.is_empty() {
        bail!(
            "`--exclude` is not valid for `cabin {command}`; pass exactly one `--package <name>`"
        );
    }
    // Package / publish only need to identify the selected
    // workspace member; foundation-port edges are skipped so the
    // selection works without network access on workspaces with
    // HTTP-backed ports that have never been cached.
    let graph = cabin_workspace::load_workspace_skip_ports(invocation)?;
    let name = &selection.package[0];
    let idx = graph
        .index_of(name)
        .ok_or_else(|| anyhow::anyhow!("package `{name}` is not a member of this workspace"))?;
    if !graph.primary_packages.contains(&idx) {
        bail!("package `{name}` is not a member of this workspace");
    }
    Ok(SinglePackageSelection {
        manifest_path: graph.packages[idx].manifest_path.clone(),
        resolved_project: Some(graph.packages[idx].package.clone()),
    })
}

pub(crate) fn lock_mode_for_flags(locked: bool, frozen: bool) -> LockMode {
    if locked || frozen {
        LockMode::Locked
    } else {
        LockMode::PreferLocked
    }
}

/// Resolve the cache directory using --cache-dir,
/// `$CABIN_CACHE_DIR`, or the user-global XDG fallback.
///
/// Precedence: `--cache-dir` ▶ `$CABIN_CACHE_DIR` ▶
/// `$CABIN_CACHE_HOME` ▶ the platform base cache directory with a
/// `cabin` suffix (`$XDG_CACHE_HOME/cabin` / `~/.cache/cabin` on
/// Linux, `~/Library/Caches/cabin` on macOS, `%LOCALAPPDATA%\cabin`
/// on Windows). The fallback shape mirrors `cabin_config::discovery`
/// so the cache home and config home follow the same rule.
///
/// The cache is content-addressed (e.g. foundation-port archives
/// land at `<cache>/ports/archives/sha256/<hex>.tar.gz`), so the
/// user-global default lets two projects on the same machine
/// share a single download.
pub(crate) fn cache_dir_for(override_dir: Option<&Path>) -> Result<PathBuf> {
    let user_cache_home = directories::BaseDirs::new().map(|dirs| dirs.cache_dir().join("cabin"));
    cache_dir_for_with_env(
        override_dir,
        &|key| std::env::var_os(key),
        user_cache_home.as_deref(),
    )
}

/// Inner form of [`cache_dir_for`] with the env lookup and the
/// xdg-resolved user cache home injected so tests can drive the
/// precedence chain without touching real process env. Production
/// code calls [`cache_dir_for`].
fn cache_dir_for_with_env(
    override_dir: Option<&Path>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    xdg_cache_home: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(p) = override_dir {
        return absolutise(p)
            .with_context(|| format!("failed to resolve cache dir {}", p.display()));
    }
    if let Some(val) = env(cabin_env::CABIN_CACHE_DIR).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(val);
        return absolutise(&p)
            .with_context(|| format!("failed to resolve cache dir {}", p.display()));
    }
    user_cache_default(env, xdg_cache_home).ok_or_else(|| {
        anyhow::anyhow!(
            "no cache directory: set --cache-dir, CABIN_CACHE_DIR, CABIN_CACHE_HOME, XDG_CACHE_HOME, or HOME"
        )
    })
}

/// User-global cache root: `$CABIN_CACHE_HOME` if set, otherwise
/// the xdg-resolved user cache home with the `cabin` application
/// prefix already applied (`$XDG_CACHE_HOME/cabin`, falling back
/// to `$HOME/.cache/cabin` per the XDG Base Directory spec). The
/// `CABIN_CACHE_HOME` override is Cabin-specific and resolves
/// directly to its value with no extra `cabin` component.
fn user_cache_default(
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    xdg_cache_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(d) = env(cabin_env::CABIN_CACHE_HOME).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    xdg_cache_home.map(Path::to_path_buf)
}

pub(crate) struct ArtifactPipelineRequest<'a> {
    pub(crate) manifest_path: &'a Path,
    pub(crate) initial_graph: &'a PackageGraph,
    pub(crate) index_path: Option<&'a Path>,
    pub(crate) index_url: Option<&'a str>,
    pub(crate) mode: LockMode,
    pub(crate) allow_write: bool,
    pub(crate) frozen: bool,
    pub(crate) cache_dir: &'a Path,
    pub(crate) reporter: Reporter,
    /// Workspace selection that contributes versioned deps
    /// to the resolution. Defaults to every primary package when
    /// the user passes no selection flags.
    pub(crate) selection: cabin_workspace::PackageSelection,
    /// Feature flags from the CLI. Drives optional-dependency
    /// inclusion.
    pub(crate) selection_request: &'a cabin_core::SelectionRequest,
    /// Names of patched packages — the pipeline must skip them
    /// because they ship from a local working copy and never need
    /// to be fetched from the index.
    pub(crate) patched_names: &'a BTreeSet<String>,
    /// Active patches recorded into the new lockfile and
    /// compared against the existing lockfile under `--locked`.
    pub(crate) active_patches: &'a cabin_workspace::ActivePatchSet,
    /// Active source-replacement entries (post-merge) recorded
    /// into the new lockfile.
    pub(crate) source_replacements: &'a cabin_core::SourceReplacementSettings,
    /// Whether `--no-patches` was supplied — suppresses
    /// source-replacement records on the lockfile to match the
    /// "no local override policy" semantics.
    pub(crate) no_patches: bool,
    /// Names of packages whose `[dev-dependencies]` should be
    /// activated for this invocation. Empty for `cabin build`;
    /// `cabin test` passes the selected primary packages' names
    /// so the resolver / fetch path picks up dev-deps the test
    /// executables need.
    pub(crate) dev_for: &'a BTreeSet<String>,
}

pub(crate) struct ArtifactPipeline {
    pub(crate) fetched: Vec<FetchedPackage>,
}

impl ArtifactPipeline {
    /// Project each fetched package into the
    /// [`RegistryPackageSource`] the workspace loader consumes,
    /// pinning every manifest at `<source_dir>/cabin.toml`. Shared
    /// by `build` / `run` / `test`, which all feed the fetched
    /// closure back into a strict workspace reload.
    pub(crate) fn registry_sources(&self) -> Vec<RegistryPackageSource> {
        self.fetched
            .iter()
            .map(|p| RegistryPackageSource {
                name: p.name.clone(),
                version: p.version.clone(),
                manifest_path: p.source_dir.join("cabin.toml"),
            })
            .collect()
    }
}

/// Resolved index access: either a directory on disk we already
/// turned into a [`PackageIndex`], or a live HTTP client we will use
/// to download artifacts.
enum IndexAccess {
    Local,
    Http(cabin_index_http::HttpClient),
}

/// Run the resolve → lockfile → fetch pipeline used by both
/// `cabin fetch` and `cabin build`.
pub(crate) fn run_artifact_pipeline(
    request: &ArtifactPipelineRequest<'_>,
) -> Result<ArtifactPipeline> {
    let manifest_path = request.manifest_path;
    let graph = request.initial_graph;
    let resolved_selection = selected_resolution_packages(graph, &request.selection)?;
    let features =
        compute_feature_resolution(graph, &resolved_selection, request.selection_request)?;
    let mut root_deps = collect_closure_versioned_deps_excluding_patches(
        graph,
        &resolved_selection,
        &features,
        request.patched_names,
        request.dev_for,
    )?;
    // Patched manifests are not part of the workspace graph at
    // this point, so their own `[dependencies]` never appeared
    // in the closure walk. Fold them in so a workspace whose only
    // versioned dep is patched still resolves and fetches the
    // patched manifest's transitive registry edges.
    let patched_root_deps =
        collect_patched_versioned_deps(request.active_patches, request.patched_names)?;
    merge_versioned_deps(&mut root_deps, patched_root_deps)?;
    // short-circuit when neither the selected closure nor the
    // active patch set introduces a versioned dependency.
    // Loading an index, walking the lockfile, and downloading
    // artifacts are all unnecessary in that case.
    if root_deps.is_empty() {
        return Ok(ArtifactPipeline {
            fetched: Vec::new(),
        });
    }
    // pick a stable synthetic root identity for pure
    // workspace roots; fall back to the [package] root otherwise.
    let (root_name, root_version) = match graph.root_package {
        Some(idx) => (
            graph.packages[idx].package.name.clone(),
            graph.packages[idx].package.version.clone(),
        ),
        None => cabin_workspace::synthetic_root_identity(graph),
    };

    let lockfile_path = lockfile_path_for(manifest_path);

    let existing_lockfile: Option<Lockfile> = if lockfile_path.is_file() {
        Some(
            cabin_lockfile::read_lockfile(&lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        )
    } else {
        if matches!(request.mode, LockMode::Locked) {
            bail!(
                "cannot resolve with --locked because {} does not exist",
                lockfile_path.display()
            );
        }
        None
    };

    let (index, access) = load_index_for_pipeline(
        request.index_path,
        request.index_url,
        request.frozen,
        &root_deps,
    )?;

    let resolver_mode = match &request.mode {
        LockMode::PreferLocked => ResolveMode::PreferLocked,
        LockMode::Locked => ResolveMode::Locked,
        LockMode::UpdateAll => ResolveMode::UpdateAll,
        LockMode::UpdatePackage(name) => ResolveMode::UpdatePackage(
            PackageName::new(name.clone())
                .map_err(|err| anyhow::anyhow!("invalid --package value {name:?}: {err}"))?,
        ),
    };

    let mut input = ResolveInput::new(root_name, root_version, root_deps);
    if let Some(lock) = &existing_lockfile {
        for pkg in &lock.packages {
            input.locked.insert(
                pkg.name.clone(),
                LockedVersion {
                    version: pkg.version.clone(),
                    checksum: pkg.checksum.clone(),
                },
            );
        }
    }
    input.mode = resolver_mode;

    // Patch / source-replacement state recorded into the new
    // lockfile and compared against the existing lockfile under
    // `--locked`.
    let active_patch_records = crate::cli::patch::lockfile_patches(request.active_patches);
    let active_replacement_records = crate::cli::patch::lockfile_source_replacements(
        request.source_replacements,
        request.no_patches,
    );
    if matches!(request.mode, LockMode::Locked)
        && let Some(prev) = &existing_lockfile
        && !prev.matches_patch_state(&active_patch_records, &active_replacement_records)
    {
        bail!(
            "--locked cannot be used because active patch / source-replacement policy differs from {}; re-run without --locked to refresh the lockfile",
            lockfile_path.display()
        );
    }

    let output = cabin_resolver::resolve(&input, &index).context("dependency resolution failed")?;

    let mut new_lockfile = lockfile_from_resolution(&output, &index);
    new_lockfile.patches = active_patch_records;
    new_lockfile.source_replacements = active_replacement_records;

    if request.allow_write {
        let needs_write = match &existing_lockfile {
            Some(prev) => prev != &new_lockfile,
            None => true,
        };
        if needs_write {
            cabin_lockfile::write_lockfile(&lockfile_path, &new_lockfile)
                .with_context(|| format!("failed to write {}", lockfile_path.display()))?;
            request
                .reporter
                .aux_verbose(format_args!("cabin: wrote {}", lockfile_path.display()));
        } else {
            request.reporter.aux_verbose(format_args!(
                "cabin: {} is up to date",
                lockfile_path.display()
            ));
        }
    }

    let plan = build_fetch_plan(&output, &index, &access)?;
    let cache = ArtifactCache::new(request.cache_dir);
    let result = cabin_artifact::fetch(
        &plan,
        &cache,
        FetchOptions {
            frozen: request.frozen,
        },
    )?;
    Ok(ArtifactPipeline {
        fetched: result.packages,
    })
}

/// Pick the right index source for a fetch / build run, validate
/// CLI flag combinations, and return both the [`PackageIndex`] the
/// Resolver consumes and a tag describing which access mode the
/// fetch plan should use.
fn load_index_for_pipeline(
    index_path: Option<&Path>,
    index_url: Option<&str>,
    frozen: bool,
    root_deps: &BTreeMap<PackageName, semver::VersionReq>,
) -> Result<(PackageIndex, IndexAccess)> {
    match (index_path, index_url) {
        (Some(_), Some(_)) => bail!("use either --index-path or --index-url, not both"),
        (None, None) => {
            bail!("versioned dependencies require --index-path or --index-url")
        }
        (Some(path), None) => {
            let index_path = absolutise(path)
                .with_context(|| format!("failed to resolve {}", path.display()))?;
            let index = cabin_index::load_index(&index_path)
                .with_context(|| format!("failed to load index at {}", index_path.display()))?;
            Ok((index, IndexAccess::Local))
        }
        (None, Some(url)) => {
            if frozen {
                bail!(
                    "cannot use --index-url with --frozen: there is no persistent HTTP index metadata cache, so a frozen run would have to perform network fetches it is not allowed to perform"
                );
            }
            let client = cabin_index_http::HttpClient::new();
            let http_index = cabin_index_http::HttpIndex::open(url, client.clone())?;
            let names: Vec<PackageName> = root_deps.keys().cloned().collect();
            let index = http_index.load_package_index(&names)?;
            Ok((index, IndexAccess::Http(client)))
        }
    }
}

/// Build a [`FetchPlan`] from a resolver output and the index it ran
/// against. Each resolved registry package contributes exactly one
/// fetch entry; the index is the source of truth for `source` and
/// `checksum`.
///
/// `access` decides whether HTTP-resolved sources get downloaded
/// here (so `cabin-artifact` stays HTTP-free) or whether the source
/// Path is handed straight through as a local file.
fn build_fetch_plan(
    output: &ResolveOutput,
    index: &PackageIndex,
    access: &IndexAccess,
) -> Result<FetchPlan> {
    let mut entries = Vec::new();
    for resolved in &output.packages {
        if resolved.source != ResolvedSource::Index {
            continue;
        }
        let entry = index.package(&resolved.name).ok_or_else(|| {
            anyhow::anyhow!(
                "resolver chose `{} {}`, but it is not in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let meta = entry.versions.get(&resolved.version).ok_or_else(|| {
            anyhow::anyhow!(
                "resolver chose `{} {}`, but the index has no entry for this version",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let source = meta.source.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "package `{} {}` has no source artifact in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let checksum = meta.checksum.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "missing checksum for `{} {}`; cabin fetch requires a sha256:<hex> entry in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let fetch_source = match (&source.location, access) {
            (cabin_index::SourceLocation::LocalPath(p), _) => {
                cabin_artifact::FetchSource::LocalArchive(p.clone())
            }
            (cabin_index::SourceLocation::HttpUrl(url), IndexAccess::Http(client)) => {
                let label = format!("{} {}", resolved.name.as_str(), resolved.version);
                let bytes = client.download(url, &label).with_context(|| {
                    format!(
                        "failed to download source archive for `{} {}`",
                        resolved.name.as_str(),
                        resolved.version
                    )
                })?;
                cabin_artifact::FetchSource::InMemoryArchive(bytes)
            }
            (cabin_index::SourceLocation::HttpUrl(_), IndexAccess::Local) => {
                bail!(
                    "package `{} {}` has an HTTP source URL but the run is using a local index",
                    resolved.name.as_str(),
                    resolved.version
                );
            }
        };
        entries.push(FetchEntry {
            name: resolved.name.clone(),
            version: resolved.version.clone(),
            checksum,
            source: fetch_source,
        });
    }
    Ok(FetchPlan { entries })
}

/// What kind of resolution the CLI is asking for.
#[derive(Debug, Clone)]
pub(crate) enum LockMode {
    PreferLocked,
    Locked,
    UpdateAll,
    UpdatePackage(String),
}

pub(crate) fn lockfile_path_for(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
        .join("cabin.lock")
}

/// Read the lockfile at `lockfile_path` if it exists, attaching a
/// read-error context that names the path. Returns `Ok(None)` when
/// the file is absent. Shared by the read-only inspection commands
/// (`metadata` / `tree` / `explain`); the commands that enforce
/// `--locked` keep their own bespoke read so the missing-lockfile
/// case stays a hard error there.
pub(crate) fn read_optional_lockfile(lockfile_path: &Path) -> Result<Option<Lockfile>> {
    if lockfile_path.is_file() {
        Ok(Some(
            cabin_lockfile::read_lockfile(lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        ))
    } else {
        Ok(None)
    }
}

fn lockfile_from_resolution(output: &ResolveOutput, index: &cabin_index::PackageIndex) -> Lockfile {
    // We need each resolved package's transitive deps to write the
    // lockfile's `dependencies = [...]` field. The resolver doesn't
    // surface the dep edges directly, so we read them off the index
    // entry for the chosen version.
    let resolved_names: BTreeSet<&str> = output
        .packages
        .iter()
        .filter(|p| p.source == ResolvedSource::Index)
        .map(|p| p.name.as_str())
        .collect();
    let mut packages: Vec<LockedPackage> = Vec::new();
    for pkg in &output.packages {
        if pkg.source != ResolvedSource::Index {
            continue;
        }
        let entry = index
            .package(&pkg.name)
            .expect("index has every resolved package");
        let meta = entry
            .versions
            .get(&pkg.version)
            .expect("index has the resolved version");
        // Filter to only dep names that are also resolved (defensive).
        let mut deps: Vec<PackageName> = meta
            .dependencies
            .keys()
            .filter(|n| resolved_names.contains(n.as_str()))
            .cloned()
            .collect();
        deps.sort();
        packages.push(LockedPackage {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            source: LockedSource::Index,
            checksum: meta.checksum.clone(),
            dependencies: deps,
        });
    }
    packages.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    Lockfile {
        version: cabin_lockfile::LOCKFILE_VERSION,
        packages,
        patches: Vec::new(),
        source_replacements: Vec::new(),
    }
}

/// Resolve a path to an absolute one without requiring it to exist.
pub(crate) fn absolutise(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_binary_template_round_trips_through_parser() {
        let manifest = scaffold::render_manifest("hello", scaffold::ScaffoldKind::Binary);
        let parsed = cabin_manifest::parse_manifest_str(&manifest).unwrap();
        let package = parsed.package.expect("template should parse as a package");
        assert_eq!(package.name.as_str(), "hello");
        assert_eq!(package.targets.len(), 1);
        assert_eq!(package.targets[0].name.as_str(), "hello");
    }

    #[test]
    fn rendered_library_template_round_trips_through_parser() {
        let manifest = scaffold::render_manifest("hello", scaffold::ScaffoldKind::Library);
        let parsed = cabin_manifest::parse_manifest_str(&manifest).unwrap();
        let package = parsed.package.expect("template should parse as a package");
        assert_eq!(package.name.as_str(), "hello");
        assert_eq!(package.targets.len(), 1);
        assert_eq!(package.targets[0].name.as_str(), "hello");
    }

    #[test]
    fn registry_dependency_build_flags_are_dropped_but_local_kept() {
        use cabin_core::{Package, Target};
        use cabin_workspace::{PackageKind, WorkspacePackage};
        use std::path::PathBuf;

        fn dep_with_command_flags(name: &str, kind: PackageKind) -> WorkspacePackage {
            let mut package = Package::new(
                PackageName::new(name).unwrap(),
                semver::Version::parse("0.1.0").unwrap(),
                Vec::<Target>::new(),
                Vec::new(),
            )
            .unwrap();
            package.build.general.cflags = vec!["-fplugin=evil.so".into()];
            package.build.general.cxxflags = vec!["-B.".into()];
            package.build.general.ldflags = vec!["-fuse-ld=/tmp/evil".into()];
            WorkspacePackage {
                package,
                manifest_dir: PathBuf::from("/tmp"),
                manifest_path: PathBuf::from("/tmp/cabin.toml"),
                kind,
                deps: Vec::new(),
                is_port: false,
            }
        }

        let graph = PackageGraph {
            root_manifest_path: PathBuf::from("/tmp/cabin.toml"),
            root_dir: PathBuf::from("/tmp"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![
                dep_with_command_flags("local_dep", PackageKind::Local),
                dep_with_command_flags("registry_dep", PackageKind::Registry),
            ],
        };

        let host = cabin_core::TargetPlatform::current();
        let resolved = resolve_per_package_build_flags(
            &graph,
            None,
            &host,
            &cabin_feature::FeatureResolution::default(),
            None,
        );

        // A local package (workspace member / path dependency) is trusted:
        // its declared compiler and linker flags are preserved.
        let local = resolved.get(&0).expect("local package flags");
        assert_eq!(local.cflags, vec!["-fplugin=evil.so".to_owned()]);
        assert_eq!(local.cxxflags, vec!["-B.".to_owned()]);
        assert_eq!(local.ldflags, vec!["-fuse-ld=/tmp/evil".to_owned()]);

        // A registry dependency is untrusted: its compiler and linker flags
        // are dropped so it cannot execute code at build time.
        let registry = resolved.get(&1).expect("registry package flags");
        assert!(registry.cflags.is_empty());
        assert!(registry.cxxflags.is_empty());
        assert!(registry.ldflags.is_empty());
    }

    // -------------------------------------------------------------
    // cache_dir_for precedence (XDG user-global default)
    // -------------------------------------------------------------

    type EnvFn = Box<dyn Fn(&str) -> Option<std::ffi::OsString>>;

    /// Build an env-lookup closure backed by a fixed map. Mirrors
    /// the `env_with` helper in `cabin-config::discovery::tests`
    /// so the cache-dir tests look like the sibling config-dir
    /// tests they parallel.
    fn env_with(items: &[(&'static str, &str)]) -> EnvFn {
        let map: std::collections::HashMap<&'static str, std::ffi::OsString> = items
            .iter()
            .map(|(k, v)| (*k, std::ffi::OsString::from(*v)))
            .collect();
        Box::new(move |k| map.get(k).cloned())
    }

    /// The user cache home (`<HOME>/.cache/cabin`) Cabin resolves on
    /// Linux when `HOME` is `home` and `XDG_CACHE_HOME` is unset.
    /// Tests inject this so they exercise the fallback chain without
    /// mutating the process environment.
    fn home_xdg_cache_home(home: &str) -> PathBuf {
        PathBuf::from(home).join(".cache").join("cabin")
    }

    #[test]
    fn cache_dir_flag_wins_over_everything() {
        let env = env_with(&[
            ("CABIN_CACHE_DIR", "/tmp/from-env"),
            ("CABIN_CACHE_HOME", "/tmp/cabin-home"),
        ]);
        let xdg = PathBuf::from("/tmp/xdg/cabin");
        let out =
            cache_dir_for_with_env(Some(Path::new("/tmp/from-flag")), &env, Some(&xdg)).unwrap();
        // The `--cache-dir` and `CABIN_CACHE_DIR` arms absolutise
        // their value; compare against the same absolutisation so the
        // assertion holds on Windows (where `/tmp/from-flag` gains the
        // current drive) as well as on Unix.
        assert_eq!(out, absolutise(Path::new("/tmp/from-flag")).unwrap());
    }

    #[test]
    fn cabin_cache_dir_env_wins_over_xdg() {
        let env = env_with(&[
            ("CABIN_CACHE_DIR", "/tmp/from-env"),
            ("CABIN_CACHE_HOME", "/tmp/cabin-home"),
        ]);
        let xdg = PathBuf::from("/tmp/xdg/cabin");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, absolutise(Path::new("/tmp/from-env")).unwrap());
    }

    #[test]
    fn cabin_cache_home_used_when_cabin_cache_dir_unset() {
        // CABIN_CACHE_HOME is a Cabin-specific override: it
        // resolves directly to its value with no `cabin`
        // segment appended, and it wins over the xdg-resolved
        // path.
        let env = env_with(&[("CABIN_CACHE_HOME", "/tmp/cabin-home")]);
        let xdg = PathBuf::from("/tmp/xdg/cabin");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/cabin-home"));
    }

    #[test]
    fn xdg_cache_home_appends_cabin_segment() {
        // The injected `xdg_cache_home` represents the resolved user
        // cache home: the `cabin` segment is already applied, so
        // Cabin uses it verbatim.
        let env = env_with(&[]);
        let xdg = PathBuf::from("/tmp/xdg/cabin");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/xdg/cabin"));
    }

    #[test]
    fn home_cache_fallback_used_when_xdg_unset() {
        // When `XDG_CACHE_HOME` is unset, `xdg` falls back to
        // `$HOME/.cache`; the injected path simulates that.
        let env = env_with(&[]);
        let xdg = home_xdg_cache_home("/tmp/home");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/home/.cache/cabin"));
    }

    #[test]
    fn empty_cabin_cache_dir_value_falls_through() {
        // Empty-as-unset rule for CABIN_CACHE_DIR so a shell
        // that exports the variable as empty doesn't
        // short-circuit the XDG fallback.
        let env = env_with(&[("CABIN_CACHE_DIR", "")]);
        let xdg = home_xdg_cache_home("/tmp/home");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/home/.cache/cabin"));
    }

    #[test]
    fn empty_cabin_cache_home_value_falls_through_to_xdg() {
        // Same empty-as-unset rule for CABIN_CACHE_HOME so a
        // shell that exports the variable as empty doesn't
        // short-circuit the XDG fallback.
        let env = env_with(&[("CABIN_CACHE_HOME", "")]);
        let xdg = PathBuf::from("/tmp/xdg/cabin");
        let out = cache_dir_for_with_env(None, &env, Some(&xdg)).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/xdg/cabin"));
    }

    #[test]
    fn all_envs_unset_returns_error() {
        // No CABIN_CACHE_HOME, no CABIN_CACHE_DIR, no xdg-resolved
        // path (e.g. HOME and XDG_CACHE_HOME both unset on the host).
        let env = env_with(&[]);
        let err = cache_dir_for_with_env(None, &env, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no cache directory"),
            "expected diagnostic mentioning 'no cache directory', got: {msg}"
        );
    }
}
