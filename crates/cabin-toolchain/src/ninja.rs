use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::error::ToolchainError;
use crate::path_search::{find_with_exe_suffix, looks_like_relative_path, search_path};

/// Detect a usable Ninja binary on the host.
///
/// # Errors
/// Returns [`ToolchainError::BadEnvOverride`] when the `NINJA`
/// environment variable is set but does not point at an executable,
/// and [`ToolchainError::NoNinja`] when no `ninja` binary is found on
/// `PATH`.
pub fn locate_ninja() -> Result<PathBuf, ToolchainError> {
    find_ninja(&|v| std::env::var_os(v))
}

// Not `resolve::locate`: `NINJA` may name a non-UTF-8 path, and the
// MSVC registry fallback there must not hand back `cl.exe` as Ninja.
fn find_ninja(env: &dyn Fn(&str) -> Option<OsString>) -> Result<PathBuf, ToolchainError> {
    let probe = Path::is_file;
    let Some(value) = env("NINJA") else {
        return search_path(OsStr::new("ninja"), env, &probe).ok_or(ToolchainError::NoNinja);
    };
    let path = Path::new(&value);
    let found = if path.is_absolute() || looks_like_relative_path(&value.to_string_lossy()) {
        if probe(path) {
            Some(path.to_path_buf())
        } else {
            find_with_exe_suffix(path, &probe)
        }
    } else {
        search_path(&value, env, &probe)
    };
    found.ok_or_else(|| ToolchainError::BadEnvOverride {
        var: "NINJA".to_owned(),
        value: value.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(map: HashMap<&'static str, OsString>) -> impl Fn(&str) -> Option<OsString> {
        move |k| map.get(k).cloned()
    }

    #[cfg(unix)]
    fn make_executable(dir: &assert_fs::TempDir, name: &str) -> PathBuf {
        use assert_fs::prelude::*;
        use std::os::unix::fs::PermissionsExt;
        let child = dir.child(name);
        child.write_str("#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(child.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(child.path(), perms).unwrap();
        child.path().to_path_buf()
    }

    #[cfg(unix)]
    #[test]
    fn env_override_uses_the_provided_value() {
        let dir = assert_fs::TempDir::new().unwrap();
        let ninja = make_executable(&dir, "my-ninja");

        let mut env = HashMap::new();
        env.insert("NINJA", OsString::from(ninja.to_str().unwrap()));
        env.insert("PATH", OsString::from(""));

        let found = find_ninja(&fake_env(env)).unwrap();
        assert_eq!(found, ninja);
    }

    // Linux only: macOS file systems reject non-UTF-8 names.
    #[cfg(target_os = "linux")]
    #[test]
    fn env_override_accepts_a_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let dir = assert_fs::TempDir::new().unwrap();
        let ninja = make_executable(&dir, "ninja");
        let renamed = dir.path().join(OsStr::from_bytes(b"ninja-\xff"));
        std::fs::rename(&ninja, &renamed).unwrap();

        let mut env = HashMap::new();
        env.insert("NINJA", renamed.as_os_str().to_owned());
        env.insert("PATH", OsString::from(""));

        assert_eq!(find_ninja(&fake_env(env)).unwrap(), renamed);
    }

    #[test]
    fn env_override_pointing_at_missing_file_errors() {
        let dir = assert_fs::TempDir::new().unwrap();
        let missing = dir.path().join("missing-ninja");

        let mut env = HashMap::new();
        env.insert("NINJA", missing.as_os_str().to_owned());
        env.insert("PATH", OsString::from(""));

        let err = find_ninja(&fake_env(env)).unwrap_err();
        match err {
            ToolchainError::BadEnvOverride { var, value } => {
                assert_eq!(var, "NINJA");
                assert_eq!(value, missing.to_string_lossy());
            }
            ToolchainError::NoNinja => panic!("expected BadEnvOverride, got NoNinja"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn searches_path_when_env_unset() {
        let dir = assert_fs::TempDir::new().unwrap();
        let ninja = make_executable(&dir, "ninja");

        let mut env = HashMap::new();
        env.insert("PATH", OsString::from(dir.path().to_str().unwrap()));

        let found = find_ninja(&fake_env(env)).unwrap();
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&ninja).unwrap()
        );
    }

    #[test]
    fn returns_not_found_when_nothing_matches() {
        let mut env = HashMap::new();
        env.insert("PATH", OsString::from(""));
        let err = find_ninja(&fake_env(env)).unwrap_err();
        assert!(matches!(err, ToolchainError::NoNinja));
    }
}
