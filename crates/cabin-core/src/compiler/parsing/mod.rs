//! Pure parsers for compiler / archiver `--version` output.
//!
//! Family-specific banner recognition and version extraction live
//! in one submodule per compiler family (`CMake`'s
//! `Modules/Compiler/*` organization is the reference); this
//! module owns the classification dispatch and the shared
//! line-shaping helpers.

mod apple;
mod ar;
mod clang;
mod gcc;
mod msvc;

use super::identity::{ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind};

/// Pure parser for compiler `--version` output.
///
/// Recognizes the canonical first-line shapes Cabin cares about:
///
/// - `clang version 17.0.6 (...)`
/// - `Apple clang version 14.0.3 (clang-1403.0.22.14.1)`
/// - `g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0`
/// - `Microsoft (R) C/C++ Optimizing Compiler Version 19.39.x`
/// - any other first non-empty line → [`CompilerKind::Unknown`].
///
/// Also picks up the `Target: aarch64-apple-darwin` / similar
/// follow-up line when present so metadata can show the
/// compiler-reported target without running additional probes.
pub fn parse_cxx_version_output(text: &str) -> CompilerIdentity {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let first_line = lines.first().copied().unwrap_or("").to_owned();

    // `detect_cxx_kind` classifies from the `--version` banner alone
    // and never returns `ClangCl` (whose banner is a clang version);
    // the detector reclassifies `clang-cl` by its invoked name after
    // this pure parse.  The arm is kept exhaustive regardless.
    let kind = detect_cxx_kind(&lines);
    let version = match kind {
        CompilerKind::Clang | CompilerKind::ClangCl => clang::parse_version(&lines),
        CompilerKind::AppleClang => apple::parse_version(&lines),
        CompilerKind::Gcc => gcc::parse_version(&lines),
        CompilerKind::Msvc => msvc::parse_version(&lines),
        CompilerKind::Unknown => None,
    };
    let target = parse_target_line(&lines);

    CompilerIdentity {
        kind,
        version,
        target,
        raw_version_line: truncate(&first_line, 256),
    }
}

fn detect_cxx_kind(lines: &[&str]) -> CompilerKind {
    let joined = lines.join("\n");
    let lower = joined.to_ascii_lowercase();
    if lower.contains("apple clang") || lower.contains("apple llvm version") {
        return CompilerKind::AppleClang;
    }
    if lower.contains("clang version")
        || lower.contains("clang++")
        || lower.contains("openbsd clang")
    {
        return CompilerKind::Clang;
    }
    if lower.contains("microsoft (r)") || lower.contains("microsoft c/c++") {
        return CompilerKind::Msvc;
    }
    if lower.contains("free software foundation")
        || lower.starts_with("g++")
        || lower.starts_with("gcc")
        || lower.contains("gnu c++")
        || lower.contains("(gcc)")
    {
        return CompilerKind::Gcc;
    }
    CompilerKind::Unknown
}

fn parse_target_line(lines: &[&str]) -> Option<String> {
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Target:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// Pure parser for archiver `--version` output.  The recognized
/// families (`ar` and `llvm-ar`) print one line that includes the
/// family name.  Anything else is classified as
/// [`ArchiverKind::Unknown`]; archivers that exit non-zero on
/// `--version` are left to the subprocess layer to surface as
/// `Unknown`.
pub fn parse_ar_version_output(text: &str) -> ArchiverIdentity {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let first_line = lines.first().copied().unwrap_or("").to_owned();
    let lower = lines.join("\n").to_ascii_lowercase();

    let kind = if lower.contains("llvm version") || lower.contains("llvm-ar") {
        ArchiverKind::LlvmAr
    } else if lower.contains("gnu ar") || lower.contains("gnu binutils") || lower.starts_with("ar ")
    {
        ArchiverKind::Ar
    } else if lower.contains("microsoft (r) library manager") || lower.contains("lib.exe") {
        ArchiverKind::Lib
    } else {
        ArchiverKind::Unknown
    };

    let version = match kind {
        ArchiverKind::LlvmAr => ar::parse_llvm_version(&lines),
        ArchiverKind::Ar => ar::parse_gnu_version(&lines),
        ArchiverKind::Lib => msvc::parse_version(&lines),
        ArchiverKind::Unknown => None,
    };

    ArchiverIdentity {
        kind,
        version,
        raw_version_line: truncate(&first_line, 256),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    s[..end].to_owned()
}
