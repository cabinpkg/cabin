//! GCC banner version extraction.

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_version(lines: &[&str]) -> Option<CompilerVersion> {
    // GCC's first line typically looks like
    //   "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0"
    // The version we care about is the last whitespace-delimited
    // token; some distros add a trailing copyright suffix on the
    // same line, so we accept the *last* dotted-numeric token.
    let first = lines.first()?;
    first
        .split_whitespace()
        .filter_map(|tok| {
            let trimmed = tok.trim_end_matches(',');
            CompilerVersion::parse(trimmed)
        })
        .next_back()
}
