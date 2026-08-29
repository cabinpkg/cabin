//! The SQL consolidation guard (`registry/docs/architecture.md`, "Why no
//! ORM").
//!
//! Every SQL statement the Worker executes must live in `src/sql/`,
//! where `tests/sql_validation/` prepares it against the real migrated
//! schema.  This guard keeps executed SQL from growing outside that
//! module: the two literal patterns below must never appear, every
//! `prepare()` call must name a `sql::` const, and D1's unprepared
//! escape hatch (`exec`) is rejected outright.  The last two are a
//! lexical scan rather than a line match, so no comment can hide a call
//! or fake one and no multi-line spelling slips through.
//!
//! Three diagnostic details differ from the Perl guard this replaces,
//! none of them changing which call sites are accepted:
//!
//! - a violation names the line its call starts on. The Perl computed
//!   that line from `@-` *after* normalizing the argument's whitespace,
//!   which silently reset the offsets, so any call whose argument
//!   contained whitespace was reported on an unrelated line;
//! - within each pass, violations come out sorted by path rather than in
//!   `find`'s directory order. The passes themselves keep their order:
//!   each literal pattern over the whole tree, then the lexical scan;
//! - bytes that are not valid UTF-8 are rendered lossily rather than
//!   emitted raw, in an echoed argument and in a literal-pattern match
//!   alike.
//!
//! The governor carve-outs below are also tightened: the Perl matched
//! any path ENDING in `src/governor.rs`, so a nested `src/x/src/governor.rs`
//! inherited them.  This matches the reported path exactly.
//!
//! ponytail: a lexical scan, not a Rust parser, so it has no receiver
//! types - an unrelated `prepare`/`exec` method on some other receiver
//! would be flagged too (loudly, at the call site: rename it or teach
//! the scanner), and a call assembled by a macro would pass.  It is a
//! regression tripwire for ordinary contributions - deliberate evasion
//! is a code-review question, and the statements still have to work - so
//! make it syntax-aware only if that stops holding.

use std::path::Path;

use anyhow::Result;

use crate::lexical::blank_comments_and_strings;
use crate::source::{is_space, is_word, line_of, matching_lines, rust_sources};

/// The two commissioned literal patterns, matched on the source as
/// written (comments and strings included) rather than on the blanked
/// copy the scans below use.
const LITERAL_PATTERNS: [&[u8]; 2] = [b"prepare(\"", b"prepare(&format!"];

/// How much of the call's argument the scan reads to classify it, and
/// how much of that it echoes back in a violation.
const ARGUMENT_LOOKAHEAD: usize = 400;
const ARGUMENT_REPORTED: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Prepare,
    Exec,
}

impl Method {
    fn name(self) -> &'static [u8] {
        match self {
            Self::Prepare => b"prepare",
            Self::Exec => b"exec",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Exec => "exec",
        }
    }
}

/// Every violation under `registry_dir`, as the diagnostic lines the
/// guard prints.  An empty result means the guard accepted the tree.
///
/// # Errors
///
/// Fails when `registry_dir/src` cannot be read.  Unlike the shell
/// guard this replaces - whose `find` error drained into an empty
/// argument list and a silent pass - an unreadable source tree refuses
/// rather than reporting nothing.
pub fn check(registry_dir: &Path) -> Result<Vec<String>> {
    let src = registry_dir.join("src");
    let mut violations = Vec::new();
    for pattern in LITERAL_PATTERNS {
        violations.extend(matching_lines(&src, "src", pattern)?);
    }
    for source in rust_sources(&src, "src")? {
        let blanked = blank_comments_and_strings(&std::fs::read(&source.path)?);
        violations.extend(scan(&blanked, &source.relative));
    }
    Ok(violations)
}

/// The violations in one blanked source file.
fn scan(source: &[u8], file: &str) -> Vec<String> {
    let mut violations = Vec::new();
    // The governor's Durable Object SQLite statements have their own
    // consolidated home with the same assurance model as sql/: every
    // statement is a module-local const in src/governor.rs, executed by
    // the engine and prepared against the real governor schema by its
    // host tests.  So src/governor.rs may exec a bare SCREAMING_CASE
    // const, and src/governor_do.rs - the storage adapter the engine
    // runs through - may exec exactly its pass-through parameters
    // (`sql`, the engine's statement; `statement`, the schema loop) or a
    // named const; dynamic and literal spellings stay rejected even
    // there.
    let is_governor = file == "src/governor.rs";
    let is_governor_do = file == "src/governor_do.rs";

    for call in calls(source) {
        let argument = &call.argument;
        let sanctioned = match call.method {
            Method::Prepare => {
                sql_const_argument(argument) || (is_governor && bare_sql_argument(argument))
            }
            Method::Exec => {
                (is_governor && screaming_first_argument(argument))
                    || (is_governor_do && passthrough_first_argument(argument))
            }
        };
        if sanctioned {
            continue;
        }
        let echo = &argument[..argument.len().min(ARGUMENT_REPORTED)];
        violations.push(format!(
            "{file}:{}: {}{}",
            line_of(source, call.offset),
            call.method.as_str(),
            String::from_utf8_lossy(echo),
        ));
    }

    // A path-form method item (`D1Database::prepare` with no call
    // parens) is an alias that would launder every later call past the
    // scan above, so creating one is itself a violation.
    for (offset, method) in aliases(source) {
        violations.push(format!(
            "{file}:{}: {} method alias (path form without a call); \
             call it directly instead",
            line_of(source, offset),
            method.as_str(),
        ));
    }
    violations
}

struct Call {
    offset: usize,
    method: Method,
    /// The whitespace-normalized argument list, read through a bounded
    /// lookahead so an accepted call never consumes the one behind it
    /// and wrapping the call (or commenting its argument) stays
    /// acceptable.
    argument: Vec<u8>,
}

/// Every spelling that can reach the D1 methods - `.prepare(`,
/// `::prepare(`, `r#prepare`, the name split from its receiver or its
/// paren across lines.  The word boundary keeps `prepare_statement` and
/// `execute` out, and requiring a following `(` keeps plain field access
/// (`config.exec`) out.
fn calls(source: &[u8]) -> Vec<Call> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let Some(call) = call_at(source, at) else {
            at += 1;
            continue;
        };
        at = call.0;
        found.push(call.1);
    }
    found
}

/// The call starting at `at`, paired with the offset scanning resumes
/// from (just past the method name: the argument is a lookahead).
fn call_at(source: &[u8], at: usize) -> Option<(usize, Call)> {
    if source[at] != b'.' && source[at] != b':' {
        return None;
    }
    let mut cursor = skip_space(source, at + 1);
    if source[cursor..].starts_with(b"r#") {
        cursor += 2;
    }
    let method = [Method::Prepare, Method::Exec]
        .into_iter()
        .find(|method| source[cursor..].starts_with(method.name()))?;
    let after = cursor + method.name().len();
    if source.get(after).is_some_and(|&byte| is_word(byte)) {
        return None;
    }
    let paren = skip_space(source, after);
    if source.get(paren) != Some(&b'(') {
        return None;
    }
    let end = source.len().min(paren + ARGUMENT_LOOKAHEAD);
    Some((
        after,
        Call {
            offset: at,
            method,
            argument: normalize_space(&source[paren..end]),
        },
    ))
}

/// Every path-form method item: `::` and the name, with no call parens.
fn aliases(source: &[u8]) -> Vec<(usize, Method)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 1 < source.len() {
        if &source[at..at + 2] != b"::" {
            at += 1;
            continue;
        }
        let mut cursor = skip_space(source, at + 2);
        if source[cursor..].starts_with(b"r#") {
            cursor += 2;
        }
        let Some(method) = [Method::Prepare, Method::Exec]
            .into_iter()
            .find(|method| source[cursor..].starts_with(method.name()))
        else {
            at += 1;
            continue;
        };
        let after = cursor + method.name().len();
        if source.get(after).is_some_and(|&byte| is_word(byte)) {
            at += 1;
            continue;
        }
        let paren = skip_space(source, after);
        if source.get(paren) == Some(&b'(') {
            at += 1;
            continue;
        }
        found.push((at, method));
        at = after;
    }
    found
}

fn skip_space(source: &[u8], mut at: usize) -> usize {
    while source.get(at).is_some_and(|&byte| is_space(byte)) {
        at += 1;
    }
    at
}

/// Every run of whitespace collapsed to one space, so a call wrapped
/// across lines classifies exactly like the one-line spelling.
fn normalize_space(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        if is_space(byte) {
            if out.last() != Some(&b' ') {
                out.push(b' ');
            }
        } else {
            out.push(byte);
        }
    }
    out
}

/// A cursor over the head of a normalized argument list.
struct Head<'a>(&'a [u8]);

impl Head<'_> {
    fn literal(&mut self, text: &[u8]) -> bool {
        let matched = self.0.starts_with(text);
        if matched {
            self.0 = &self.0[text.len()..];
        }
        matched
    }

    /// An optional single space - every longer run was already
    /// collapsed to one.
    fn space(&mut self) {
        self.literal(b" ");
    }

    /// `[A-Z][A-Z0-9_]*`, the shape of a statement const's name.
    fn screaming(&mut self) -> bool {
        if !self.0.first().is_some_and(u8::is_ascii_uppercase) {
            return false;
        }
        let end = self.0.iter().position(|byte| {
            !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
        });
        self.0 = &self.0[end.unwrap_or(self.0.len())..];
        true
    }
}

/// `(sql::SOME_CONST)` - the only accepted `prepare` argument.
fn sql_const_argument(argument: &[u8]) -> bool {
    let mut head = Head(argument);
    head.literal(b"(")
        && {
            head.space();
            head.literal(b"sql::")
        }
        && head.screaming()
        && {
            head.space();
            head.literal(b",");
            head.space();
            head.literal(b")")
        }
}

/// `(sql)` - the governor engine's host-test rusqlite adapter forwards
/// the engine's `sql` parameter verbatim; only that exact spelling
/// passes.
fn bare_sql_argument(argument: &[u8]) -> bool {
    let mut head = Head(argument);
    head.literal(b"(")
        && {
            head.space();
            head.literal(b"sql")
        }
        && {
            head.space();
            head.literal(b")")
        }
}

/// `(SOME_CONST, ...)` - the governor engine executes module-local
/// consts.
fn screaming_first_argument(argument: &[u8]) -> bool {
    let mut head = Head(argument);
    head.literal(b"(")
        && {
            head.space();
            head.screaming()
        }
        && {
            head.space();
            head.literal(b",")
        }
}

/// `(sql, ...)`, `(statement, ...)`, or `(SOME_CONST, ...)` - the
/// storage adapter's pass-through parameters and its own consts.
fn passthrough_first_argument(argument: &[u8]) -> bool {
    let mut head = Head(argument);
    if !head.literal(b"(") {
        return false;
    }
    head.space();
    let rest = head.0;
    let named = [b"sql".as_slice(), b"statement".as_slice()]
        .into_iter()
        .any(|name| {
            let mut head = Head(rest);
            head.literal(name) && {
                head.space();
                head.literal(b",")
            }
        });
    named || {
        let mut head = Head(rest);
        head.screaming() && {
            head.space();
            head.literal(b",")
        }
    }
}
