//! Drift pin for the freshness guards' path lists.
//!
//! Each workflow that runs the guard states the same set three times -
//! once under `push.paths`, once under `pull_request.paths`, once as
//! the guard's `--path` arguments - and the guard is only sound while
//! all three agree. They were hand-maintained duplicates before the
//! port, and they did drift: `crates/xtask-registry-smoke/**` reached
//! `registry.yml`'s trigger filter and not its guard, so the guard
//! could answer "not superseded" for a commit main had already moved
//! past.

use std::fs;
use std::path::PathBuf;

/// Every workflow whose freshness guard runs `cargo
/// workflow-superseded`.
const GUARDED: [&str; 2] = ["registry.yml", "ports-publish.yml"];

fn read(workflow: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/workflows")
        .join(workflow);
    fs::read_to_string(path).unwrap_or_else(|error| panic!("reading {workflow}: {error}"))
}

/// A trigger glob and a `rev-list` pathspec spell a directory
/// differently (`registry/**` vs `registry/`) and a file identically.
fn normalize(entry: &str) -> String {
    let entry = entry.trim().trim_matches('"');
    entry.strip_suffix("**").unwrap_or(entry).to_owned()
}

/// Every `paths:` list in the file, in order.
fn trigger_paths(yaml: &str) -> Vec<Vec<String>> {
    let mut lists = Vec::new();
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "paths:" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mut list = Vec::new();
        while let Some(next) = lines.peek() {
            let trimmed = next.trim_start();
            if next.len() - trimmed.len() <= indent {
                break;
            }
            if trimmed.starts_with('#') {
                lines.next();
            } else if let Some(entry) = trimmed.strip_prefix("- ") {
                list.push(normalize(entry));
                lines.next();
            } else {
                break;
            }
        }
        lists.push(list);
    }
    lists
}

/// The `--path` arguments of the `freshness` step, up to the next step.
fn guard_paths(yaml: &str) -> Vec<String> {
    let mut lines = yaml
        .lines()
        .skip_while(|line| line.trim() != "id: freshness");
    lines.next();
    let mut paths = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("- name:") {
            break;
        }
        if let Some(argument) = trimmed.strip_prefix("--path ") {
            paths.push(normalize(argument.trim_end_matches('\\')));
        }
    }
    paths
}

#[test]
fn the_guards_path_list_matches_both_trigger_filters() {
    for workflow in GUARDED {
        let yaml = read(workflow);
        let guard = guard_paths(&yaml);
        assert!(
            !guard.is_empty(),
            "{workflow}: found no --path arguments in the freshness step"
        );

        let triggers = trigger_paths(&yaml);
        assert_eq!(
            triggers.len(),
            2,
            "{workflow}: expected a push and a pull_request paths: filter"
        );
        for (copy, trigger) in triggers.iter().enumerate() {
            assert_eq!(
                *trigger, guard,
                "{workflow}: paths: copy {copy} disagrees with the freshness guard's --path list"
            );
        }
    }
}

#[test]
fn the_guard_treats_edits_to_itself_as_superseding() {
    for workflow in GUARDED {
        let yaml = read(workflow);
        assert!(
            guard_paths(&yaml).contains(&"crates/xtask-workflow-guard/".to_owned()),
            "{workflow}: the guard's own crate is missing from its path list, so it \
             would stop noticing changes to itself"
        );
    }
}
