//! GNU `ar` / `llvm-ar` banner version extraction.

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_gnu_version(lines: &[&str]) -> Option<CompilerVersion> {
    // GNU ar prints e.g.
    // "GNU ar (GNU Binutils for Debian) 2.40"
    let first = lines.first()?;
    first
        .split_whitespace()
        .filter_map(|tok| CompilerVersion::parse(tok.trim_end_matches(',')))
        .next_back()
}

pub(super) fn parse_llvm_version(lines: &[&str]) -> Option<CompilerVersion> {
    // llvm-ar emits multi-line output; somewhere is e.g.
    // "LLVM version 17.0.6"
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(idx) = lower.find("llvm version ") {
            let after = &line[idx + "llvm version ".len()..];
            if let Some(token) = after.split_whitespace().next()
                && let Some(v) = CompilerVersion::parse(token)
            {
                return Some(v);
            }
        }
    }
    None
}
