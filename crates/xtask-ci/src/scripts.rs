//! The repository-automation guard (AGENTS.md, "Repository
//! automation").
//!
//! Repository automation is Rust, in a private `crates/xtask-*` crate
//! reached through a Cargo alias.  This guard keeps a non-Rust script
//! from coming back.  It reads git's index - paths, file modes, and blob
//! ids - and refuses:
//!
//! - a tooling extension, in any component of the file name, so
//!   `deploy.sh.in` is caught as well as `deploy.sh`;
//! - a bare name that is itself a tool (`Makefile`, `justfile`);
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
//! nobody mistakes them for coverage: automation smuggled in as a data
//! file and run through an interpreter argument
//! (`node tools/deploy.data`) passes the file scan - the workflow-block
//! scan is what catches the caller; `website/src/**/*.ts` is website
//! source and is not scanned as tooling; a file name whose extension
//! uses non-ASCII homoglyphs would not match; `TOOLING_EXTENSIONS` names
//! the languages somebody thought of, so one nobody did is caught by the
//! executable bit, a shebang or its caller rather than its name, and the
//! list takes the next language the day it appears; and content comes
//! from the working tree, so a locally-staged-but-rewritten file reads
//! as its on-disk bytes (CI checks out clean, which is the authority).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// File-name components that make a tracked file executable tooling
/// wherever it sits.  None of these is a source language in this
/// repository, so there is nothing to weigh: a `.sh` under
/// `website/src/` is as much a script as one under `tools/`.
///
/// Checked against every dot-separated component after the first, so a
/// template (`deploy.sh.in`) cannot launder one.
const TOOLING_EXTENSIONS: [&str; 34] = [
    "applescript",
    "awk",
    "bash",
    "bat",
    "cmd",
    "csh",
    "dash",
    "expect",
    "fish",
    "groovy",
    "jl",
    "jse",
    "ksh",
    "lua",
    "nu",
    "php",
    "pl",
    "pm",
    "ps1",
    "psd1",
    "psm1",
    "py",
    "pyw",
    "rake",
    "rb",
    "sed",
    "sh",
    "tcl",
    "tcsh",
    "vbe",
    "vbs",
    "wsf",
    "wsh",
    "zsh",
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

/// Bare file names that are a tool without needing an extension.
const TOOLING_NAMES: [&str; 8] = [
    ".envrc",
    "GNUmakefile",
    "Justfile",
    "Makefile",
    "Rakefile",
    "Taskfile.yml",
    "justfile",
    "makefile",
];

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

/// The workflow that must run this guard, and the job that must do it.
const GUARD_WORKFLOW: &str = ".github/workflows/rust.yml";
const GUARD_JOB: &str = "automation";
const GUARD_COMMAND: &str = "./target/debug/xtask-ci check-scripts";

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

/// The job that must run the guard, verbatim: unconditional, not
/// allowed to fail, and depending on nothing that could skip it.  Runs
/// through the next job's key for the same reason as the triggers.
const PINNED_JOB: &str = "  automation:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@2c7215f132e9ebf062739d9130488b56d53c060c # master
        with:
          toolchain: stable
      - uses: Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4 # v2.9.1

      # Built, then executed directly: `cargo run` honors a
      # `[target.<cfg>] runner` from .cargo/config.toml, so a change
      # could add one and have CI run `true` in place of the guard. A
      # plain exec cannot be redirected that way, and the guard itself
      # then refuses any non-[alias] section in that file.
      #
      # `shell:` is pinned for the same reason one step up: a
      # workflow-level `defaults.run.shell` can name any command and
      # have `run:` handed to it, which would report success without
      # running anything. A step's own shell wins over that default.
      - name: Build the repository automation guard
        shell: bash
        run: cargo build --locked -p xtask-ci

      - name: Repository automation guard
        shell: bash
        run: ./target/debug/xtask-ci check-scripts

  clippy:";

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
    violations.extend(workflow_wiring_problems(repo_root));
    Ok(violations)
}

/// The product-source roots the rule's domain carves out.
#[must_use]
pub fn source_roots() -> Vec<&'static str> {
    PRODUCT_SOURCE_ROOTS.to_vec()
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

/// Why `path`'s name alone makes it a tool, if it does.
fn name_problem(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?;
    if TOOLING_NAMES
        .into_iter()
        .any(|known| known.eq_ignore_ascii_case(name))
    {
        return Some(format!("{name} is repository automation"));
    }
    let in_source_root = PRODUCT_SOURCE_ROOTS
        .into_iter()
        .any(|root| path.starts_with(root));
    // Every component after the first: `deploy.sh.in` is a shell script
    // behind a template suffix.
    name.split('.').skip(1).find_map(|component| {
        if let Some(known) = TOOLING_EXTENSIONS
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(component))
        {
            return Some(format!("a .{known} script is repository automation"));
        }
        if in_source_root {
            return None;
        }
        SOURCE_LANGUAGE_EXTENSIONS
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(component))
            .map(|known| {
                format!(
                    "a .{known} file outside {} is repository automation, not source",
                    PRODUCT_SOURCE_ROOTS.join(" or ")
                )
            })
    })
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
    // A byte-order mark before `#!` stops the kernel but not a human
    // running `sh file`.
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let Some(rest) = bytes.strip_prefix(b"#!") else {
        return Ok(false);
    };
    let line = rest.split(|&byte| byte == b'\n').next().unwrap_or_default();
    Ok(!line.starts_with(b"[") && line.iter().any(|byte| !byte.is_ascii_whitespace()))
}

/// Whatever is wrong with `.cargo/config.toml` beyond the aliases.
///
/// The file claims to be `[alias]`-only, and that claim is load-bearing
/// twice over: a `[build]` or `[target]` key here also reaches the
/// standalone `registry/` workspace, and a `[target.<cfg>] runner` would
/// make `cargo run` execute something else entirely - so the CI job
/// could report success having run `true` instead of this guard.
fn cargo_config_problems(repo_root: &Path) -> Result<Vec<String>> {
    let path = repo_root.join(".cargo/config.toml");
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
    for (name, value) in aliases(&config) {
        match alias_package(&value) {
            Some(package) if package.starts_with("xtask-") => {}
            _ => problems.push(format!(
                "the `cargo {name}` alias does not run an xtask package (`-p xtask-<name>`); \
                 repository automation is an xtask crate reached through an alias, and an \
                 alias onto anything else is that rule routed around \
                 (AGENTS.md, \"Repository automation\")"
            )),
        }
    }
    Ok(problems)
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
    let mut words = value.split_whitespace();
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

/// Whatever is wrong with the shape of the `xtask-*` crates: the
/// convention is a private workspace member reached through an alias, so
/// a crate that is none of those is not the convention.
///
/// The three manifests are PARSED, not matched as text: `publish = false`
/// in a comment satisfies a substring search, and `publish.workspace`
/// does not appear in one at all.
fn xtask_crate_problems(repo_root: &Path) -> Result<Vec<String>> {
    let crates = repo_root.join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", crates.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("xtask-") && entry.path().join("Cargo.toml").is_file() {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    names.sort();

    let members = manifest(&repo_root.join("Cargo.toml"))?;
    let members: Vec<&str> = members
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .map(|list| list.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    let config = manifest(&repo_root.join(".cargo/config.toml"))?;
    let aliases = aliases(&config);

    let mut problems = Vec::new();
    for name in names {
        let path = format!("crates/{name}");
        // A glob member (`crates/*`) covers it too; anything else has to
        // name it exactly.
        let member = members.iter().any(|entry| {
            *entry == path
                || entry
                    .strip_suffix('*')
                    .is_some_and(|prefix| path.starts_with(prefix))
        });
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
        if publish.and_then(toml::Value::as_bool) != Some(false) {
            problems.push(format!(
                "{path} must be publish = false; repository tooling is not shipped"
            ));
        }
        if !aliases
            .iter()
            .any(|(_, value)| value.contains(&format!("-p {name} --")))
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
    if !pinned_at_indent(&text, PINNED_JOB, 2, Some("jobs:")) {
        problems.push(format!(
            "{GUARD_WORKFLOW}'s {GUARD_JOB} job is not the pinned one; it must run \
             {GUARD_COMMAND} unconditionally - no if:, no continue-on-error:, no needs:. \
             Re-pin PINNED_JOB in crates/xtask-ci/src/scripts.rs if the change is deliberate."
        ));
    }
    problems
}
