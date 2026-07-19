//! Shared `$PATH` executable lookup.
//!
//! Both compiler/archiver resolution ([`crate::resolve`]) and
//! compiler-wrapper resolution ([`crate::wrapper`]) need the same
//! "walk `$PATH`, probe each candidate, then retry with the platform
//! `EXE_SUFFIX`" search.  Keeping it in one place ensures the two paths
//! agree on which binary on `PATH` is selected - a security-relevant
//! decision - and on the Windows `.exe`-suffix fallback.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Walk the `PATH` entries returned by `env`, returning the first
/// `name` candidate that `probe` accepts (trying the bare name first,
/// then the platform `EXE_SUFFIX`).  Empty `PATH` entries are skipped.
pub(crate) fn search_path<F, P>(name: &str, env: &F, probe: &P) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString> + ?Sized,
    P: Fn(&Path) -> bool + ?Sized,
{
    let path_var = env("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if probe(&candidate) {
            return Some(candidate);
        }
        if let Some(found) = find_with_exe_suffix(&candidate, probe) {
            return Some(found);
        }
    }
    None
}

/// Retry `path` with the platform's `EXE_SUFFIX` appended (e.g.
/// `cc` → `cc.exe` on Windows).  Returns `None` on platforms with an
/// empty suffix or when the suffixed path is not accepted by `probe`.
pub(crate) fn find_with_exe_suffix<P>(path: &Path, probe: &P) -> Option<PathBuf>
where
    P: Fn(&Path) -> bool + ?Sized,
{
    let suffix = std::env::consts::EXE_SUFFIX;
    if suffix.is_empty() {
        return None;
    }
    let mut name: OsString = path.file_name()?.to_owned();
    name.push(suffix);
    let with_suffix = path.with_file_name(name);
    if probe(&with_suffix) {
        Some(with_suffix)
    } else {
        None
    }
}

/// Whether `name` lexically looks like a path rather than a bare
/// program name to look up on `PATH` - it contains a `/` separator
/// (or a `\` on Windows).  Resolution probes such values as explicit
/// paths instead of walking `PATH`, so [`crate::resolve`] and
/// [`crate::ninja`] must apply the same rule.
pub(crate) fn looks_like_relative_path(name: &str) -> bool {
    name.contains('/') || (cfg!(windows) && name.contains('\\'))
}
