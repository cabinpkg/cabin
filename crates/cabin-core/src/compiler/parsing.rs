//! Pure parsers for compiler / archiver `--version` output.

use super::identity::{
    ArchiverIdentity, ArchiverKind, CompilerIdentity, CompilerKind, CompilerVersion,
};

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
    // this pure parse. The arm is kept exhaustive regardless.
    let kind = detect_cxx_kind(&lines);
    let version = match kind {
        CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::ClangCl => {
            parse_clang_version(&lines)
        }
        CompilerKind::Gcc => parse_gcc_version(&lines),
        CompilerKind::Msvc => parse_msvc_version(&lines),
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
    if lower.contains("apple clang") {
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
    {
        return CompilerKind::Gcc;
    }
    CompilerKind::Unknown
}

fn parse_clang_version(lines: &[&str]) -> Option<CompilerVersion> {
    let first = lines.first()?;
    let lower = first.to_ascii_lowercase();
    let needle = if lower.starts_with("apple clang") {
        "apple clang version "
    } else {
        "clang version "
    };
    let idx = lower.find(needle)?;
    let after = &first[idx + needle.len()..];
    let token = after
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',');
    CompilerVersion::parse(token)
}

fn parse_gcc_version(lines: &[&str]) -> Option<CompilerVersion> {
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

fn parse_msvc_version(lines: &[&str]) -> Option<CompilerVersion> {
    let joined = lines.join(" ");
    let lower = joined.to_ascii_lowercase();
    let idx = lower.find("version ")?;
    let after = &joined[idx + "version ".len()..];
    let token = after.split_whitespace().next().unwrap_or("");
    CompilerVersion::parse(token)
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

/// Pure parser for archiver `--version` output. The recognized
/// families (`ar` and `llvm-ar`) print one line that includes the
/// family name. Anything else is classified as
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
        ArchiverKind::LlvmAr => parse_llvm_ar_version(&lines),
        ArchiverKind::Ar => parse_gnu_ar_version(&lines),
        ArchiverKind::Lib => parse_msvc_version(&lines),
        ArchiverKind::Unknown => None,
    };

    ArchiverIdentity {
        kind,
        version,
        raw_version_line: truncate(&first_line, 256),
    }
}

fn parse_gnu_ar_version(lines: &[&str]) -> Option<CompilerVersion> {
    // GNU ar prints e.g.
    //   "GNU ar (GNU Binutils for Debian) 2.40"
    let first = lines.first()?;
    first
        .split_whitespace()
        .filter_map(|tok| CompilerVersion::parse(tok.trim_end_matches(',')))
        .next_back()
}

fn parse_llvm_ar_version(lines: &[&str]) -> Option<CompilerVersion> {
    // llvm-ar emits multi-line output; somewhere is e.g.
    //   "LLVM version 17.0.6"
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
