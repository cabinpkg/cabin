//! GCC banner version extraction.
//!
//! The first line is `<driver> (<vendor string>) <version>
//! [<YYYYMMDD date>] [(<vendor patch>)]`. The version is anchored
//! as the first dotted-numeric token *after* the vendor
//! parenthetical; a build date has no dots and is skipped, and the
//! trailing vendor-patch parenthetical is never reached when a
//! clean version precedes it. Lines without a parenthetical fall
//! back to a whole-line scan.

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_version(lines: &[&str]) -> Option<CompilerVersion> {
    let first = lines.first()?;
    let after_paren = first.find(')').map(|i| &first[i + 1..]);
    for segment in after_paren.into_iter().chain(std::iter::once(*first)) {
        for tok in segment.split_whitespace() {
            let cleaned = tok.trim_matches(|c: char| matches!(c, '(' | ')' | ','));
            if !cleaned.contains('.') {
                continue;
            }
            if let Some(v) = CompilerVersion::parse_with_suffix(cleaned) {
                return Some(v);
            }
        }
    }
    None
}
