//! The repository-automation guard (AGENTS.md, "Repository
//! automation").
//!
//! Repository automation is Rust, in a private `crates/xtask-*` crate
//! reached through a Cargo alias.  This guard keeps a non-Rust script
//! from coming back.  It reads git's index - paths, file modes, and blob
//! ids - and refuses:
//!
//! - a file name that is not a KIND OF FILE THIS REPOSITORY KEEPS - an
//!   allowlist, so `deploy.sh` is refused for the same reason as
//!   `deploy.kt` and `Makefile`, and a template suffix (`deploy.sh.in`)
//!   launders neither;
//! - local action metadata (`action.yml`) and a task runner
//!   (`Taskfile.yml`), which carry an extension this repository does
//!   keep: a local action names the entry point GitHub runs, which need
//!   not look like a script;
//! - the executable bit, which is what makes an extensionless,
//!   shebang-less file runnable as `./tools/deploy`;
//! - an interpreter shebang on the first line;
//! - a symlink, which would give any file a second, unlisted path;
//! - a submodule, whose contents this guard cannot see.
//!
//! Two exact-path lists carve out what exists today, and nothing else
//! does.  `LEGACY_SCRIPTS` is the shrinking migration queue, and each
//! entry pins its blob id: editing a legacy script changes the id and
//! fails the guard, which is what "do not extend them" means
//! mechanically.  `WEBSITE_SCRIPTS` is the site's own npm-driven build
//! tooling, which `website/AGENTS.md` owns; those may evolve freely, so
//! they pin a path only.
//!
//! Exact paths and nothing else, on purpose.  A pattern (`*.sh`,
//! `registry/scripts/**`) would keep covering whatever landed in its
//! place, so the exception could outlive the script it was written for;
//! an exact path stops matching the moment the file is deleted, and this
//! guard reports an exception whose file is gone as a violation of its
//! own.  Migrating a script therefore forces its line here to be deleted
//! in the same change.
//!
//! ponytail: a tracked-content scan, not a sandbox.  Ceilings, stated so
//! nobody mistakes them for coverage: this reads TRACKED FILES, so shell
//! written inside a workflow `run:` block is not scanned at all, and
//! neither is automation smuggled in as one of the kinds of file this
//! repository does keep and then handed to something that runs it - an
//! interpreter argument (`node tools/deploy.json`) or a compiler (`cc
//! tools/deploy.c && ./deploy`), the C kinds being kept because this is
//! a C/C++ package manager whose examples and fixtures are written in
//! them.  What `unreadable_invocation` reads of those blocks is a skim
//! and not a parse, for the same reason the file scan is an allowlist:
//! the wrappers that can stand in front of a command (`rustup run
//! stable rustc …`, `env`, `nice`, `sh -c`) are an open list, so
//! enumerating them buys a little and promises a lot.  AGENTS.md
//! forbids all of it, and until the workflow-block scan
//! lands, review is what enforces that half, with
//! the executable bit and the shebang catching the rest;
//! `website/src/**/*.ts` is website
//! source and is not scanned as tooling; a file name whose extension
//! uses non-ASCII homoglyphs would not match; and content comes
//! from the working tree, so a locally-staged-but-rewritten file reads
//! as its on-disk bytes (CI checks out clean, which is the authority).
//!
//! It does not defend its own invocation, which is the deliberate one.
//! `PINNED_TRIGGERS` keeps `rust.yml` running on every pull request and
//! the check below keeps its `run:` line, but an `if:` on the job, a
//! `continue-on-error:`, or a `CARGO_ALIAS_CHECK_SCRIPTS` in the
//! environment all leave the guard green having run nothing.  Pinning
//! the job verbatim caught those and cost more to read than the rule it
//! protects; no other job here is pinned that way either, so a change
//! that switches this one off is as visible in review as one that
//! switches off the tests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// Every kind of file this repository keeps, by extension.
///
/// An ALLOWLIST, and that direction is the whole point.  A list of
/// scripting languages is open - it grows with every language anyone
/// ever writes a tool in, and the one nobody listed is the one that gets
/// through.  The kinds of file a repository keeps are closed: this tree
/// has about thirty and gains one only when a change introduces a
/// genuinely new kind, which is exactly when a reviewer should be
/// deciding whether that kind is data or a tool.  So `.sh` is refused
/// for the same reason `.kt` is - not because either was foreseen, but
/// because neither is a kind of file this repository keeps.
///
/// Checked against every extension-shaped component after the first, so
/// a template (`deploy.sh.in`) cannot launder one.
const DATA_EXTENSIONS: [&str; 28] = [
    "astro",
    "c",
    "cc",
    "cpp",
    "css",
    "dockerignore",
    "gitignore",
    "h",
    "hpp",
    "ico",
    "json",
    "jsonc",
    "lean",
    "lock",
    "md",
    "node-version",
    "png",
    // A verbatim upstream file in the fake-libpng test fixture.
    "prebuilt",
    "rs",
    "sql",
    "svg",
    // VHS recording script for the README demo - declarative, and named
    // as not-automation by AGENTS.md.
    "tape",
    "toml",
    "txt",
    "webmanifest",
    "yaml",
    "yml",
    "zip",
];

/// Name components this repository writes BEFORE the extension
/// (`env.d.ts`, `astro.config.ts`, `avatar.test.ts`).  Allowed only in
/// that position: a component is a kind of file only when it is last.
const INFIX_COMPONENTS: [&str; 3] = ["config", "d", "test"];

/// The extensionless files this repository keeps.  Nothing else may be
/// extensionless: that is what refuses `Makefile`, `justfile` and
/// `Rakefile` without naming any of them.
const ALLOWED_NAMES: [&str; 6] = [
    "Dockerfile",
    "LICENSE",
    "_headers",
    "_redirects",
    "lean-toolchain",
    "migrations-applied",
];

/// Languages this repository writes BOTH product source and tooling in.
/// Outside a product-source root they are tooling like any other; inside
/// one they are the website's own source.
const SOURCE_LANGUAGE_EXTENSIONS: [&str; 7] = ["cjs", "cts", "js", "mjs", "mts", "ts", "tsx"];

/// Where this repository keeps product source written in a language it
/// also writes tooling in.  This is the RULE'S DOMAIN, not a tolerated
/// violation: `website/src/` is the site itself, which
/// `website/AGENTS.md` owns.  Kept to exact prefixes, checked to be
/// non-empty in the tree, and deliberately tiny - it buys back 46
/// TypeScript files that would otherwise each need an exception.
const PRODUCT_SOURCE_ROOTS: [&str; 1] = ["website/src/"];

/// The only names the allowlist above cannot refuse on its own: tools
/// that wear an extension this repository keeps.
///
/// A local action is repository automation by definition, and its
/// `runs:` names the entry point GitHub executes - a file that need not
/// look like a script at all (`main: deploy.data`). Refusing the
/// metadata refuses the whole shape, which is why nothing here has to
/// guess what an entry point is. Everything else that used to be listed
/// here - `Makefile`, `justfile`, `.envrc` - is refused for having no
/// kind at all.
const TOOLING_NAMES: [&str; 3] = ["Taskfile.yml", "action.yaml", "action.yml"];

/// Shell tooling that predates the `xtask-*` convention, by exact path,
/// each with the crate that will absorb it and the index mode and git
/// blob id of the file this exception was written for.
///
/// Sorted, and checked to be sorted, so the list reads as a shrinking
/// work queue. Deleting a script means deleting its line here in the
/// same change: [`check`] reports an entry whose file is no longer
/// tracked. Editing one means re-pinning its blob id in the same change,
/// which is a reviewer's decision, not a drive-by.
const LEGACY_SCRIPTS: [(&str, &str, &str, &str); 12] = [
    (
        "registry/scripts/backup-audit.sh",
        "xtask-registry-admin",
        "100755",
        "2410c2b00296777f7c3ac74b81e8a866d0f43935",
    ),
    (
        "registry/scripts/backup-backfill.sh",
        "xtask-registry-admin",
        "100755",
        "1400197d167b70d6c03dda29d301afba423a099f",
    ),
    (
        "registry/scripts/diagnose.sh",
        "xtask-registry-admin",
        "100755",
        "7da2f57e8b7bd7ec79d6c5a8e640a1be7fd4fef0",
    ),
    (
        "registry/scripts/gen-fixtures.sh",
        "xtask-registry-test",
        "100755",
        "a64402c0cdc34e5f72467f35fadab4714d6963c3",
    ),
    (
        "registry/scripts/governor.sh",
        "xtask-registry-admin",
        "100755",
        "e8111fa8ac3757abd00b6d8454232eb9da433c17",
    ),
    (
        "registry/scripts/launch-guard.sh",
        "xtask-registry-admin",
        "100755",
        "baa616cf12db3572f71505c419cd922775e1b7b5",
    ),
    (
        "registry/scripts/lib.sh",
        "xtask-registry-admin",
        "100644",
        "fa9e7df8fca8dcb4b6f45f0fd2e03e3ea3f9d0a1",
    ),
    (
        "registry/scripts/migrate.sh",
        "xtask-registry-admin",
        "100755",
        "61a357a4a21400b9a19a6a6851fc2afd3c43f0de",
    ),
    (
        "registry/scripts/restore-drill.sh",
        "xtask-registry-admin",
        "100755",
        "66d71638bcf71b1d46789aa120b5d146a867e381",
    ),
    (
        "registry/scripts/smoke.sh",
        "xtask-registry-test",
        "100755",
        "3bdcd587db7b58e4c82793cc038808bf6ac2b731",
    ),
    (
        "registry/scripts/wipe.sh",
        "xtask-registry-admin",
        "100755",
        "9a5745d975d905b4d969f9996a1eaffa85b62ad2",
    ),
    (
        "scripts/ci.sh",
        "xtask-ci",
        "100755",
        "573aa5bad6ca2aacfadeddc96c50d36b16f9c1c1",
    ),
];

/// The website's own build-time checks, run through npm by
/// `website.yml` and owned by `website/AGENTS.md`.
///
/// Not the migration queue: these are website tooling, they may change
/// freely, and they pin a path rather than a blob id. They are listed
/// one by one anyway - the alternative is a `website/**` pattern, which
/// is exactly the shape this guard refuses to have.
const WEBSITE_SCRIPTS: [&str; 6] = [
    "website/astro.config.ts",
    "website/scripts/lib/find-html-files.mjs",
    "website/scripts/verify-docs-links.mjs",
    "website/scripts/verify-no-inline-scripts.mjs",
    "website/scripts/verify-progressive-independence.mjs",
    "website/scripts/verify-progressive-independence.test.mjs",
];

/// Every alias, with the command it must be, verbatim.
///
/// The package an alias selects is not the whole of what it does: the
/// same crate reached with `-- --help` exits zero having checked
/// nothing, and a job that runs it stays green. Pinning the command
/// makes any edit to one - a retarget, an added flag, a changed
/// subcommand - a reviewer's decision, the way editing a legacy script
/// is, and a new alias has to be added here to exist at all.
const PINNED_ALIASES: [(&str, &str); 5] = [
    (
        "check-deploy",
        "run --quiet --locked -p xtask-registry-guard -- check-deploy",
    ),
    (
        "check-r2",
        "run --quiet --locked -p xtask-registry-guard -- check-r2",
    ),
    (
        "check-scripts",
        "run --quiet --locked -p xtask-ci -- check-scripts",
    ),
    (
        "check-sql",
        "run --quiet --locked -p xtask-registry-guard -- check-sql",
    ),
    (
        "port-publish",
        "run --quiet --locked -p xtask-port-publish --",
    ),
];

/// The repository's one cargo config, holding the aliases.
const CARGO_CONFIG: &str = ".cargo/config.toml";

/// The workflow that must run this guard, and the command that does it.
const GUARD_WORKFLOW: &str = ".github/workflows/rust.yml";
const GUARD_COMMAND: &str = "cargo check-scripts";

/// The trigger block `rust.yml` must carry, verbatim: no `paths:` or
/// `paths-ignore:` filter, and `pull_request` present, so no change can
/// reach main without this guard having run.  The pin runs THROUGH the
/// next key, so a filter appended after the block cannot hide behind a
/// still-matching prefix.
///
/// It carries the workflow-level `env:` block for a second reason: those
/// variables reach the guard's steps, and a shell reads some of them
/// before it reads the script it was handed.  `BASH_ENV` naming a
/// tracked file that says `exit 0` would leave both steps green without
/// building or running anything.
const PINNED_TRIGGERS: &str = "on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: \"-D warnings\"

permissions:";

/// Every other workflow that runs an alias, with the trigger block it
/// must carry verbatim.
///
/// The alias file and the tool's crate have to stay in these filters, or
/// editing either one stops reaching the job it feeds. Whether they
/// still do is not something a scan can decide: GitHub evaluates an
/// ordered pattern list, where a later `!crates/<tool>/**` cancels an
/// earlier entry, and the whole block can be written in flow style.
/// Pinning the text sidesteps the semantics - any edit here is a
/// conscious re-pin, and re-pinning is where a reviewer checks the two
/// inputs are still covered.
const PINNED_CONSUMERS: [(&str, &[&str], &str); 2] = [
    (
        ".github/workflows/ports-publish.yml",
        &[
            // The publish job builds the tool directly as well as
            // reaching it through the alias.
            "direct=xtask-port-publish",
            "port-publish=xtask-port-publish",
        ],
        r#"on:
  push:
    branches: [main]
    paths:
      - ".github/workflows/ports-publish.yml"
      # The publish steps run the `cargo port-publish` alias.
      - ".cargo/config.toml"
      # Cargo.lock / root Cargo.toml: the tool builds --locked, and
      # lock-only bumps of byte-producing dependencies (zip, flate2)
      # or workspace-manifest feature flips (which need no lockfile
      # change) change the archive bytes the registry holds immutable.
      - "Cargo.lock"
      - "Cargo.toml"
      - "ports/**"
      - "crates/cabin-artifact/**"
      - "crates/xtask-port-publish/**"
      - "crates/cabin-core/**"
      - "crates/cabin-manifest/**"
      - "crates/cabin-package/**"
      - "crates/cabin-publish/**"
      - "crates/cabin-registry-api/**"
  pull_request:
    paths:
      - ".github/workflows/ports-publish.yml"
      # The publish steps run the `cargo port-publish` alias.
      - ".cargo/config.toml"
      # Cargo.lock / root Cargo.toml: the tool builds --locked, and
      # lock-only bumps of byte-producing dependencies (zip, flate2)
      # or workspace-manifest feature flips (which need no lockfile
      # change) change the archive bytes the registry holds immutable.
      - "Cargo.lock"
      - "Cargo.toml"
      - "ports/**"
      - "crates/cabin-artifact/**"
      - "crates/xtask-port-publish/**"
      - "crates/cabin-core/**"
      - "crates/cabin-manifest/**"
      - "crates/cabin-package/**"
      - "crates/cabin-publish/**"
      - "crates/cabin-registry-api/**"
  workflow_dispatch:

permissions:"#,
    ),
    (
        ".github/workflows/registry.yml",
        &[
            "check-deploy=xtask-registry-guard",
            "check-r2=xtask-registry-guard",
            "check-sql=xtask-registry-guard",
            // The deploy job names the guard's crate directly as well.
            "direct=xtask-registry-guard",
        ],
        r#"on:
  push:
    branches: [ main ]
    paths:
      - ".github/workflows/registry.yml"
      # On the config search path of the registry workspace too.
      - ".cargo/config.toml"
      - "registry/**"
      # The guard the build job runs, plus the root manifest that
      # keeps it a workspace member: dropping it there would remove the
      # guard and its tests from `cargo test --workspace` at the same
      # time, and nothing else here would notice. Cargo.lock is
      # deliberately not listed - it moves on every dependency bump and
      # cannot remove a member on its own.
      - "crates/xtask-registry-guard/**"
      - "Cargo.toml"
      - "crates/cabin-package/**"
      - "crates/cabin-publish/**"
      - "crates/cabin-registry-api/**"
      - "crates/cabin-core/**"
  pull_request:
    paths:
      - ".github/workflows/registry.yml"
      # On the config search path of the registry workspace too.
      - ".cargo/config.toml"
      - "registry/**"
      # The guard the build job runs, plus the root manifest that
      # keeps it a workspace member: dropping it there would remove the
      # guard and its tests from `cargo test --workspace` at the same
      # time, and nothing else here would notice. Cargo.lock is
      # deliberately not listed - it moves on every dependency bump and
      # cannot remove a member on its own.
      - "crates/xtask-registry-guard/**"
      - "Cargo.toml"
      - "crates/cabin-package/**"
      - "crates/cabin-publish/**"
      - "crates/cabin-registry-api/**"
      - "crates/cabin-core/**"

permissions:"#,
    ),
];

/// One entry of git's index.
struct Entry {
    mode: String,
    oid: String,
    path: String,
}

/// Every violation under `repo_root`, as the diagnostic lines the guard
/// prints.  An empty result means the guard accepted the tree.
///
/// # Errors
///
/// Fails when `git ls-files` cannot be run or does not succeed - the
/// guard polices committed content, so an unusable index is a refusal,
/// never an empty report.
pub fn check(repo_root: &Path) -> Result<Vec<String>> {
    let entries = index_entries(repo_root)?;
    let mut violations = exception_list_problems();

    let website: BTreeSet<&str> = WEBSITE_SCRIPTS.into_iter().collect();

    for entry in &entries {
        let path = entry.path.as_str();
        // One cargo config, at the root, alias-only and checked below.
        // Cargo READS several: it prefers the extensionless name to
        // `config.toml`, and it walks up from wherever it was invoked,
        // so a second file anywhere would be the aliases (or a runner)
        // this guard never looked at.
        // A cargo manifest outside `crates/<name>/` is a package nothing
        // that checks a tool would look at: the scan that finds them
        // reads that shape, and so does the location rule. `crates/`
        // itself is one of those places - a package rooted there has no
        // crate directory to be named after. The root's own manifest and
        // the standalone `registry/` workspace are the tree's two, and
        // their nested crates live under those roots.
        if path.rsplit('/').next() == Some("Cargo.toml")
            && (!path.starts_with("crates/") || path == "crates/Cargo.toml")
            && path != "registry/Cargo.toml"
            && path != "Cargo.toml"
        {
            violations.push(format!(
                "{path} is a cargo manifest outside crates/<name>/; a package here is a crate \
                 the tool checks never see, and repository automation is a crates/xtask-* \
                 crate (AGENTS.md, \"Repository automation\")"
            ));
            continue;
        }
        if is_cargo_config(path) && path != CARGO_CONFIG {
            violations.push(format!(
                "{path} is a second cargo config; cargo prefers `config` to `config.toml` and \
                 reads one per directory on the way up, so only {CARGO_CONFIG} may exist \
                 (AGENTS.md, \"Repository automation\")"
            ));
            continue;
        }
        if let Some((_, owner, mode, pinned)) =
            LEGACY_SCRIPTS.iter().find(|(known, ..)| *known == path)
        {
            // The mode is pinned alongside the content: `chmod +x` keeps
            // the blob id, and an executable file is a different thing.
            if entry.oid != *pinned || entry.mode != *mode {
                violations.push(format!(
                    "{path} changed while it is still an exception ({owner} awaits it); \
                     do not extend a legacy script - if the edit is part of migrating it, \
                     re-pin the blob id in LEGACY_SCRIPTS \
                     (crates/xtask-ci/src/scripts.rs) in the same change"
                ));
            }
            continue;
        }
        if website.contains(path) {
            // Path-pinned, not content-pinned: what these files say is
            // the website's business. What they *are* is not - a
            // symlink here would hand an excepted script a second path,
            // and the exception is for a checked-in npm script.
            violations.extend(mode_problem(entry));
            continue;
        }
        violations.extend(entry_problem(repo_root, entry));
    }

    // A stale exception is a rule that stopped binding: the file it
    // named is gone, so nothing is being tolerated any more.
    let tracked: BTreeSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    let excepted = LEGACY_SCRIPTS
        .iter()
        .map(|(path, ..)| *path)
        .chain(WEBSITE_SCRIPTS);
    for path in excepted {
        if !tracked.contains(path) {
            violations.push(format!(
                "{path} is no longer tracked; delete its exception in \
                 crates/xtask-ci/src/scripts.rs"
            ));
        }
    }

    for root in PRODUCT_SOURCE_ROOTS {
        if !tracked.iter().any(|path| path.starts_with(root)) {
            violations.push(format!(
                "{root} is declared a product-source root but tracks nothing; \
                 delete it from PRODUCT_SOURCE_ROOTS in crates/xtask-ci/src/scripts.rs"
            ));
        }
    }

    violations.extend(cargo_config_problems(repo_root)?);
    violations.extend(xtask_crate_problems(repo_root)?);
    violations.extend(shipped_dependency_problems(repo_root)?);
    violations.extend(workflow_wiring_problems(repo_root));
    violations.extend(alias_consumer_problems(repo_root)?);
    Ok(violations)
}

/// The product-source roots the rule's domain carves out.
#[must_use]
pub fn source_roots() -> Vec<&'static str> {
    PRODUCT_SOURCE_ROOTS.to_vec()
}

/// Every package the repository's aliases select.
///
/// # Errors
///
/// Fails when `.cargo/config.toml` cannot be read or parsed.
pub fn aliased_packages(repo_root: &Path) -> Result<Vec<String>> {
    let config = manifest(&repo_root.join(CARGO_CONFIG))?;
    Ok(aliases(&config)
        .iter()
        .filter_map(|(_, value)| alias_package(value).map(ToOwned::to_owned))
        .collect())
}

/// Every workflow whose trigger block the guard pins, the guard's own
/// included.
#[must_use]
pub fn pinned_workflows() -> Vec<&'static str> {
    std::iter::once(GUARD_WORKFLOW)
        .chain(PINNED_CONSUMERS.iter().map(|(path, ..)| *path))
        .collect()
}

/// Every path the guard excepts today, from both lists.
#[must_use]
pub fn exceptions() -> Vec<&'static str> {
    LEGACY_SCRIPTS
        .iter()
        .map(|(path, ..)| *path)
        .chain(WEBSITE_SCRIPTS)
        .collect()
}

/// The legacy scripts still awaiting migration, with the crate that will
/// absorb each.
#[must_use]
pub fn pending() -> Vec<(&'static str, &'static str)> {
    LEGACY_SCRIPTS
        .iter()
        .map(|(path, owner, ..)| (*path, *owner))
        .collect()
}

/// Whatever is wrong with the exception lists themselves.
fn exception_list_problems() -> Vec<String> {
    let mut problems = Vec::new();
    if !PRODUCT_SOURCE_ROOTS
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        problems.push("PRODUCT_SOURCE_ROOTS must stay sorted and free of duplicates".to_owned());
    }
    for root in PRODUCT_SOURCE_ROOTS {
        if !root.ends_with('/') {
            problems.push(format!(
                "PRODUCT_SOURCE_ROOTS entry {root} must end in / so it cannot match a sibling"
            ));
        }
    }
    let lists = [
        (
            "LEGACY_SCRIPTS",
            LEGACY_SCRIPTS
                .iter()
                .map(|(path, ..)| *path)
                .collect::<Vec<_>>(),
        ),
        ("WEBSITE_SCRIPTS", WEBSITE_SCRIPTS.to_vec()),
    ];
    for (name, paths) in lists {
        if !paths.windows(2).all(|pair| pair[0] < pair[1]) {
            problems.push(format!("{name} must stay sorted and free of duplicates"));
        }
        for path in paths {
            if path.contains(['*', '?', '[']) {
                problems.push(format!(
                    "{name} entry {path} is a pattern, not an exact path"
                ));
            }
        }
    }
    problems
}

/// What to do instead, appended to the diagnostics that name a file the
/// guard will not have.
const REMEDY: &str = "write it as a crates/xtask-* command with a cargo alias \
                      (AGENTS.md, \"Repository automation\")";

/// Whatever makes one index entry repository automation.
fn entry_problem(repo_root: &Path, entry: &Entry) -> Option<String> {
    let path = entry.path.as_str();
    if let Some(reason) = name_problem(path) {
        return Some(format!("{path}: {reason}; {REMEDY}"));
    }
    if let Some(problem) = mode_problem(entry) {
        return Some(problem);
    }
    match first_line_shebang(&repo_root.join(path)) {
        Ok(true) => Some(format!(
            "{path}: an interpreter shebang makes this repository automation; {REMEDY}"
        )),
        Ok(false) => None,
        Err(err) => Some(format!(
            "{path}: tracked but not readable, so the guard cannot clear it ({err:#})"
        )),
    }
}

/// Whether cargo would read `path` as configuration: either name, in any
/// `.cargo/` directory.
///
/// Case-insensitively, because Windows and macOS resolve `.Cargo/Config`
/// to the path cargo looks for while git records the name as typed.
fn is_cargo_config(path: &str) -> bool {
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or_default();
    ["config", "config.toml"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
        && parts
            .next()
            .is_some_and(|parent| parent.eq_ignore_ascii_case(".cargo"))
}

/// Whatever makes one index entry repository automation by its kind
/// alone - which an exception for a file does not excuse, because every
/// exception was written for a checked-in regular file.
fn mode_problem(entry: &Entry) -> Option<String> {
    let path = entry.path.as_str();
    match entry.mode.as_str() {
        // A gitlink hides a whole tree from this scan.
        "160000" => Some(format!(
            "{path}: a submodule's contents cannot be inspected by this guard; \
             vendor what you need instead (AGENTS.md, \"Repository automation\")"
        )),
        // A symlink aliases whatever it points at - including an
        // excepted script, which would give that exception a second,
        // unlisted path. There are none in this tree, so refusing
        // outright costs nothing and bounds what an exception covers.
        "120000" => Some(format!(
            "{path}: a tracked symlink can alias any file, including an excepted one, \
             giving it a second path this guard does not list; {REMEDY}"
        )),
        "100755" => Some(format!(
            "{path}: the executable bit makes this runnable as a script, \
             whatever its name; {REMEDY}"
        )),
        _ => None,
    }
}

/// Whether `component` names a kind of file at all.  A version
/// (`smoke-withdep-0.2.0.json`) and a route parameter
/// (`[...slug].astro`) both split into components no reader would call
/// an extension, and no interpreter is selected by one.
fn is_extension_shaped(component: &str) -> bool {
    component.starts_with(|first: char| first.is_ascii_alphabetic())
        && component
            .chars()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == '-')
}

/// Why `path`'s name alone makes it a tool, if it does.
fn name_problem(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?;
    // The name itself, or the name a template suffix is hiding:
    // `make -f Taskfile.yml.in` runs it as readily as the plain name.
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    if let Some(known) = TOOLING_NAMES
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(name) || known.eq_ignore_ascii_case(stem))
    {
        return Some(format!("{known} is repository automation"));
    }
    let in_source_root = PRODUCT_SOURCE_ROOTS
        .into_iter()
        .any(|root| path.starts_with(root));
    let kinds: Vec<&str> = name
        .split('.')
        .skip(1)
        .filter(|component| is_extension_shaped(component))
        .collect();
    let Some((last, infixes)) = kinds.split_last() else {
        if ALLOWED_NAMES.contains(&name) {
            return None;
        }
        return Some(format!(
            "{name} has no extension, and is not one of the extensionless files this \
             repository keeps; an extensionless tracked file is a tool unless \
             ALLOWED_NAMES in crates/xtask-ci/src/scripts.rs says otherwise"
        ));
    };
    // Every component, not just the last: `deploy.sh.md` would otherwise
    // read as markdown.
    for (component, allow_infix) in infixes
        .iter()
        .map(|infix| (*infix, true))
        .chain([(*last, false)])
    {
        let known = DATA_EXTENSIONS
            .into_iter()
            .chain(
                in_source_root
                    .then_some(SOURCE_LANGUAGE_EXTENSIONS)
                    .into_iter()
                    .flatten(),
            )
            .chain(
                allow_infix
                    .then_some(INFIX_COMPONENTS)
                    .into_iter()
                    .flatten(),
            )
            .any(|known| known.eq_ignore_ascii_case(component));
        if known {
            continue;
        }
        return Some(
            if SOURCE_LANGUAGE_EXTENSIONS
                .into_iter()
                .any(|source| source.eq_ignore_ascii_case(component))
            {
                format!(
                    ".{component} outside {} is repository automation, not source",
                    PRODUCT_SOURCE_ROOTS.join(" or ")
                )
            } else {
                format!(
                    ".{component} is not a kind of file this repository keeps; \
                     if it is data or source, add it to DATA_EXTENSIONS in \
                     crates/xtask-ci/src/scripts.rs"
                )
            },
        );
    }
    None
}

/// Every entry of git's index under `repo_root`.
fn index_entries(repo_root: &Path) -> Result<Vec<Entry>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-files", "--stage", "-z"])
        .output()
        .with_context(|| format!("run git ls-files in {}", repo_root.display()))?;
    if !output.status.success() {
        bail!(
            "git ls-files failed in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| anyhow::anyhow!("git tracks a path that is not valid UTF-8"))?;
    text.split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            // `<mode> <oid> <stage>\t<path>`
            let (meta, path) = record
                .split_once('\t')
                .ok_or_else(|| anyhow::anyhow!("unexpected git ls-files record: {record}"))?;
            let mut fields = meta.split_whitespace();
            let mode = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("no mode in: {record}"))?;
            let oid = fields
                .next()
                .ok_or_else(|| anyhow::anyhow!("no object id in: {record}"))?;
            Ok(Entry {
                mode: mode.to_owned(),
                oid: oid.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

/// Whether the file's FIRST line is an interpreter shebang.
///
/// `#!` alone is not enough: Rust's inner attributes (`#![cfg(unix)]`)
/// open source files in this repository the same way, and they are the
/// one shape that must not be flagged. Everything else on that first
/// line - `#!/bin/sh`, `#! /bin/sh`, `#!bash` - is a shebang, including
/// the relative-interpreter form the kernel rejects but `bash file`
/// honors.
fn first_line_shebang(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    // The inner-attribute spelling is only Rust's in a Rust file: every
    // shell treats `#![ignored]` as a comment and runs the lines under
    // it, so the exemption stops at the extension.
    let rust = path.extension().is_some_and(|kind| kind == "rs");
    // A byte-order mark before `#!` stops the kernel but not a human
    // running `sh file`.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let Some(after) = bytes.strip_prefix(b"#!") else {
        return Ok(false);
    };
    let line = after
        .split(|&byte| byte == b'\n')
        .next()
        .unwrap_or_default();
    // `#! [allow(dead_code)]` is an inner attribute too: the space is
    // legal Rust, and only what follows it tells the two apart.
    let interpreter = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |start| &line[start..]);
    let attribute = rust && interpreter.starts_with(b"[");
    Ok(!interpreter.is_empty() && !attribute)
}

/// Whatever is wrong with `.cargo/config.toml` beyond the aliases.
///
/// The file claims to be `[alias]`-only, and that claim is load-bearing
/// twice over: a `[build]` or `[target]` key here also reaches the
/// standalone `registry/` workspace, and a `[target.<cfg>] runner` would
/// make `cargo run` execute something else entirely - so the CI job
/// could report success having run `true` instead of this guard.
fn cargo_config_problems(repo_root: &Path) -> Result<Vec<String>> {
    let path = repo_root.join(CARGO_CONFIG);
    if !path.is_file() {
        return Ok(vec![
            ".cargo/config.toml is missing; it is where the aliases live".to_owned(),
        ]);
    }
    let config = manifest(&path)?;
    let Some(table) = config.as_table() else {
        return Ok(vec![".cargo/config.toml is not a table".to_owned()]);
    };
    let mut problems: Vec<String> = table
        .keys()
        .filter(|key| key.as_str() != "alias")
        .map(|key| {
            format!(
                ".cargo/config.toml carries a [{key}] section; it must stay [alias]-only \
                 (a runner or build key would change what `cargo run` executes, this guard \
                 included, and reaches the registry workspace as well)"
            )
        })
        .collect();
    // Checked from the alias side as well as the crate side: the
    // crates/xtask-* scan can only see crates that already sit there, so
    // on its own it would let a tool land as any other package with an
    // alias pointed at it.
    let empty = toml::value::Table::new();
    let declared = table
        .get("alias")
        .and_then(toml::Value::as_table)
        .unwrap_or(&empty);
    for (name, value) in declared {
        let Some(text) = value.as_str() else {
            problems.push(format!(
                "the `cargo {name}` alias is not a string; cargo JOINS array config values \
                 across layers, so an array here would become the ARGUMENTS of a same-named \
                 alias in a developer's ~/.cargo/config.toml rather than the command"
            ));
            continue;
        };
        // `crates/<name>` is only the right place to look while the
        // alias resolves against the root workspace; another manifest
        // can hold a package of the same name.
        // An alias may not move the ground the rest of these checks
        // stand on: `--manifest-path` resolves the package in another
        // workspace, where the same name is another package, `--config`
        // can set the `[target] runner` the alias file is kept free of,
        // and `--example`/`--bin` run a target of the package that is
        // not its tool - each leaves the crate named here standing for
        // something that never runs.
        for flag in text
            .split_whitespace()
            .take_while(|word| *word != "--")
            .filter(|word| {
                matches!(
                    word.split('=').next(),
                    Some(
                        "--manifest-path"
                            | "--config"
                            | "--bin"
                            | "--example"
                            | "--test"
                            | "--bench"
                    )
                )
            })
        {
            let flag = flag.split('=').next().unwrap_or(flag);
            problems.push(format!(
                "the `cargo {name}` alias passes {flag}; an alias runs its crate's own tool, \
                 against the root workspace and the alias-only {CARGO_CONFIG}, and this moves \
                 one of those"
            ));
        }
        match PINNED_ALIASES.iter().find(|(known, _)| known == name) {
            Some((_, pinned)) if pinned == &text => {}
            Some((_, pinned)) => problems.push(format!(
                "the `cargo {name}` alias is `{text}`, but it is pinned as `{pinned}`; \
                 check that it still RUNS the tool rather than exiting zero past it, then \
                 re-pin PINNED_ALIASES in crates/xtask-ci/src/scripts.rs"
            )),
            None => problems.push(format!(
                "the `cargo {name}` alias is not pinned; add it to PINNED_ALIASES in \
                 crates/xtask-ci/src/scripts.rs, so that what it runs is read once by a \
                 reviewer and cannot drift after"
            )),
        }
        // The subcommand AND the name AND the place AND the manifest: an
        // alias that builds a tool without running it still exits zero,
        // and a package may be called anything wherever it sits, so any
        // one of the four on its own would let a tool live where the
        // crate scan never looks, or leave an alias that runs nothing.
        let reachable = text.split_whitespace().next() == Some("run")
            && alias_package(text).is_some_and(|package| {
                package.starts_with("xtask-")
                    && package_named(&repo_root.join("crates").join(package), package)
            });
        if !reachable {
            problems.push(format!(
                "the `cargo {name}` alias does not RUN a crates/xtask-* package \
                 (`run ... -p xtask-<name>`, declared by crates/xtask-<name>/Cargo.toml); \
                 repository automation is an xtask crate reached through an alias, and an \
                 alias that runs anything else - or merely builds it - is that rule routed \
                 around (AGENTS.md, \"Repository automation\")"
            ));
        }
    }
    Ok(problems)
}

/// Whatever would compile a tool into what ships.
///
/// `publish = false` keeps an xtask crate off the registry; it does not
/// keep it out of the binary. A normal or build dependency on one from
/// any other crate puts it in the product's graph, which is what "never
/// part of the shipped `cabin` binary" means. A DEV dependency does not:
/// it is test-only, and `cargo publish` strips a version-less path one,
/// which is how `crates/cabin` reaches the publisher to build its
/// registry fixtures.
///
/// One direct edge is enough to look for: for a tool to be reachable
/// from the binary, SOME shipped crate has to name it.
fn shipped_dependency_problems(repo_root: &Path) -> Result<Vec<String>> {
    let crates = repo_root.join("crates");
    if !crates.is_dir() {
        return Ok(Vec::new());
    }
    // `dep.workspace = true` carries no package name of its own: the
    // root table holds it, rename included.
    let root = manifest(&repo_root.join("Cargo.toml"))?;
    let inherited = root
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table);
    // Every crate, at any depth, that is not itself a tool: a shipped
    // one nested under `crates/libs/` reaches the binary the same way.
    let dirs: Vec<String> = workspace_crates(repo_root)?
        .into_iter()
        .filter(|(_, package, _)| !package.starts_with("xtask-"))
        .map(|(dir, ..)| dir)
        .collect();

    let mut problems = Vec::new();
    for dir in dirs {
        let manifest = manifest(&crates.join(&dir).join("Cargo.toml"))?;
        for (kind, table) in dependency_tables(&manifest) {
            // The table key is the name the crate refers to it by; a
            // `package = ` field - here or in the workspace table this
            // one inherits from - says what it actually depends on.
            let named = table.iter().map(|(key, spec)| {
                let inherited = spec
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or_default()
                    .then(|| inherited.and_then(|table| table.get(key)))
                    .flatten();
                spec.get("package")
                    .or_else(|| inherited.and_then(|spec| spec.get("package")))
                    .and_then(toml::Value::as_str)
                    .unwrap_or(key.as_str())
            });
            for tool in named.filter(|name| name.starts_with("xtask-")) {
                problems.push(format!(
                    "crates/{dir} carries {tool} as a {kind}; an xtask crate is maintainer \
                     tooling and never part of what ships (a dev-dependency is fine - it is \
                     test-only, and cargo publish strips a version-less path one)"
                ));
            }
        }
    }
    Ok(problems)
}

/// The dependency tables of a manifest that reach the built artifact,
/// per-target ones included, with the name each goes by.
fn dependency_tables(manifest: &toml::Value) -> Vec<(&'static str, &toml::value::Table)> {
    let kinds = ["dependencies", "build-dependencies"];
    let direct = kinds.into_iter().filter_map(move |kind| {
        manifest
            .get(kind)
            .and_then(toml::Value::as_table)
            .map(|table| (kind, table))
    });
    let per_target = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(move |targets| {
            targets.values().flat_map(move |target| {
                kinds.into_iter().filter_map(move |kind| {
                    target
                        .get(kind)
                        .and_then(toml::Value::as_table)
                        .map(|table| (kind, table))
                })
            })
        });
    direct.chain(per_target).collect()
}

/// Whether the crate at `dir` declares itself to be `name`.
fn package_named(dir: &Path, name: &str) -> bool {
    manifest(&dir.join("Cargo.toml")).is_ok_and(|manifest| {
        manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            == Some(name)
    })
}

/// Every alias in a parsed `.cargo/config.toml`, with its value flattened
/// to one string: cargo accepts a whitespace-split scalar or an array of
/// words, and both mean the same command line.
fn aliases(config: &toml::Value) -> Vec<(String, String)> {
    let Some(table) = config.get("alias").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    table
        .iter()
        .map(|(name, value)| {
            let words = match value {
                toml::Value::String(text) => text.clone(),
                toml::Value::Array(words) => words
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            };
            (name.clone(), words)
        })
        .collect()
}

/// The package an alias selects, in any of the spellings cargo accepts.
fn alias_package(value: &str) -> Option<&str> {
    // Everything after `--` is the runee's own arguments: an alias that
    // runs something else can carry a `-p xtask-*` there and mean
    // nothing by it.
    let mut words = value.split_whitespace().take_while(|word| *word != "--");
    while let Some(word) = words.next() {
        let selected = match word {
            "-p" | "--package" => words.next(),
            _ => word
                .strip_prefix("--package=")
                .or_else(|| word.strip_prefix("-p").filter(|rest| !rest.is_empty())),
        };
        if selected.is_some() {
            return selected;
        }
    }
    None
}

/// Whatever would let an edit to an alias or its tool stop reaching the
/// job it feeds.
///
/// Every workflow that runs an alias is either the guard's own, whose
/// triggers are pinned above, or one of `PINNED_CONSUMERS`. A workflow
/// that runs one and is in neither is the case this exists for: its
/// filter could name anything, and nothing here would notice.
fn alias_consumer_problems(repo_root: &Path) -> Result<Vec<String>> {
    let path = repo_root.join(CARGO_CONFIG);
    if !path.is_file() {
        // Reported once, by the check that owns that file.
        return Ok(Vec::new());
    }
    let aliases = aliases(&manifest(&path)?);
    let texts = workflow_texts(repo_root)?;
    // Labeled `alias=package`, or `direct=package` for a tool a
    // workflow builds and runs itself: retargeting an alias leaves its
    // name alone while pointing it at a crate the filters here never
    // mentioned, and a direct call names no alias at all.
    let tools = xtask_packages(repo_root)?;
    let runs = alias_consumers(&texts, |text| {
        let reached = aliases
            .iter()
            .filter(|(alias, _)| runs_alias(text, alias))
            .map(|(alias, value)| format!("{alias}={}", alias_package(value).unwrap_or("?")));
        // Below `jobs:`, because that is where a workflow does things:
        // above it, `crates/xtask-foo/Cargo.toml` is a trigger path
        // saying when to run, which is the opposite of running it.
        let body = text.split_once("\njobs:").map_or(text, |(_, jobs)| jobs);
        let direct = tools
            .iter()
            .filter(|(_, tool)| uses_tool(body, tool))
            .map(|(_, tool)| format!("direct={tool}"));
        reached.chain(direct).collect()
    });

    let mut problems = Vec::new();
    let mut pinned_seen: BTreeSet<&str> = BTreeSet::new();
    for (listed, text) in &texts {
        // Not a consumer question: Rust compiled outside cargo, or a
        // command nothing can read, is automation off the convention
        // wherever it appears.
        problems.extend(unreadable_invocation(listed, text));
        let run: Vec<&str> = runs
            .get(listed.as_str())
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        if run.is_empty() || listed == GUARD_WORKFLOW {
            continue;
        }
        let listed = listed.as_str();
        let Some((pinned_path, pinned_aliases, pinned)) =
            PINNED_CONSUMERS.iter().find(|(known, ..)| *known == listed)
        else {
            problems.push(format!(
                "{listed} runs {run:?} with no pinned trigger block; add one to \
                 PINNED_CONSUMERS in crates/xtask-ci/src/scripts.rs, so that dropping \
                 {CARGO_CONFIG} or the tool's crate from its filters cannot pass unread \
                 (AGENTS.md, \"Repository automation\")"
            ));
            continue;
        };
        pinned_seen.insert(*pinned_path);
        // The aliases are pinned beside the block: a pin taken for one
        // tool says nothing about the crate of a tool added later, and
        // adding a call leaves the trigger block itself untouched.
        if run
            != pinned_aliases
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
        {
            problems.push(format!(
                "{listed} runs {run:?}, but its trigger block was pinned for \
                 {pinned_aliases:?}; check that every one of those crates is still in its \
                 filters, then re-pin PINNED_CONSUMERS in crates/xtask-ci/src/scripts.rs"
            ));
        }
        if !pinned_at_indent(text, pinned, 0, None) {
            problems.push(format!(
                "{listed}'s trigger block is not the pinned one, and it runs {run:?}; \
                 check that {CARGO_CONFIG} and each tool's crate are still in every filter, \
                 then re-pin PINNED_CONSUMERS in crates/xtask-ci/src/scripts.rs"
            ));
        }
    }
    for (listed, ..) in PINNED_CONSUMERS {
        if !pinned_seen.contains(listed) {
            problems.push(format!(
                "{listed} is pinned as an alias consumer but runs no alias; \
                 delete its entry from PINNED_CONSUMERS in crates/xtask-ci/src/scripts.rs"
            ));
        }
    }
    Ok(problems)
}

/// Whether a workflow runs Rust in a way this guard cannot read.
///
/// Two shapes, both of which put automation where nothing can check it:
/// `rustc tools/deploy.rs -o deploy` compiles a loose source file that
/// belongs to no crate and no alias, and `cargo ${{ matrix.stem }}-...`
/// assembles a subcommand out of expression fragments, so the alias it
/// runs is not in the file at all. Neither is refused for what it might
/// be doing - they are refused because the rule wants automation named
/// plainly enough to check.
fn unreadable_invocation(listed: &str, text: &str) -> Option<String> {
    // A shell continuation makes one command out of two lines, so a
    // flag on the second line is as much a part of it as one on the
    // first.
    let joined = text.replace("\\\n", " ");
    for line in joined.lines() {
        // What a step runs, not what it says about it: a `#` comment
        // that mentions rustc is prose, and the key is not the command.
        let line = line.split(" #").next().unwrap_or(line);
        let line = line.split_once("run:").map_or(line, |(_, rest)| rest);
        // One command per shell operator.
        for segment in line.split(['&', '|', ';']) {
            let mut words = segment
                .split_whitespace()
                .map(|word| normalize_target(word.trim_matches(['"', '\'', '(', ')'])))
                .filter(|word| !word.is_empty());
            let Some(head) = words.next() else {
                continue;
            };
            if head.rsplit('/').next() == Some("rustc") {
                return Some(format!(
                    "{listed} runs rustc; a loose .rs file compiled in a workflow belongs to \
                     no crate and no alias, which is where repository automation lives \
                     (AGENTS.md, \"Repository automation\")"
                ));
            }
            if !is_cargo(head) {
                continue;
            }
            // The first word that is not a flag is the subcommand -
            // except the value of a flag that takes one separately,
            // which is a word `-Z script tools/deploy.rs` would
            // otherwise offer up in place of the file it runs.
            let mut command = None;
            while let Some(next) = words.next() {
                if next.starts_with('+') {
                    continue;
                }
                if next.starts_with('-') {
                    if matches!(next, "-Z" | "-C" | "--config") {
                        words.next();
                    }
                    continue;
                }
                command = Some(next);
                break;
            }
            let Some(command) = command else {
                continue;
            };
            // `$FOO` and `${{ matrix.foo }}` alike: whatever the command
            // turns out to be, it is not in the file.
            if command.contains('$') {
                return Some(format!(
                    "{listed} runs `cargo {command}`, a subcommand assembled from an \
                     expression; name the alias literally, so what this workflow runs can \
                     be read here"
                ));
            }
            // `cargo -Zscript tools/deploy.rs` runs a loose source file
            // as a single-file package: a crate with no manifest, no
            // name and no alias.
            if std::path::Path::new(command)
                .extension()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("rs"))
            {
                return Some(format!(
                    "{listed} runs `cargo {command}`, a single-file cargo script; a loose \
                     .rs file belongs to no crate and no alias, which is where repository \
                     automation lives (AGENTS.md, \"Repository automation\")"
                ));
            }
        }
    }
    None
}

/// Whether one word names the cargo executable, by any path or suffix.
fn is_cargo(word: &str) -> bool {
    normalize_target(word).rsplit('/').next() == Some("cargo")
}

/// Every workflow file under `.github/workflows`, by repository path.
///
/// # Errors
///
/// Fails when one cannot be read.
fn workflow_texts(repo_root: &Path) -> Result<BTreeMap<String, String>> {
    let mut texts = BTreeMap::new();
    // From the index, like the crates: a scratch workflow a developer
    // has not committed runs nowhere, and this guard answers for what
    // the repository has.
    for entry in index_entries(repo_root)? {
        let is_workflow = entry.path.starts_with(".github/workflows/")
            && Path::new(&entry.path).extension().is_some_and(|kind| {
                kind.eq_ignore_ascii_case("yml") || kind.eq_ignore_ascii_case("yaml")
            });
        if !is_workflow {
            continue;
        }
        let path = repo_root.join(&entry.path);
        // Windows checkouts normalize to CRLF; the pins are written with
        // the line endings the repository stores.
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?
            .replace("\r\n", "\n");
        texts.insert(entry.path, text);
    }
    Ok(texts)
}

/// Which aliases each workflow runs, its callees' included.
///
/// A reusable workflow runs under its CALLER's triggers, so the caller
/// is a consumer too - `jobs.<id>.uses: ./.github/workflows/x` is the
/// edge. Taken to a fixed point, because a caller can call a caller.
fn alias_consumers(
    texts: &BTreeMap<String, String>,
    labels: impl Fn(&str) -> BTreeSet<String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut runs: BTreeMap<String, BTreeSet<String>> = texts
        .iter()
        .map(|(listed, text)| (listed.clone(), labels(text)))
        .collect();
    loop {
        let mut grew = false;
        for (listed, text) in texts {
            let inherited: BTreeSet<String> = calls(text)
                .into_iter()
                .filter_map(|called| runs.get(called).cloned())
                .flatten()
                .collect();
            let mine = runs.entry(listed.clone()).or_default();
            let before = mine.len();
            mine.extend(inherited);
            grew |= mine.len() != before;
        }
        if !grew {
            return runs;
        }
    }
}

/// The local workflows a workflow calls, as `.github/workflows/<name>`.
///
/// `jobs.<id>.uses: ./.github/workflows/x.yml` runs `x.yml` under THIS
/// workflow's triggers, so whatever it runs, this one runs.
fn calls(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .filter_map(|word| {
            word.trim_matches(['"', '\'', ',', '{', '}', '[', ']'])
                .strip_prefix("./")
        })
        .filter(|path| path.starts_with(".github/workflows/"))
        .collect()
}

/// Every crate under `crates/`, as (directory, package name, private).
///
/// Read from the manifests, not from the directory names: a package may
/// be called anything wherever it sits, and a tool hiding under an
/// ordinary directory name is exactly what the location rule is for.
/// Nested manifests count - `crates/tools/ci` is no less a crate for
/// being two levels down. `registry/` is a separate workspace of its own
/// and is not part of this.
///
/// The manifests come from the index, like everything else here: a
/// scratch crate a developer has not committed is not something this
/// repository ships, and failing the local run over one would make the
/// guard answer for uncommitted files.
///
/// # Errors
///
/// Fails when the index cannot be read.
fn workspace_crates(repo_root: &Path) -> Result<Vec<(String, String, bool)>> {
    let root = manifest(&repo_root.join("Cargo.toml")).unwrap_or(toml::Value::Boolean(false));
    let mut found = Vec::new();
    for entry in index_entries(repo_root)? {
        let Some(listed) = entry
            .path
            .strip_prefix("crates/")
            .and_then(|rest| rest.strip_suffix("/Cargo.toml"))
        else {
            continue;
        };
        let Ok(manifest) = manifest(&repo_root.join(&entry.path)) else {
            continue;
        };
        let Some(package) = manifest.get("package") else {
            continue;
        };
        let Some(package_name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let publish = package.get("publish");
        // `publish.workspace = true` says the answer lives in the root
        // manifest, and there it is a plain boolean.
        let publish = if publish
            .and_then(|publish| publish.get("workspace"))
            .and_then(toml::Value::as_bool)
            .unwrap_or_default()
        {
            root.get("workspace")
                .and_then(|workspace| workspace.get("package"))
                .and_then(|package| package.get("publish"))
        } else {
            publish
        };
        let private = publish.is_some_and(|publish| match publish {
            toml::Value::Boolean(allowed) => !allowed,
            // `publish = []` names no registry to publish to, which is
            // cargo's other way of saying the crate does not ship:
            // `cargo publish` refuses it in the same words.
            toml::Value::Array(registries) => registries.is_empty(),
            _ => false,
        });
        found.push((listed.to_owned(), package_name.to_owned(), private));
    }
    found.sort();
    Ok(found)
}

/// The tool crates this repository has, as (directory, package name).
///
/// # Errors
///
/// Fails when the index cannot be read.
fn xtask_packages(repo_root: &Path) -> Result<Vec<(String, String)>> {
    Ok(workspace_crates(repo_root)?
        .into_iter()
        .filter(|(_, name, _)| name.starts_with("xtask-"))
        .map(|(dir, name, _)| (dir, name))
        .collect())
}

/// One word of a command line as the thing it names: `-pxtask-foo` is
/// the package, `xtask-foo.exe` is the binary.
fn normalize_target(word: &str) -> &str {
    let word = word
        .strip_prefix("-p")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(word);
    // `-p name@version` is a package spec, and the version is not part
    // of what it names.
    let word = word.split('@').next().unwrap_or(word);
    // `name:version` says the same thing, and cargo still takes it. Only
    // before a version, so the `:` of a `file:///` URL stays where it is.
    let word = match word.rsplit_once(':') {
        Some((name, version)) if version.starts_with(|c: char| c.is_ascii_digit()) => name,
        _ => word,
    };
    word.strip_suffix(".exe").unwrap_or(word)
}

/// Whether `path` matches a workspace member pattern.
///
/// Cargo takes a glob anywhere in the pattern, not only at its end
/// (`crates/*task-*` is a member list this workspace could be written
/// with), and reading only the trailing form would report every tool
/// outside a workspace it is in.
///
/// `*` and nothing else, which is what a member list is written with.
/// A pattern using cargo's rarer glob syntax (`?`, `[a-z]`) reads here
/// as no match, so the crate reports as no member: wrong, and wrong in
/// the direction that stops the gate rather than the direction that
/// lets a tool through.
fn matches_glob(pattern: &str, path: &str) -> bool {
    let mut rest = path;
    let mut pieces = pattern.split('*');
    let Some(first) = pieces.next() else {
        return false;
    };
    let Some(tail) = rest.strip_prefix(first) else {
        return false;
    };
    rest = tail;
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middles)) = pieces.split_last() else {
        // No glob at all: the pattern is the path.
        return rest.is_empty();
    };
    for piece in middles {
        let Some(at) = rest.find(piece) else {
            return false;
        };
        rest = &rest[at + piece.len()..];
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Whether `text` names a tool crate as a thing to build or run.
///
/// An alias is not the only way to reach one: `cargo build -p xtask-foo`
/// and `./target/debug/xtask-foo` are a workflow consuming that crate as
/// surely as `cargo foo` is. Matched on the last path component, so a
/// trigger path (`crates/xtask-foo/**`) is not a use of it.
fn uses_tool(text: &str, tool: &str) -> bool {
    // `#` among them: a fully qualified package spec puts the name on
    // either side of it (`path+file:///repo/crates/xtask-foo#0.1.0`,
    // `path+file:///repo#xtask-foo@0.1.0`), so splitting there leaves
    // the name to be found whichever form it took.
    let separator = |c: char| c.is_whitespace() || "\"';&|()`$<>{}[],=\\#".contains(c);
    text.split(separator).map(normalize_target).any(|word| {
        // The last component names the binary
        // (`./target/debug/xtask-foo`); an inner one names the crate
        // (`--manifest-path crates/xtask-foo/Cargo.toml`). A glob is a
        // trigger path rather than a use of it.
        word.rsplit('/').next() == Some(tool)
            || (!word.contains('*') && word.split('/').any(|part| part == tool))
    })
}

/// Whether `text` invokes `cargo <alias>` on any line.
///
/// Read as whole words rather than as `cargo <alias>` literally, because
/// cargo takes `[+toolchain] [OPTIONS]` before the command and a shell
/// puts its own punctuation around both. The looseness cuts one way on
/// purpose: a mention in a comment counts too, and pinning the triggers
/// of a workflow that does not run an alias costs nothing, while missing
/// one that does costs the job.
fn runs_alias(text: &str, alias: &str) -> bool {
    // Split on shell punctuation as well as whitespace: a redirection
    // needs no space around it (`cargo check-sql>/dev/null`), and the
    // whole file is one stream because a `run:` block can be folded
    // (`run: >`) or continued (`cargo \`), putting the command and its
    // argument on different lines of the YAML.
    let separator = |c: char| c.is_whitespace() || "\"';&|()`$<>{}[],=\\".contains(c);
    let mut words = text.split(separator).filter(|word| !word.is_empty());
    let (mut cargo, mut named) = (false, false);
    // Both present, in either order and anywhere in the file: a command
    // can be assembled from a variable or a matrix value
    // (`env: {CMD: check-sql}` … `run: cargo "$CMD"`), which leaves no
    // literal alias after the word `cargo` to look for.
    words.any(|word| {
        cargo |= is_cargo(word);
        named |= word == alias;
        cargo && named
    })
}

/// Whatever is wrong with the shape of the `xtask-*` crates: the
/// convention is a private workspace member reached through an alias, so
/// a crate that is none of those is not the convention.
///
/// The three manifests are PARSED, not matched as text: `publish = false`
/// in a comment satisfies a substring search, and `publish.workspace`
/// does not appear in one at all.
fn xtask_crate_problems(repo_root: &Path) -> Result<Vec<String>> {
    let crates = repo_root.join("crates");
    let found = workspace_crates(repo_root)?;
    // By directory name, because that is what the rest of this checks:
    // whether the crate at `crates/xtask-<name>` is built and reachable.
    let names: Vec<String> = found
        .iter()
        .map(|(dir, ..)| dir.clone())
        .filter(|dir| dir.starts_with("xtask-"))
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let root = manifest(&repo_root.join("Cargo.toml"))?;
    let paths = |key: &str| -> Vec<String> {
        root.get("workspace")
            .and_then(|workspace| workspace.get(key))
            .and_then(toml::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(toml::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let members = paths("members");
    // `exclude` wins over a glob in `members`, so a tool can be swept in
    // by `crates/*` and taken straight back out again.
    let excluded = paths("exclude");
    let config = manifest(&repo_root.join(CARGO_CONFIG))?;
    let aliases = aliases(&config);

    let mut problems = Vec::new();
    // Found by manifest, not by directory name: `crates/tools` calling
    // itself `xtask-rogue` is a tool the name-keyed loop below would
    // never look at, and an alias could reach it by package name.
    //
    // `publish = false` is what a tool is, in this workspace: every
    // crate that ships is published, so a private one is maintainer
    // tooling whatever it calls itself - and tooling is an xtask crate.
    for (dir, package, private) in found {
        let tool = package.starts_with("xtask-");
        if tool && dir != package {
            problems.push(format!(
                "crates/{dir} declares the package {package}; a tool crate lives at \
                 crates/<its own name>, which is where everything that checks one looks"
            ));
        }
        if private && !tool {
            problems.push(format!(
                "crates/{dir} declares {package} publish = false but is not an xtask crate; \
                 a private crate here is maintainer tooling, which is a crates/xtask-* crate \
                 reached through an alias (AGENTS.md, \"Repository automation\")"
            ));
        }
    }
    for name in names {
        let path = format!("crates/{name}");
        // A glob member (`crates/*`) covers it too; anything else has to
        // name it exactly.
        // `./crates/x` and `crates/x` are the same member to cargo, and
        // an `exclude` that reads as neither is one cargo honors and
        // this would not.
        let covers = |list: &[String]| {
            list.iter()
                .any(|entry| matches_glob(entry.strip_prefix("./").unwrap_or(entry), &path))
        };
        let member = covers(&members) && !covers(&excluded);
        if !member {
            problems.push(format!(
                "{path} is not a member of the root workspace; \
                 an xtask crate nobody builds is not a check"
            ));
        }
        let manifest = manifest(&crates.join(&name).join("Cargo.toml"))?;
        let publish = manifest
            .get("package")
            .and_then(|package| package.get("publish"));
        // `publish.workspace = true` says the answer lives in the root
        // manifest, and there it is a plain boolean.
        let inherited = publish
            .and_then(|publish| publish.get("workspace"))
            .and_then(toml::Value::as_bool)
            .unwrap_or_default();
        let publish = if inherited {
            root.get("workspace")
                .and_then(|workspace| workspace.get("package"))
                .and_then(|package| package.get("publish"))
        } else {
            publish
        };
        if publish.and_then(toml::Value::as_bool) != Some(false) {
            problems.push(format!(
                "{path} must be publish = false; repository tooling is not shipped"
            ));
        }
        if !aliases
            .iter()
            .any(|(_, value)| alias_package(value) == Some(name.as_str()))
        {
            problems.push(format!(
                "{path} has no cargo alias in .cargo/config.toml; \
                 an xtask crate is reached through an alias"
            ));
        }
    }
    Ok(problems)
}

/// One parsed TOML manifest.
fn manifest(path: &Path) -> Result<toml::Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Whether `pinned` appears where YAML would actually read it: starting
/// at a line boundary, indented exactly `indent`, and (when `under` is
/// given) with that top-level key as the nearest column-0 line above it.
///
/// A plain substring match is not enough. Copying the pinned text into a
/// block scalar (`name: |`) while deleting the real key satisfies
/// `contains` while YAML sees inert string content - but a block
/// scalar's body must be indented deeper than its own key, so content at
/// a fixed shallow indent under the right top-level key cannot be inside
/// one.
fn pinned_at_indent(text: &str, pinned: &str, indent: usize, under: Option<&str>) -> bool {
    let mut from = 0;
    while let Some(offset) = text[from..].find(pinned) {
        let at = from + offset;
        from = at + 1;
        if at != 0 && !text[..at].ends_with('\n') {
            continue;
        }
        if text[at..].chars().take_while(|c| *c == ' ').count() != indent {
            continue;
        }
        let Some(parent) = under else {
            return true;
        };
        // The nearest line above that starts in column 0 is the block
        // this text belongs to.
        let enclosing = text[..at]
            .lines()
            .rev()
            .find(|line| !line.is_empty() && !line.starts_with([' ', '#']));
        if enclosing == Some(parent) {
            return true;
        }
    }
    false
}

/// Whatever would stop this guard from actually running in CI.
///
/// A check a change can switch off in the same change is not a check, so
/// the wiring is part of what the guard verifies. Both halves are pinned
/// VERBATIM rather than inspected: a lexical read of YAML can be talked
/// out of its answer (flow style, quoted keys, block scalars, a `needs:`
/// on a job that is skipped anyway), and every one of those spellings
/// changes the text. Editing either block is a conscious re-pin here.
fn workflow_wiring_problems(repo_root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(repo_root.join(GUARD_WORKFLOW)) else {
        return vec![format!(
            "{GUARD_WORKFLOW} is missing; it is where this guard runs"
        )];
    };
    // Windows checkouts normalize to CRLF, and the pins are written with
    // the line endings the repository stores.
    let text = text.replace("\r\n", "\n");
    let mut problems = Vec::new();
    if !pinned_at_indent(&text, PINNED_TRIGGERS, 0, None) {
        problems.push(format!(
            "{GUARD_WORKFLOW}'s trigger and env blocks are not the pinned ones; the triggers \
             must stay unfiltered (no paths:/paths-ignore:) and keep pull_request, and a \
             workflow-level variable reaches the guard's own steps. Re-pin PINNED_TRIGGERS in \
             crates/xtask-ci/src/scripts.rs if the change is deliberate."
        ));
    }
    // The step's own line, not a mention of it: a commented-out `run:`
    // leaves the command in the file and the guard out of CI.
    let step = format!("run: {GUARD_COMMAND}");
    if !text
        .lines()
        .any(|line| line.trim_start().trim_start_matches("- ").trim_end() == step)
    {
        problems.push(format!(
            "{GUARD_WORKFLOW} no longer runs `{GUARD_COMMAND}`; this guard has to run \
             somewhere, and this is where"
        ));
    }
    problems
}
