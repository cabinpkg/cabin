//! Auto-discovery of the MSVC toolchain environment on Windows.
//!
//! When Cabin already runs inside an activated MSVC environment - a
//! *Developer Command Prompt*, or a shell where `vcvarsall.bat` /
//! `msvc-dev-cmd` has exported `INCLUDE` / `LIB` and put `cl.exe` on
//! `PATH` - nothing here runs: the existing environment is used as-is,
//! so a pre-activated build never depends on the discovery path.
//!
//! Otherwise, on Windows, this probes the registry / COM via
//! [`find_msvc_tools`] to locate `cl.exe` and the `INCLUDE` / `LIB` /
//! `PATH` a compile needs, so `cabin build` works without a
//! pre-activated shell.  The probe runs at most once per process and is a
//! no-op on non-Windows hosts.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The MSVC tools and environment discovered for this process.
struct MsvcInstallation {
    /// Absolute path to `cl.exe`.  Its parent directory also holds
    /// `lib.exe` and `link.exe`.
    cl: PathBuf,
    /// Environment overlay (`INCLUDE`, `LIB`, `PATH`) to apply to a child
    /// process that invokes `cl` / `lib` / `link`. `find-msvc-tools`
    /// returns `PATH` with the MSVC directories already prepended to the
    /// inherited `PATH`, so the overlay is applied wholesale.
    env: Vec<(OsString, OsString)>,
}

/// Whether the current process already runs inside an activated MSVC
/// environment.  When it does, Cabin uses that environment unchanged and
/// never probes, so a pre-activated build is unaffected by this module.
fn already_activated() -> bool {
    std::env::var_os("INCLUDE").is_some() && std::env::var_os("LIB").is_some()
}

fn installation() -> Option<&'static MsvcInstallation> {
    static CELL: OnceLock<Option<MsvcInstallation>> = OnceLock::new();
    CELL.get_or_init(|| {
        // Only meaningful on Windows, and only when the environment is
        // not already activated. `find_msvc_tools::find_tool` is a no-op
        // off Windows, but the explicit guard also skips the probe inside
        // an activated shell.
        if !cfg!(windows) || already_activated() {
            return None;
        }
        // `find_tool(arch, tool)`: match the host architecture so the
        // discovered toolset targets the machine Cabin runs on.
        let tool = find_msvc_tools::find_tool(std::env::consts::ARCH, "cl.exe")?;
        Some(MsvcInstallation {
            cl: tool.path().to_path_buf(),
            env: tool.env().into_iter().cloned().collect(),
        })
    })
    .as_ref()
}

/// Whether `candidate` is the `cl.exe` of the MSVC install Cabin
/// auto-discovered on this host.
///
/// Used to decide whether an *explicitly pinned* `cl` path may take the
/// discovered `INCLUDE` / `LIB` / `PATH` overlay: it may exactly when it
/// *is* the discovered install - then the overlay is that compiler's own
/// environment, not a foreign toolset's.  Returns `false` off Windows,
/// inside an already-activated environment, or when nothing was
/// discovered, in all of which the discovered overlay is empty anyway.
pub fn path_is_discovered_msvc_cl(candidate: &Path) -> bool {
    installation().is_some_and(|install| same_file(candidate, &install.cl))
}

/// Whether two paths point at the same file, tolerating case / short-name
/// (`8.3`) / separator differences by canonicalizing both.  A
/// canonicalization failure (e.g. a path that no longer exists) is
/// treated as "not the same", biasing toward *not* applying the
/// discovered overlay - the conservative default.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Resolve an MSVC tool (`cl`, `lib`, or `link`, with or without a
/// `.exe` suffix) to an absolute path via auto-discovery, for use when
/// the tool is not already on `PATH`.
///
/// Returns `None` off Windows, inside an already-activated environment,
/// when no MSVC installation is found, or for any other tool name.
pub fn msvc_tool_path(name: &str) -> Option<PathBuf> {
    let install = installation()?;
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    if !matches!(stem.as_str(), "cl" | "lib" | "link") {
        return None;
    }
    let candidate = install.cl.parent()?.join(format!("{stem}.exe"));
    candidate.is_file().then_some(candidate)
}

/// The environment overlay an MSVC build needs on this host, to apply to
/// the Ninja child (which in turn runs `cl` / `lib`):
///
/// - On Windows, `VSLANG=1033` pins `cl /showIncludes` to its English
///   "Note: including file:" prefix so Ninja's `deps = msvc`
///   header-dependency scan matches it on localized MSVC installs -
///   cc-rs sets the same variable for the same reason.  This is needed
///   whether or not the toolchain was auto-discovered, because a
///   pre-activated localized install emits a localized prefix too.
/// - When Cabin had to discover the toolchain itself (no pre-activated
///   Developer Command Prompt), the auto-located `INCLUDE` / `LIB` /
///   `PATH` so the spawned `cl` / `lib` find the toolchain and headers.
///
/// Callers pass `apply_discovered_install = false` when the user pinned
/// an explicit `cl` path that is *not* the discovered install: a
/// separately discovered install could belong to a *different* Visual
/// Studio toolset, so overlaying its `INCLUDE` / `LIB` onto the user's
/// chosen compiler would mix SDKs.  When the pinned path *is* the
/// discovered install (see [`path_is_discovered_msvc_cl`]) the overlay is
/// that compiler's own environment, so callers pass `true`.  `VSLANG` is
/// applied either way (it only selects the message language, never the
/// toolset).
///
/// Empty off Windows.  On Windows it always carries `VSLANG`; the
/// `INCLUDE` / `LIB` / `PATH` entries are added only when discovery ran
/// and was requested.  Applying it is always safe - non-MSVC tools
/// ignore these variables.
pub fn msvc_environment(apply_discovered_install: bool) -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    if cfg!(windows) {
        env.push((OsString::from("VSLANG"), OsString::from("1033")));
    }
    if apply_discovered_install && let Some(install) = installation() {
        env.extend(install.env.iter().cloned());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn same_file_matches_a_path_with_itself_through_a_detour() {
        let dir = assert_fs::TempDir::new().unwrap();
        let cl = dir.child("cl.exe");
        cl.write_str("").unwrap();
        // The same file reached through a `.` detour canonicalizes to one
        // path, so the differently-spelled forms compare equal.
        let detour = dir.path().join(".").join("cl.exe");
        assert!(same_file(cl.path(), &detour));
    }

    #[test]
    fn same_file_rejects_distinct_files() {
        let dir = assert_fs::TempDir::new().unwrap();
        let a = dir.child("a.exe");
        let b = dir.child("b.exe");
        a.write_str("").unwrap();
        b.write_str("").unwrap();
        assert!(!same_file(a.path(), b.path()));
    }

    #[test]
    fn same_file_rejects_a_path_that_cannot_be_canonicalized() {
        let dir = assert_fs::TempDir::new().unwrap();
        let real = dir.child("cl.exe");
        real.write_str("").unwrap();
        let missing = dir.path().join("missing.exe");
        assert!(!same_file(real.path(), &missing));
    }

    #[test]
    fn path_is_discovered_msvc_cl_is_false_without_a_discovered_install() {
        // Off Windows - and on Windows inside an already-activated shell -
        // nothing is discovered, so no path is ever the discovered `cl`.
        if installation().is_none() {
            assert!(!path_is_discovered_msvc_cl(Path::new("/anywhere/cl.exe")));
        }
    }
}
