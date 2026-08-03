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
//! uses non-ASCII homoglyphs would not match; and content comes from the
//! working tree, so a locally-staged-but-rewritten file reads as its
//! on-disk bytes (CI checks out clean, which is the authority).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// File-name components that make a tracked file executable tooling
/// rather than source or data.
///
/// Checked against every dot-separated component after the first, so a
/// template (`deploy.sh.in`) cannot launder one.
const TOOLING_EXTENSIONS: [&str; 32] = [
    "applescript",
    "awk",
    "bash",
    "bat",
    "cjs",
    "cmd",
    "csh",
    "dash",
    "expect",
    "fish",
    "groovy",
    "jl",
    "js",
    "ksh",
    "lua",
    "mjs",
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
    "zsh",
];

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
        "0988ea652abf2399463f223a377a583f836b9f31",
    ),
];

/// The website's own build-time checks, run through npm by
/// `website.yml` and owned by `website/AGENTS.md`.
///
/// Not the migration queue: these are website tooling, they may change
/// freely, and they pin a path rather than a blob id. They are listed
/// one by one anyway - the alternative is a `website/**` pattern, which
/// is exactly the shape this guard refuses to have.
const WEBSITE_SCRIPTS: [&str; 5] = [
    "website/scripts/lib/find-html-files.mjs",
    "website/scripts/verify-docs-links.mjs",
    "website/scripts/verify-no-inline-scripts.mjs",
    "website/scripts/verify-progressive-independence.mjs",
    "website/scripts/verify-progressive-independence.test.mjs",
];

/// The workflow that must run this guard, and the job that must do it.
const GUARD_WORKFLOW: &str = ".github/workflows/rust.yml";
const GUARD_JOB: &str = "automation";
const GUARD_COMMAND: &str = "cargo check-scripts";

/// The trigger block `rust.yml` must carry, verbatim: no `paths:` or
/// `paths-ignore:` filter, and `pull_request` present, so no change can
/// reach main without this guard having run.  The pin runs THROUGH the
/// next key, so a filter appended after the block cannot hide behind a
/// still-matching prefix.
const PINNED_TRIGGERS: &str = "on:
  push:
    branches: [main]
  pull_request:

env:";

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

      - name: Repository automation guard
        run: cargo check-scripts

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

    violations.extend(xtask_crate_problems(repo_root)?);
    violations.extend(workflow_wiring_problems(repo_root));
    Ok(violations)
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

/// Whatever makes one index entry repository automation.
fn entry_problem(repo_root: &Path, entry: &Entry) -> Option<String> {
    let path = entry.path.as_str();
    let remedy = "write it as a crates/xtask-* command with a cargo alias \
                  (AGENTS.md, \"Repository automation\")";
    if let Some(reason) = name_problem(path) {
        return Some(format!("{path}: {reason}; {remedy}"));
    }
    match entry.mode.as_str() {
        // A gitlink hides a whole tree from this scan.
        "160000" => Some(format!(
            "{path}: a submodule's contents cannot be inspected by this guard; \
             vendor what you need instead (AGENTS.md, \"Repository automation\")"
        )),
        // A symlink's bytes are its target path, not a script.
        "120000" => None,
        "100755" => Some(format!(
            "{path}: the executable bit makes this runnable as a script, \
             whatever its name; {remedy}"
        )),
        _ => match first_line_shebang(&repo_root.join(path)) {
            Ok(true) => Some(format!(
                "{path}: an interpreter shebang makes this repository automation; {remedy}"
            )),
            Ok(false) => None,
            Err(err) => Some(format!(
                "{path}: tracked but not readable, so the guard cannot clear it ({err:#})"
            )),
        },
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
    // Every component after the first: `deploy.sh.in` is a shell script
    // behind a template suffix.
    name.split('.').skip(1).find_map(|component| {
        TOOLING_EXTENSIONS
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(component))
            .map(|known| format!("a .{known} script is repository automation"))
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
    let aliases = manifest(&repo_root.join(".cargo/config.toml"))?;
    let aliases: Vec<String> = aliases
        .get("alias")
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .values()
                .map(|value| match value {
                    toml::Value::String(text) => text.clone(),
                    toml::Value::Array(words) => words
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                    _ => String::new(),
                })
                .collect()
        })
        .unwrap_or_default();

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
            .any(|alias| alias.contains(&format!("-p {name} --")))
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
    let mut problems = Vec::new();
    if !text.contains(PINNED_TRIGGERS) {
        problems.push(format!(
            "{GUARD_WORKFLOW}'s trigger block is not the pinned one; it must stay unfiltered \
             (no paths:/paths-ignore:) and keep pull_request, or a change could route around \
             the guard. Re-pin PINNED_TRIGGERS in crates/xtask-ci/src/scripts.rs if the change \
             is deliberate."
        ));
    }
    if !text.contains(PINNED_JOB) {
        problems.push(format!(
            "{GUARD_WORKFLOW}'s {GUARD_JOB} job is not the pinned one; it must run \
             {GUARD_COMMAND} unconditionally - no if:, no continue-on-error:, no needs:. \
             Re-pin PINNED_JOB in crates/xtask-ci/src/scripts.rs if the change is deliberate."
        ));
    }
    problems
}
