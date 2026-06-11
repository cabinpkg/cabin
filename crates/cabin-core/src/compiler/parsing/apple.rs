//! Apple clang (Xcode) banner version extraction, covering both
//! the modern `Apple clang version N.N.N` banner and the
//! pre-Xcode-10 `Apple LLVM version N.N.N` era. Both number
//! spaces are Apple's own (not LLVM release numbers).

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_version(lines: &[&str]) -> Option<CompilerVersion> {
    let first = lines.first()?;
    let lower = first.to_ascii_lowercase();
    for needle in ["apple clang version ", "apple llvm version "] {
        if let Some(idx) = lower.find(needle) {
            let token = first[idx + needle.len()..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');
            return CompilerVersion::parse_with_suffix(token);
        }
    }
    None
}
