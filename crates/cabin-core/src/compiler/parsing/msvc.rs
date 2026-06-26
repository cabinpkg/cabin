//! MSVC (`cl.exe` / `lib.exe`) banner version extraction.  Shared
//! by the compiler and archiver paths - Microsoft's library
//! manager prints the same `... Version N.N.N ...` shape.

use crate::compiler::identity::CompilerVersion;

pub(super) fn parse_version(lines: &[&str]) -> Option<CompilerVersion> {
    let joined = lines.join(" ");
    let lower = joined.to_ascii_lowercase();
    let idx = lower.find("version ")?;
    let after = &joined[idx + "version ".len()..];
    let token = after.split_whitespace().next().unwrap_or("");
    CompilerVersion::parse(token)
}
