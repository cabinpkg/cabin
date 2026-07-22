use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use cabin_build::{PlanRequest, plan};
use cabin_package::scaffold;
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
pub(crate) mod login;
pub(crate) mod metadata;
pub(crate) mod ninja;
pub(crate) mod patch;
pub(crate) mod port;
pub(crate) mod remove;
pub(crate) mod run;
pub(crate) mod source_tooling;
pub(crate) mod standard_compat;
pub(crate) mod system_deps;
pub(crate) mod term_color;
pub(crate) mod term_verbosity;
pub(crate) mod test;
pub(crate) mod tidy;
pub(crate) mod tree;
pub(crate) mod vendor;
pub(crate) mod version;
pub(crate) mod yank;

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

// The build-configuration and artifact/lockfile orchestration layers
// live in `build_prep` / `resolve`; the hub re-exports the entries the
// other command modules consume so their `super::` / `crate::cli::`
// paths stay stable.
pub(crate) use self::build_prep::{
    augment_build_flags, compiler_wrapper_override_from_args, profile_descriptor,
    profile_selection_for_metadata, profile_selection_from_flags, resolve_build_configurations,
    resolve_per_package_build_flags, resolve_per_package_language_standards,
    resolve_toolchain_layered, toolchain_selection_from_args, workspace_compiler_wrapper_settings,
    workspace_profile_definitions,
};
pub(crate) use self::resolve::{
    ArtifactPipelineRequest, LockPolicy, closure_has_versioned_deps_excluding_patches,
    lockfile_path_for, read_optional_lockfile, run_artifact_pipeline,
};

use crate::cli::fetch_output::emit_fetch_output;
use crate::cli::term_color::CliColorChoice;
use crate::cli::term_verbosity::Reporter;

/// Bail message shared by the build/run/test/resolve commands when a
/// manifest declares versioned dependencies but no index source was
/// provided.
pub(crate) const VERSIONED_DEPS_REQUIRE_INDEX: &str =
    "versioned dependencies require --index-path, --index-url, or a `[registry]` config setting";

/// Bail message shared by the resolve paths when `--index-url` is
/// combined with `--frozen`: there is no persistent HTTP index cache,
/// so a frozen run cannot honor the URL without forbidden fetches.
pub(crate) const FROZEN_INDEX_URL_ERR: &str = "cannot use --index-url with --frozen: there is no persistent HTTP index metadata cache, so a frozen run would have to perform network fetches it is not allowed to perform";

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

/// Top-level help template - mirrors `cargo --help`:
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

    /// Enable an experimental feature (may be repeated).
    //
    // Mirrors cargo's `-Z`.  The recognized names live in
    // `cabin_core::ExperimentalFeature`; an unknown name is
    // rejected by the value parser with the full list, so
    // future features fall out of the enum rather than a
    // feature-specific arm here.
    #[arg(
        short = 'Z',
        value_name = "FEATURE",
        global = true,
        action = clap::ArgAction::Append,
        value_parser = parse_experimental_feature,
        display_order = 5,
    )]
    pub(crate) unstable: Vec<cabin_core::ExperimentalFeature>,

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

/// clap value parser for `-Z`: exact-match against the typed
/// feature list, surfacing the enum's own error wording (which
/// names every recognized feature).
fn parse_experimental_feature(
    raw: &str,
) -> Result<cabin_core::ExperimentalFeature, cabin_core::UnknownExperimentalFeature> {
    raw.parse()
}

// `cabin --help` is the curated, day-to-day surface and
// closely mirrors `cargo --help`.  Subcommands tagged
// `#[command(hide = true)]` below stay fully functional but
// surface only through `cabin --list`, `cabin <sub> --help`,
// shell completions, and per-subcommand man pages.
//
// Curation pattern (matching cargo --help):
// - hide inspection-only commands (`metadata`, `tree`,
//   `explain`) - useful for scripts / CI, rarely typed
//   day-to-day;
// - hide low-level / scripting commands (`resolve`) -
//   `cabin metadata` and `cabin update` are the user-facing
//   paths;
// - hide offline / networking helpers (`fetch`, `vendor`) -
//   triggered automatically when needed;
// - hide pre-publish packaging (`package`) - `publish` is
//   the user-facing entry;
// - hide distribution helpers (`compgen`, `mangen`) - aimed
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
    /// formatting and comments.  Use `<scope>/<name>@<req>` to add a
    /// registry dependency, `--port <name>` to add a bundled
    /// foundation port, or `--path <dir>` to add a local package.
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
    /// machine-readable form.  Use this for tooling / scripts;
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
    /// linking.  No object files or binaries are produced.  Faster than
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
    /// it.  Arguments after `--` are forwarded verbatim to the
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
    /// resolved registry dependencies.  Triggered
    /// automatically by `cabin build`, `cabin run`, and
    /// `cabin test`; use this command to warm the cache.
    #[command(hide = true)]
    Fetch(FetchArgs),
    /// Vendor external versioned dependencies locally.
    ///
    /// Materializes the selected external registry dependency
    /// closure into a deterministic local file-registry directory
    /// for offline use.  Local path dependencies stay local.
    /// Combine with `--offline --index-path <vendor-dir>` on
    /// subsequent commands.
    #[command(hide = true)]
    Vendor(crate::cli::vendor::VendorArgs),
    /// Display the dependency tree.
    ///
    /// Renders the loaded workspace / local-path dependency
    /// graph as a tree (human or JSON).  Workspace, feature,
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
    /// Publish a package to a registry.
    ///
    /// With `--registry-dir <PATH>`, writes the archive plus
    /// canonical metadata into a local Cabin file registry.  With
    /// `--dry-run` alone, stages the same artifacts under
    /// `--output-dir` without touching any registry.  Behind
    /// `-Z remote-registry`, an HTTP index source (`--index-url` or
    /// the `[registry] index-url` config setting) uploads the same
    /// staged bytes to the registry's API origin.
    Publish(PublishArgs),
    /// Save a registry token for the experimental remote-registry client.
    ///
    /// Requires `-Z remote-registry`.  Resolves the registry from
    /// `--index-url` (or the `[registry] index-url` config setting),
    /// prints where to create a token, reads the token from stdin
    /// (without echo when stdin is a terminal), and stores it in the
    /// user-level `credentials.toml`.
    #[command(hide = true)]
    Login(crate::cli::login::LoginArgs),
    /// Remove the stored registry token for an index origin.
    ///
    /// Requires `-Z remote-registry`.  The counterpart of
    /// `cabin login`; reports whether a token was stored for the
    /// effective registry origin.
    #[command(hide = true)]
    Logout(crate::cli::login::LogoutArgs),
    /// Yank or un-yank a published version on a remote registry.
    ///
    /// Requires `-Z remote-registry`.  Sets the version's yanked
    /// flag through the registry API: a yanked version is excluded
    /// from new resolution, but its archive stays downloadable so
    /// existing lockfiles keep building.  `--undo` clears the flag.
    #[command(hide = true)]
    Yank(crate::cli::yank::YankArgs),
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
    /// wording as `cabin --version`).  With `-v` /
    /// `--verbose`, prints a stable key/value block describing
    /// the release and runtime OS; the OS row is omitted when
    /// unavailable.
    Version(crate::cli::version::VersionArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Package name.  Defaults to the current directory name.
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
    /// Path of the new package directory.  The directory must not already exist.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Package name.  Defaults to the final component of `<PATH>`.
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
    /// Path to the cabin.toml manifest.  May be a single-package manifest
    /// or a workspace root.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Feature selection flags.  Empty by default.  When any
    /// selection flag is passed, `cabin metadata --format json`
    /// adds a `configuration` block to each primary package
    /// describing the resolved configuration.
    #[command(flatten)]
    pub selection: ConfigSelectionArgs,

    /// Workspace package-selection flags.  The metadata view
    /// always reports every loaded package; selection flags only
    /// narrow the `selected_packages` list.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Output format. `human` is a readable summary; `json`
    /// produces a machine-parseable document.  Defaults to `json`
    /// for back-compat with scripts that pipe the metadata output
    /// into `jq`.
    #[arg(long, value_name = "FORMAT", default_value = "json")]
    pub format: ResolveFormat,

    /// Profile to evaluate for the metadata view.  Defaults to
    /// `dev`.  The view always lists every available profile in
    /// the `profiles.available` array regardless of which one is
    /// selected.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Toolchain-selection flags.  Same precedence rules as
    /// `cabin build` so the metadata view reflects exactly the
    /// toolchain a build would use.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.  Manifest `[patch]` tables and
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
    /// profile declared in `[profile.<name>]`).  Defaults to `dev`.
    /// Mutually exclusive with `--release`.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Path to a directory containing the local JSON package index.
    /// Required when the manifest declares any versioned dependencies
    /// and `--index-url` is not given.  Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.  Static sparse HTTP
    /// serving of the file-registry layout is supported
    /// (`<url>/config.json`, `<url>/packages/<scope>/<name>.json`;
    /// a bare local name reads `<url>/packages/<name>.json`).
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`.  Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side effects:
    /// The lockfile must not change and the artifact cache will not be
    /// populated.  Already-cached artifacts may be reused.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access.  Cabin refuses to use an HTTP index URL
    /// (`--index-url` or a `[registry] index-url` config setting) and
    /// expects every needed artifact to be available from a local
    /// index (`--index-path`) or already in the artifact cache.
    /// Combine with `cabin vendor` to consume a self-contained vendor
    /// directory.
    #[arg(long)]
    pub offline: bool,

    /// Enable named features.  May be passed multiple times; values
    /// may also be comma-separated (`--features simd,ssl`).  The
    /// selection applies to the root package being built.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every feature declared by the root package.  Combines
    /// with `--features` (the union is the same as `--all-features`)
    /// and overrides `--no-default-features`.
    #[arg(long)]
    pub all_features: bool,

    /// Disable the package's default features.  Without this flag, the
    /// names listed under `[features].default` are enabled.
    #[arg(long)]
    pub no_default_features: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Toolchain-selection flags.  Each flag (when supplied)
    /// overrides any `CC`/`CXX`/`AR` environment variable and
    /// any `[toolchain]` table in the workspace root manifest.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.  See `docs/patch-overrides.md`.
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
/// `cabin metadata`.  Each flag accepts either a bare command name
/// (`clang++`, resolved against `PATH`) or an explicit path
/// (`/opt/llvm/bin/clang++`).
#[derive(Debug, Args, Default)]
pub(crate) struct ToolchainSelectionArgs {
    /// Override the C compiler.  Accepts a bare command name or a
    /// path.  Highest precedence - also overrides `CC` and
    /// `[toolchain].cc`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub cc: Option<String>,

    /// Override the C++ compiler.  Accepts a bare command name or
    /// a path.  Highest precedence - also overrides `CXX` and
    /// `[toolchain].cxx`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub cxx: Option<String>,

    /// Override the static-library archiver.  Accepts a bare
    /// command name or a path.  Highest precedence - also
    /// overrides `AR` and `[toolchain].ar`.
    #[arg(long, value_name = "PATH-OR-NAME")]
    pub ar: Option<String>,

    /// Select an executable that prefixes every C and C++ compile
    /// command. Accepts a command name or path; `none` disables it.
    /// Highest precedence - also overrides
    /// `CABIN_COMPILER_WRAPPER`, config `[build]`, and the manifest
    /// `[build] compiler-wrapper` declaration.
    /// Mutually exclusive with `--no-compiler-wrapper`.
    #[arg(long, value_name = "WRAPPER", conflicts_with = "no_compiler_wrapper")]
    pub compiler_wrapper: Option<String>,

    /// Disable the compiler wrapper for this invocation,
    /// regardless of any environment variable or manifest
    /// declaration.  Equivalent to `--compiler-wrapper none` but
    /// shorter to type.  Mutually exclusive with
    /// `--compiler-wrapper`.
    #[arg(long)]
    pub no_compiler_wrapper: bool,
}

/// Selection-flag bundle shared by `cabin build` and `cabin metadata`.
#[derive(Debug, Args, Default)]
pub(crate) struct ConfigSelectionArgs {
    /// Enable named features.  May be repeated and/or comma-separated.
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
/// "refresh this direct registry dep" semantic, so this
/// bundle deliberately omits `-p / --package`.  Members can still
/// be scoped by `--workspace`, `--default-members`, and
/// `--exclude`.  Adding a separate long flag (e.g.
/// `--scope-package`) for member-name selection is a deferred
/// improvement.
#[derive(Debug, Args, Default)]
pub(crate) struct WorkspaceSelectionArgsForUpdate {
    /// Operate on every workspace member, then apply `--exclude`.
    #[arg(long, conflicts_with = "default_members")]
    pub workspace: bool,

    /// Operate on `[workspace.default-members]`.  Errors when the
    /// Workspace declares no default-members.
    #[arg(long, conflicts_with = "workspace")]
    pub default_members: bool,

    /// Drop the named package from the selection.  Only valid in
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

    /// Operate on the named workspace package.  Repeat the flag to
    /// select multiple packages.  Errors when a name is not a workspace
    /// member or appears twice in the workspace.
    #[arg(long = "package", short = 'p', value_name = "PACKAGE")]
    pub package: Vec<String>,

    /// Operate on `[workspace.default-members]`.  Errors when the
    /// workspace declares no default-members.
    #[arg(long, conflicts_with_all = &["workspace", "package"])]
    pub default_members: bool,

    /// Drop the named package from the selection.  Only valid in
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
    /// and `--index-url` is not given.  Mutually exclusive with
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

    /// Require an existing, current `cabin.lock`.  Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side effects.
    /// The lockfile is not written and the artifact cache will not be
    /// populated.  Already-cached artifacts may be reused.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access.  Cabin refuses to use an HTTP index
    /// URL (`--index-url` or a `[registry] index-url` config setting)
    /// and expects every needed input to be local or already cached.
    #[arg(long)]
    pub offline: bool,

    /// Output format. `human` is a readable summary; `json` produces a
    /// machine-parseable document.  Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.  See `docs/patch-overrides.md`.
    #[arg(long)]
    pub no_patches: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PackageArgs {
    /// Path to the cabin.toml manifest.  Must point at a single
    /// package; pure-workspace roots are rejected unless the
    /// Workspace selects exactly one member with `--package`.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the generated archive and metadata.
    #[arg(long, default_value = "dist")]
    pub output_dir: PathBuf,

    /// Output format. `human` is a readable summary; `json` produces
    /// a machine-parseable document.  Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags.  In a workspace with
    /// multiple members, `cabin package` requires a single
    /// `--package <name>` selection.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PublishArgs {
    /// Path to the cabin.toml manifest.  Must point at a single
    /// package; pure-workspace roots are rejected.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the dry-run's archive and metadata when
    /// `--registry-dir` is not given.  Defaults to `dist/`.  Mutually
    /// exclusive with `--registry-dir`.
    #[arg(long, value_name = "PATH")]
    pub output_dir: Option<PathBuf>,

    /// Run a publish dry-run only.  With `--registry-dir`, validates
    /// what would happen against the registry without mutating it.
    /// Without `--registry-dir`, runs the staging-only dry-run that
    /// writes the archive + metadata to `--output-dir`.
    #[arg(long)]
    pub dry_run: bool,

    /// Local file-registry root to publish into.  Without
    /// `--dry-run`, the registry is mutated; with `--dry-run`, every
    /// pre-write check runs but the registry is left untouched.
    #[arg(long, value_name = "PATH")]
    pub registry_dir: Option<PathBuf>,

    /// Sparse HTTP index URL of the registry to publish to.  Falls
    /// back to the `[registry] index-url` config setting.  Requires
    /// `-Z remote-registry`; the staged archive and metadata are
    /// uploaded to the API origin the registry's `config.json`
    /// declares.  Combines with `--dry-run` (which stays entirely
    /// local); `--output-dir` then names the staging directory.
    #[arg(long, value_name = "URL", conflicts_with = "registry_dir")]
    pub index_url: Option<String>,

    /// Output format for the publish or dry-run report.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Workspace package-selection flags.  In a workspace with
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
    /// and `--index-url` is not given.  Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Output format. `human` is a readable summary; `json` produces a
    /// machine-parseable document.  Defaults to `human`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Require an existing, current `cabin.lock`.  Resolution is not
    /// allowed to choose any version that differs from the lockfile.
    /// Implies that `cabin.lock` will not be written.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects any state-writing side
    /// effects.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access.  Cabin refuses to use an HTTP index
    /// URL (`--index-url` or a `[registry] index-url` config setting)
    /// and expects every needed input to be local or already cached.
    #[arg(long)]
    pub offline: bool,

    /// Workspace package-selection flags.  The resolver is
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
    /// for this invocation.  See `docs/patch-overrides.md`.
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
    /// and `--index-url` is not given.  Mutually exclusive with
    /// `--index-url`.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    /// Mutually exclusive with `--index-path`.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Update only the named **dependency** (and any of its
    /// transitive deps that must change to satisfy the new
    /// constraints).  Without this flag every locked package is
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

    /// Forbid network access.  Cabin refuses to use an HTTP index
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
    /// for this invocation.  See `docs/patch-overrides.md`.
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

/// Dispatch a parsed CLI invocation.  Returns the exit code the
/// process should propagate.  Most commands return
/// `ExitCode::SUCCESS` on the happy path; `cabin run` forwards
/// the spawned program's exit status so a non-zero exit from the
/// program becomes Cabin's own exit status.
///
/// The `cli.color` field carries the user's `--color` choice;
/// the resolved [`cabin_core::ColorChoice`] for top-level
/// error rendering is computed in `main.rs` against the env
/// and the user-level config.  Subcommands today produce
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
            termcolor::StandardStream::stdout(crate::term_setup::termcolor_choice(color));
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
    // The parsed `-Z` occurrences travel as one typed
    // `ExperimentalFeatures` set so downstream index loading can ask
    // "is remote-registry enabled" without re-parsing argv.  Only
    // the commands that reach an index source consume it today.
    let experimental_features: cabin_core::ExperimentalFeatures =
        cli.unstable.iter().copied().collect();
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
        Command::Build(args) => build(
            &args,
            reporter,
            BuildMode::Build,
            color,
            &experimental_features,
        )
        .map(|()| ExitCode::SUCCESS),
        Command::Check(args) => build(
            &args,
            reporter,
            BuildMode::Check,
            color,
            &experimental_features,
        )
        .map(|()| ExitCode::SUCCESS),
        Command::Clean(args) => clean(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Run(args) => crate::cli::run::run(&args, reporter, color, &experimental_features),
        Command::Test(args) => {
            crate::cli::test::test(&args, reporter, color, &experimental_features)
                .map(|()| ExitCode::SUCCESS)
        }
        Command::Resolve(args) => {
            resolve(&args, reporter, &experimental_features).map(|()| ExitCode::SUCCESS)
        }
        Command::Update(args) => {
            update(&args, reporter, &experimental_features).map(|()| ExitCode::SUCCESS)
        }
        Command::Fetch(args) => {
            fetch(&args, reporter, &experimental_features).map(|()| ExitCode::SUCCESS)
        }
        Command::Vendor(args) => {
            crate::cli::vendor::vendor(&args, reporter, &experimental_features)
                .map(|()| ExitCode::SUCCESS)
        }
        Command::Tree(args) => crate::cli::tree::tree(&args).map(|()| ExitCode::SUCCESS),
        Command::Explain(args) => {
            crate::cli::explain::explain(&args, reporter).map(|()| ExitCode::SUCCESS)
        }
        Command::Package(args) => package(&args, reporter).map(|()| ExitCode::SUCCESS),
        Command::Publish(args) => {
            publish(&args, reporter, &experimental_features).map(|()| ExitCode::SUCCESS)
        }
        Command::Login(args) => crate::cli::login::login(&args, reporter, &experimental_features)
            .map(|()| ExitCode::SUCCESS),
        Command::Logout(args) => crate::cli::login::logout(&args, reporter, &experimental_features)
            .map(|()| ExitCode::SUCCESS),
        Command::Yank(args) => crate::cli::yank::yank(&args, reporter, &experimental_features)
            .map(|()| ExitCode::SUCCESS),
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

/// Resolve the manifest the user is operating on.  When the
/// user did not pass `--manifest-path` (the option is `None`), walk
/// upward from the current directory looking for a workspace root
/// and prefer it.  When the user passed `--manifest-path`
/// explicitly - even with the value `cabin.toml` - the supplied
/// path is honored as-is so callers can intentionally target a
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
/// `cabin_workspace::PackageSelection`.  The mode mirrors the order
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

/// Resolve features for the selected closure.  Roots receive the
/// caller-provided request; non-root reachable packages inherit
/// requests through dependency edges per the documented feature
/// model.  The returned [`cabin_feature::FeatureResolution`] is
/// then threaded into the dependency-iteration helpers so
/// disabled optional dependencies disappear from the resolver /
/// fetch / build planning.
pub(crate) fn compute_feature_resolution(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    request: &cabin_core::SelectionRequest,
    dev_for: &BTreeSet<String>,
) -> Result<cabin_feature::FeatureResolution> {
    let root_request: cabin_feature::RootFeatureRequest = request.into();
    let platform = cabin_core::TargetPlatform::current();
    cabin_feature::resolve_features(
        graph,
        &selection.packages,
        &root_request,
        &platform,
        dev_for,
    )
    .map_err(anyhow::Error::from)
}

/// Flatten a [`cabin_feature::FeatureResolution`] into the
/// per-package enabled-feature map the build planner consumes for
/// `required-features` gating ([`cabin_build::PlanRequest`]'s
/// `enabled_features`).
pub(crate) fn enabled_features_by_package(
    resolution: &cabin_feature::FeatureResolution,
) -> std::collections::HashMap<usize, BTreeSet<String>> {
    resolution
        .per_package
        .iter()
        .map(|(idx, resolved)| (*idx, resolved.enabled_features.clone()))
        .collect()
}

/// Pick the single package manifest path that
/// `cabin package` / `cabin publish` should operate on.  When the
/// invocation manifest is a workspace root, the user must supply
/// exactly one explicit `--package <name>` selection.  Otherwise we
/// honor the existing single-package contract.
/// Result of selecting a single package manifest for a
/// workspace-aware `cabin package` / `cabin publish` invocation.
/// Carries both the manifest path and the pre-resolved `Package`,
/// so member manifests with `dep = { workspace = true }` reach
/// `cabin-package` after the workspace loader has substituted the
/// inherited requirement.
enum SinglePackageSelection {
    /// The user passed a standalone manifest path; `cabin-package`'s
    /// own validator decides what to do with any unresolved
    /// workspace dep it sees.
    Standalone { manifest_path: PathBuf },
    /// The manifest was loaded through a workspace, so
    /// `cabin-workspace` resolved any `workspace = true` deps into
    /// `package`, and the raw `[workspace.<kind>-dependencies]`
    /// strings from the workspace root travel alongside so archive
    /// staging can rewrite dependency `{ workspace = true }` markers
    /// to the author's original requirement spelling.
    WorkspaceMember {
        manifest_path: PathBuf,
        // Boxed: `Package` dwarfs the `Standalone` variant
        // (clippy::large_enum_variant).
        package: Box<cabin_core::Package>,
        workspace_dep_requirements: cabin_core::WorkspaceDepRequirements,
    },
}

impl SinglePackageSelection {
    /// Split into the (manifest path, loader-resolved package,
    /// workspace requirement strings) triple the packaging APIs
    /// consume.  Standalone manifests carry no workspace
    /// requirements, so they contribute an empty table.
    fn into_parts(
        self,
    ) -> (
        PathBuf,
        Option<cabin_core::Package>,
        cabin_core::WorkspaceDepRequirements,
    ) {
        match self {
            Self::Standalone { manifest_path } => (
                manifest_path,
                None,
                cabin_core::WorkspaceDepRequirements::default(),
            ),
            Self::WorkspaceMember {
                manifest_path,
                package,
                workspace_dep_requirements,
            } => (manifest_path, Some(*package), workspace_dep_requirements),
        }
    }
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
        // unchanged.  Reject workspace-selection flags so the user
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
        return Ok(SinglePackageSelection::Standalone {
            manifest_path: invocation.to_path_buf(),
        });
    }
    // The root manifest parse carries the original requirement
    // strings; the loader-resolved `Package` below would respell
    // them through `semver::VersionReq`.
    let mut workspace_dep_requirements = cabin_core::WorkspaceDepRequirements::default();
    if let Some(workspace) = &parsed.workspace {
        for (kind, table) in [
            (cabin_core::DependencyKind::Normal, &workspace.dependencies),
            (cabin_core::DependencyKind::Dev, &workspace.dev_dependencies),
        ] {
            for (name, requirement) in table {
                workspace_dep_requirements.insert(kind, name.clone(), requirement.clone());
            }
        }
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
    Ok(SinglePackageSelection::WorkspaceMember {
        manifest_path: graph.packages[idx].manifest_path.clone(),
        package: Box::new(graph.packages[idx].package.clone()),
        workspace_dep_requirements,
    })
}

/// Resolve the cache directory using --cache-dir,
/// `$CABIN_CACHE_DIR`, or the user-global platform fallback.
///
/// Precedence: `--cache-dir` ▶ `$CABIN_CACHE_DIR` ▶
/// `$CABIN_CACHE_HOME` ▶ the platform base cache directory with a
/// `cabin` suffix (`$XDG_CACHE_HOME/cabin` / `~/.cache/cabin` on
/// Linux and macOS, `%LOCALAPPDATA%\cabin` on Windows).  The
/// fallback shape mirrors `cabin_config::discovery`
/// so the cache home and config home follow the same rule.
///
/// The cache is content-addressed (e.g. foundation-port archives
/// land at `<cache>/ports/archives/sha256/<hex>.tar.gz`), so the
/// user-global default lets two projects on the same machine
/// share a single download.
pub(crate) fn cache_dir_for(override_dir: Option<&Path>) -> Result<PathBuf> {
    use etcetera::{BaseStrategy, choose_base_strategy};
    let user_cache_home = choose_base_strategy()
        .ok()
        .map(|dirs| dirs.cache_dir().join("cabin"));
    cache_dir_for_with_env(
        override_dir,
        &|key| std::env::var_os(key),
        user_cache_home.as_deref(),
    )
}

/// Inner form of [`cache_dir_for`] with the env lookup and the
/// platform user cache home injected so tests can drive the
/// precedence chain without touching real process env.  Production
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
/// the platform user cache home with the `cabin` suffix already
/// applied (see [`cache_dir_for`] for the per-OS shapes).  The
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

    // -------------------------------------------------------------
    // cache_dir_for precedence (XDG user-global default)
    // -------------------------------------------------------------

    type EnvFn = Box<dyn Fn(&str) -> Option<std::ffi::OsString>>;

    /// Build an env-lookup closure backed by a fixed map.  Mirrors
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
        // path (e.g.  HOME and XDG_CACHE_HOME both unset on the host).
        let env = env_with(&[]);
        let err = cache_dir_for_with_env(None, &env, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no cache directory"),
            "expected diagnostic mentioning 'no cache directory', got: {msg}"
        );
    }
}
