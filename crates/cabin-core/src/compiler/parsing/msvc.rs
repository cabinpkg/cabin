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

/// The triple named by the `cl.exe` banner's trailing architecture
/// word (`... for x64`, localized `... f\u{fc}r ARM64`), the one it
/// builds for.
pub(super) fn parse_target(lines: &[&str]) -> Option<String> {
    let banner = lines.iter().find(|line| line.starts_with("Microsoft"))?;
    let arch = match banner.split_whitespace().last()? {
        "x64" => "x86_64",
        "x86" => "i686",
        "ARM64" => "aarch64",
        "ARM" => "armv7",
        _ => return None,
    };
    Some(format!("{arch}-pc-windows-msvc"))
}
