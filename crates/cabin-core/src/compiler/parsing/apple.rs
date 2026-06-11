//! Apple clang (Xcode) banner version extraction.

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_version(lines: &[&str]) -> Option<CompilerVersion> {
    let first = lines.first()?;
    let lower = first.to_ascii_lowercase();
    let needle = "apple clang version ";
    let idx = lower.find(needle)?;
    let after = &first[idx + needle.len()..];
    let token = after
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',');
    CompilerVersion::parse(token)
}
