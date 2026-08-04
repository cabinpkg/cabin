//! The agent Stop-hook adapter (Claude Code / Codex): reads the hook
//! JSON on stdin, always exits 0, and prints `{}` on success or a
//! decision naming the failed step.
//!
//! Two rules the shell established and this keeps:
//!
//! - stdout carries ONLY the JSON.  Every byte the gate produced is
//!   replayed to stderr, success included;
//! - every path exits 0.  A non-zero exit from a Stop hook reads as
//!   "the hook crashed", not as a decision, so the verdict has to
//!   travel in the body.  That includes a panic, which would
//!   otherwise exit 101.

use std::fmt::Write as _;
use std::io::Read as _;

/// One blocked stop per natural stop.  `stop_hook_active` means we
/// already blocked once, and blocking again on an unfixable failure
/// would loop the agent through the full gate forever.
///
/// Matched per line over the raw text, as the shell's
/// `grep '"stop_hook_active"[[:space:]]*:[[:space:]]*true'` matched
/// it, rather than parsed.  The difference is deliberate and runs both
/// ways: a parser would find the key in pretty-printed JSON that the
/// grep misses (allowing a stop the shell blocked), and would miss a
/// nested occurrence the grep finds.  Keeping the grep keeps the
/// behavior, and keeping it infallible means malformed JSON cannot
/// take the hook down.
/// POSIX `[[:space:]]` in the C locale, which is what the shell's grep
/// accepted between the key, the colon and `true`.  Trimming only
/// space and tab would treat a carriage return as a first stop and
/// block twice.
const BLANK: [char; 6] = [' ', '\t', '\n', '\r', '\u{b}', '\u{c}'];

#[must_use]
pub fn already_blocked(input: &str) -> bool {
    input.lines().any(|line| {
        line.match_indices("\"stop_hook_active\"").any(|(at, key)| {
            let rest = line[at + key.len()..].trim_start_matches(BLANK);
            rest.strip_prefix(':')
                .map(|rest| rest.trim_start_matches(BLANK))
                .is_some_and(|rest| rest.starts_with("true"))
        })
    })
}

/// The step a failed gate died at: the last `==> ` line of its output.
///
/// `await_phases` re-prints a failed phase's marker *after* its
/// output, so this names the phase that failed rather than the last
/// one launched.  A log with no marker at all means the gate died
/// before its first step.
#[must_use]
pub fn failed_step(log: &str) -> String {
    log.lines()
        .filter_map(|line| line.strip_prefix("==> "))
        .next_back()
        .filter(|step| !step.is_empty())
        .map_or_else(
            || "cargo ci (failed before the first step)".to_owned(),
            str::to_owned,
        )
}

/// The JSON body for a gate verdict.
#[must_use]
pub fn decision(passed: bool, step: &str, already_blocked: bool) -> String {
    if passed {
        return "{}".to_owned();
    }
    if already_blocked {
        return format!(
            "{{\"systemMessage\":\"cargo ci is still failing at: {} \
             (stop allowed to avoid a hook loop; rerun it manually)\"}}",
            escape(step)
        );
    }
    format!(
        "{{\"decision\":\"block\",\"reason\":\"Local CI failed at: {}. \
         Run cargo ci, fix the failures, and rerun it until it passes \
         before stopping.\"}}",
        escape(step)
    )
}

/// The step name is a phase label this repository writes, not
/// arbitrary compiler output - the shell kept the reason a fixed
/// ASCII template for exactly that reason, since one bad escape makes
/// the whole hook output invalid.  Escaping it anyway costs nothing
/// and removes the constraint.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control < ' ' => {
                let _ = write!(escaped, "\\u{:04x}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Everything on stdin, or the empty string.  A closed or absent fd is
/// not an error: the shell's `$(cat || true)` treated it as empty, and
/// empty input falls through to the blocking branch, which is the safe
/// direction.
#[must_use]
pub fn read_stdin() -> String {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_flag_is_matched_per_line_as_the_grep_matched_it() {
        assert!(already_blocked(r#"{"stop_hook_active":true}"#));
        assert!(already_blocked(r#"{"stop_hook_active": true}"#));
        assert!(already_blocked("{\"stop_hook_active\"\t:\ttrue}"));
        // Nested at any depth, because the grep had no structure.
        assert!(already_blocked(r#"{"a":{"stop_hook_active":true}}"#));

        assert!(!already_blocked(r#"{"stop_hook_active":false}"#));
        assert!(!already_blocked(r#"{"my_stop_hook_active":true}"#));
        assert!(!already_blocked("{}"));
        assert!(!already_blocked(""));
        // A JSON string cannot hold a raw quote, so an escaped mention
        // never matches - the grep was accidentally robust here.
        assert!(!already_blocked(r#"{"x":"{\"stop_hook_active\": true}"}"#));
        // Line-oriented: the shell's `[[:space:]]*` never spanned a
        // newline, and neither does this - `lines()` has already split
        // it, so the key and the value are never on one line together.
        assert!(!already_blocked("{\"stop_hook_active\":\n  true}"));
        // ...but every other blank the character class covers is
        // accepted on a single line.
        assert!(already_blocked("{\"stop_hook_active\"\r:\r true}"));
        assert!(already_blocked("{\"stop_hook_active\"\u{b}:\u{c}true}"));
    }

    #[test]
    fn the_failed_step_is_the_last_marker() {
        assert_eq!(failed_step("==> one\n==> two\n"), "two");
        assert_eq!(
            failed_step("==> clippy (started)\nboom\n\n==> clippy\n"),
            "clippy"
        );
        assert_eq!(
            failed_step("nothing here"),
            "cargo ci (failed before the first step)"
        );
        assert_eq!(failed_step(""), "cargo ci (failed before the first step)");
        // A line that only looks like a marker carries no name.
        assert_eq!(
            failed_step("==> "),
            "cargo ci (failed before the first step)"
        );
    }

    #[test]
    fn a_passing_gate_says_nothing() {
        assert_eq!(decision(true, "anything", false), "{}");
        assert_eq!(decision(true, "anything", true), "{}");
    }

    #[test]
    fn a_failing_gate_blocks_once_and_then_only_warns() {
        let blocked = decision(false, "cargo clippy", false);
        assert!(blocked.contains(r#""decision":"block""#));
        assert!(blocked.contains("Local CI failed at: cargo clippy."));

        let warned = decision(false, "cargo clippy", true);
        assert!(!warned.contains("\"decision\""), "{warned}");
        assert!(warned.contains("systemMessage"));
        assert!(warned.contains("stop allowed to avoid a hook loop"));
    }

    /// A step name carrying a quote must not break the body, which is
    /// the failure the shell avoided by never interpolating one.
    #[test]
    fn a_quoted_step_name_stays_inside_the_string() {
        let body = decision(false, "npm run \"lint\"\n", false);
        assert!(body.contains(r#"npm run \"lint\"\n"#), "{body}");
        assert_eq!(body.matches('"').count() % 2, 0, "unbalanced: {body}");
    }
}
