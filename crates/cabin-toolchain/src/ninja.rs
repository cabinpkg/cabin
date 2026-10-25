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
    find_command_with_env(|v| std::env::var_os(v), "NINJA", &["ninja"])
}

/// Locate an executable, honoring an environment variable override and
/// falling back to a list of candidate names searched on `PATH`.  The
/// environment getter is injectable so the helper can be tested without
/// mutating real process environment.
fn find_command_with_env<F>(
    env: F,
    env_var: &str,
    fallbacks: &[&str],
) -> Result<PathBuf, ToolchainError>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = env(env_var) {
        if let Some(path) = locate(&env, value.as_os_str()) {
            return Ok(path);
        }
        return Err(ToolchainError::BadEnvOverride {
            var: env_var.to_owned(),
            value: value.to_string_lossy().into_owned(),
        });
    }
    for &candidate in fallbacks {
        if let Some(path) = locate(&env, OsStr::new(candidate)) {
            return Ok(path);
        }
    }
    Err(ToolchainError::NoNinja)
}

fn locate<F>(env: &F, name: &OsStr) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    let probe = |p: &Path| p.is_file();
    let path = Path::new(name);
    if path.is_absolute() || looks_like_relative_path(&name.to_string_lossy()) {
        if probe(path) {
            return Some(path.to_path_buf());
        }
        return find_with_exe_suffix(path, &probe);
    }
    search_path(name, env, &probe)
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

        let found = find_command_with_env(fake_env(env), "NINJA", &["ninja"]).unwrap();
        assert_eq!(found, ninja);
    }

    #[test]
    fn env_override_pointing_at_missing_file_errors() {
        let dir = assert_fs::TempDir::new().unwrap();
        let missing = dir.path().join("missing-ninja");

        let mut env = HashMap::new();
        env.insert("NINJA", missing.as_os_str().to_owned());
        env.insert("PATH", OsString::from(""));

        let err = find_command_with_env(fake_env(env), "NINJA", &["ninja"]).unwrap_err();
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

        let found = find_command_with_env(fake_env(env), "NINJA", &["ninja"]).unwrap();
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&ninja).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn falls_through_candidates_in_order() {
        let dir = assert_fs::TempDir::new().unwrap();
        let ninja = make_executable(&dir, "ninja");

        let mut env = HashMap::new();
        env.insert("PATH", OsString::from(dir.path().to_str().unwrap()));

        let found =
            find_command_with_env(fake_env(env), "NINJA", &["ninja-build", "ninja4", "ninja"])
                .unwrap();
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&ninja).unwrap()
        );
    }

    #[test]
    fn returns_not_found_when_nothing_matches() {
        let mut env = HashMap::new();
        env.insert("PATH", OsString::from(""));
        let err = find_command_with_env(
            fake_env(env),
            "NINJA",
            &["definitely-not-a-real-ninja-9999"],
        )
        .unwrap_err();
        assert!(matches!(err, ToolchainError::NoNinja));
    }
}
