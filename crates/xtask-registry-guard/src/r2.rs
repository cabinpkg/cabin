//! The R2 acquisition guard (`registry/docs/architecture.md`, "The cost
//! governor").
//!
//! Every billable R2 call the Worker makes must be admitted by the cost
//! governor before the call.  A guard cannot prove admission lexically,
//! so it pins the seam instead: every acquisition of an R2 bucket handle
//! (`env.bucket(...)` in any spelling) must sit in a function this
//! module's allowlist pins with its exact acquisition count, and a new site
//! fails CI until a reviewer confirms the governor admission and re-pins
//! it.  The generic accessors that could yield a `Bucket` without the
//! `bucket` token appearing at all are banned outright.
//!
//! Two things differ from the Perl guard this replaces.  Violations come
//! out sorted by path rather than in `find`'s directory order.  And the
//! allowlist keys on the reported path exactly, where the Perl matched
//! any path ENDING in a sanctioned one - so a nested
//! `src/x/src/glue/read.rs` no longer inherits `src/glue/read.rs`'s
//! pins.  Nothing else about which call sites are accepted changes.
//!
//! ponytail: pins where handles are acquired, not that every use is
//! admitted - a new call inside an already-pinned function passes, and a
//! call assembled by a macro would pass.  It is a regression tripwire
//! that forces diff review at the seam; make it syntax-aware only if
//! that stops holding.

use std::path::Path;

use anyhow::Result;

use crate::lexical::blank_comments_and_strings;
use crate::source::{is_space, is_word, line_of, rust_sources};

/// Where this guard's pins live, for the diagnostics that tell a
/// reviewer to update them.
const PIN_SITE: &str = "crates/xtask-registry-guard/src/r2.rs";

/// (reported path) => \[(enclosing fn, sanctioned acquisition count)\].
///
/// `glue/`: the four request-path acquisitions are each immediately
/// preceded by a governor decide (artifact/read/publish paths), the
/// reclaim delete is R2's one free operation, and the heal helper admits
/// per call inside.  `web_glue`'s source viewer and `backup_glue`'s jobs
/// (the dump job, and the queue drain that acquires both buckets once
/// per pass) admit before every billable call.
const SANCTIONED: [(&str, &[(&str, usize)]); 4] = [
    (
        "src/glue/read.rs",
        &[("artifact_response", 1), ("charged_blob_read", 1)],
    ),
    (
        "src/glue/bearer.rs",
        &[
            ("persist_new_revision", 1),
            ("revive_rejected_revision", 1),
            ("delete_blob_if_unreferenced", 1),
            ("heal_blobs_on_retry", 1),
        ],
    ),
    ("src/web_glue.rs", &[("package_source", 1)]),
    (
        "src/backup_glue.rs",
        &[("run_nightly_dump", 1), ("drain_backup_queue", 2)],
    ),
];

/// The generic accessors that can produce a bucket handle without the
/// `bucket` token ever appearing.  Nothing in this Worker needs either.
/// (Reflection over the raw env object could still do it - that is
/// deliberate evasion, which is code review's job, not a tripwire's.)
const BANNED_ACCESSORS: [&str; 2] = ["get_binding", "unchecked_into"];

/// Every violation under `registry_dir`, as the diagnostic lines the
/// guard prints.  An empty result means the guard accepted the tree.
///
/// # Errors
///
/// Fails when `registry_dir/src` cannot be read.
pub fn check(registry_dir: &Path) -> Result<Vec<String>> {
    let mut violations = Vec::new();
    for source in rust_sources(&registry_dir.join("src"), "src")? {
        let blanked = blank_comments_and_strings(&std::fs::read(&source.path)?);
        violations.extend(scan(&blanked, &source.relative));
    }
    Ok(violations)
}

/// The violations in one blanked source file.
fn scan(source: &[u8], file: &str) -> Vec<String> {
    let sanctioned: &[(&str, usize)] = SANCTIONED
        .iter()
        .find(|(path, _)| *path == file)
        .map_or(&[], |(_, pins)| *pins);
    let functions = functions(source);
    let mut violations = Vec::new();
    let mut seen: Vec<(&str, usize)> = Vec::new();

    // Any spelling that can reach `Env::bucket` - `.bucket(`,
    // `::bucket(`, `r#bucket`, split across lines or comments. The word
    // boundary keeps `bucket_from_columns` out; requiring a following
    // `(` keeps field access (`auth.bucket`) out.
    for offset in method_calls(source, b"bucket", true) {
        // A closure inside a function attributes to that function; good
        // enough for a pin whose job is to force review, not to prove
        // admission.
        let enclosing = functions
            .iter()
            .rev()
            .find(|(at, _)| *at <= offset)
            .map_or("(no enclosing fn)", |(_, name)| name.as_str());
        let count = if let Some((_, count)) = seen.iter_mut().find(|(name, _)| *name == enclosing) {
            *count += 1;
            *count
        } else {
            seen.push((enclosing, 1));
            1
        };
        if pinned_count(sanctioned, enclosing).is_some_and(|pinned| count <= pinned) {
            continue;
        }
        violations.push(format!(
            "{file}:{}: unsanctioned R2 bucket acquisition in {enclosing} - \
             prove the governor admission and pin it in {PIN_SITE}",
            line_of(source, offset),
        ));
    }

    // A path-form method item (`Env::bucket` with no call parens) is an
    // alias that would launder every later acquisition past the scan
    // above, so creating one is itself a violation. The dotted form
    // needs no twin check: `env.bucket` without parens is not valid Rust
    // for a method value, and plain field access is another name.
    for offset in path_items(source, b"bucket") {
        violations.push(format!(
            "{file}:{}: R2 bucket method alias (path form without a call); \
             call it directly in a pinned function instead",
            line_of(source, offset),
        ));
    }

    // One pass over the source, not one per name, so the reports come
    // out in source order.
    for (offset, accessor) in banned_accessors(source) {
        violations.push(format!(
            "{file}:{}: {accessor} sidesteps the typed accessors \
             (an R2 handle without the bucket token); use the typed \
             Env method in a pinned function",
            line_of(source, offset),
        ));
    }

    // A pinned function that still exists but no longer acquires its
    // bucket means the seam moved; the pin must move with it or it stops
    // guarding. A pin whose function is gone entirely is left to the
    // review that removed the function.
    let mut drifted: Vec<&(&str, usize)> = sanctioned
        .iter()
        .filter(|(name, pinned)| {
            let count = pinned_count(&seen, name).unwrap_or(0);
            functions.iter().any(|(_, defined)| defined == name) && count < *pinned // more than pinned was already reported
        })
        .collect();
    drifted.sort_by_key(|(name, _)| *name);
    for (name, pinned) in drifted {
        violations.push(format!(
            "{file}: {name} is pinned for {pinned} acquisition(s) but has {}; update {PIN_SITE}",
            pinned_count(&seen, name).unwrap_or(0),
        ));
    }
    violations
}

fn pinned_count(pins: &[(&str, usize)], name: &str) -> Option<usize> {
    pins.iter()
        .find(|(pinned, _)| *pinned == name)
        .map(|(_, count)| *count)
}

/// Every `fn name` position, for attributing a call site to its nearest
/// preceding function.
fn functions(source: &[u8]) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for at in 0..source.len() {
        if !source[at..].starts_with(b"fn") {
            continue;
        }
        if at > 0 && is_word(source[at - 1]) {
            continue;
        }
        let mut cursor = at + 2;
        let space = cursor;
        while source.get(cursor).is_some_and(|&byte| is_space(byte)) {
            cursor += 1;
        }
        if cursor == space {
            continue;
        }
        if source[cursor..].starts_with(b"r#") {
            cursor += 2;
        }
        let start = cursor;
        while source.get(cursor).is_some_and(|&byte| is_word(byte)) {
            cursor += 1;
        }
        if cursor == start {
            continue;
        }
        found.push((
            at,
            String::from_utf8_lossy(&source[start..cursor]).into_owned(),
        ));
    }
    found
}

/// Offsets of `.name(` / `::name(` spellings (`with_call`), or of the
/// path-form `::name` with no call parens.
fn method_calls(source: &[u8], name: &[u8], with_call: bool) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let Some(after) = method_at(source, at, name, 1, with_call) else {
            at += 1;
            continue;
        };
        found.push(at);
        at = after;
    }
    found
}

fn path_items(source: &[u8], name: &[u8]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 1 < source.len() {
        if &source[at..at + 2] != b"::" {
            at += 1;
            continue;
        }
        let Some(after) = method_at(source, at, name, 2, false) else {
            at += 1;
            continue;
        };
        found.push(at);
        at = after;
    }
    found
}

/// One past `name` when it follows `source[at..at + separator]`
/// (optionally through `r#`) and is or is not followed by a call paren.
fn method_at(
    source: &[u8],
    at: usize,
    name: &[u8],
    separator: usize,
    with_call: bool,
) -> Option<usize> {
    if separator == 1 && source[at] != b'.' && source[at] != b':' {
        return None;
    }
    let mut cursor = at + separator;
    while source.get(cursor).is_some_and(|&byte| is_space(byte)) {
        cursor += 1;
    }
    if source[cursor..].starts_with(b"r#") {
        cursor += 2;
    }
    if !source[cursor..].starts_with(name) {
        return None;
    }
    let after = cursor + name.len();
    if source.get(after).is_some_and(|&byte| is_word(byte)) {
        return None;
    }
    let mut paren = after;
    while source.get(paren).is_some_and(|&byte| is_space(byte)) {
        paren += 1;
    }
    if (source.get(paren) == Some(&b'(')) != with_call {
        return None;
    }
    Some(after)
}

/// Every banned accessor, in source order, with or without a receiver.
///
/// The receiver is optional, so a longer identifier ending in one of the
/// names matches too - deliberately loud rather than deliberately
/// precise: this is the seam where a bucket could appear without the
/// `bucket` token, so a near-miss spelling should stop the build.
fn banned_accessors(source: &[u8]) -> Vec<(usize, &'static str)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let mut cursor = at;
        if source[cursor] == b'.' || source[cursor] == b':' {
            cursor += 1;
            while source.get(cursor).is_some_and(|&byte| is_space(byte)) {
                cursor += 1;
            }
        }
        if source[cursor..].starts_with(b"r#") {
            cursor += 2;
        }
        let matched = BANNED_ACCESSORS.into_iter().find(|accessor| {
            source[cursor..].starts_with(accessor.as_bytes())
                && !source
                    .get(cursor + accessor.len())
                    .is_some_and(|&byte| is_word(byte))
        });
        let Some(accessor) = matched else {
            at += 1;
            continue;
        };
        found.push((at, accessor));
        at = cursor + accessor.len();
    }
    found
}
