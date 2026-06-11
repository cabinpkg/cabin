//! Locate and read Cabin config files in deterministic precedence
//! order.
//!
//! Discovery rules:
//!
//! 1. If `CABIN_NO_CONFIG=1` is set, no config files load.
//! 2. If `CABIN_CONFIG=<path>` is set, exactly one explicit
//!    config file is loaded; missing or unreadable files are a
//!    hard error rather than a silent fallback.
//! 3. Otherwise the user config file (if any) loads first, then
//!    one *workspace-or-package* config file (depending on
//!    whether the start path lives inside a `[workspace]` root).
//!
//! Returned [`crate::LoadedConfigFile`]s are ordered
//! lowest-priority first (user, then workspace/package, then
//! explicit when applicable) so the merger can apply them in a
//! single forward pass.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use camino::{Utf8Path, Utf8PathBuf};

use crate::error::ConfigError;
use crate::parse::parse_config_str;
use crate::source::{ConfigSource, LoadedConfigFile};

/// Environment variable that overrides the user config directory
/// for tests and controlled environments.
pub const CABIN_CONFIG_HOME_ENV: &str = cabin_env::CABIN_CONFIG_HOME;

/// Environment variable that points at one explicit config file.
/// When set to a non-empty value, normal discovery is skipped and
/// only this file is loaded.
pub const CABIN_CONFIG_ENV: &str = cabin_env::CABIN_CONFIG;

/// Environment variable that, when set to `1`, disables every
/// config file load.
pub const CABIN_NO_CONFIG_ENV: &str = cabin_env::CABIN_NO_CONFIG;

/// Workspace context discovery needs from the caller. Cabin's
/// workspace loader provides the same data via
/// `cabin_workspace::PackageGraph` so the CLI builds this struct
/// once and threads it through.
#[derive(Debug, Clone)]
pub struct WorkspaceLayout<'a> {
    /// Directory the entry-point manifest lives in. Either the
    /// workspace root or, for non-workspace projects, the package
    /// root.
    pub root_dir: &'a Path,
    /// Whether `root_dir` carries a `[workspace]` table. This
    /// chooses whether the `<root>/.cabin/config.toml` file is
    /// labeled `Workspace` or `Package` in the loaded list.
    pub is_workspace_root: bool,
}

/// Environment lookup the discovery layer consults. Production
/// callers wrap `std::env::var_os`; tests inject a hash-map-backed
/// closure so they can drive every branch without mutating the
/// process environment.
pub type EnvLookup<'a> = Box<dyn Fn(&str) -> Option<OsString> + 'a>;

/// Inputs the discovery layer takes. Builders should use
/// [`ConfigDiscoveryInputs::from_process`] for production; tests
/// provide their own env lookup and explicit XDG-resolved path.
pub struct ConfigDiscoveryInputs<'a> {
    pub workspace: Option<WorkspaceLayout<'a>>,
    pub env: EnvLookup<'a>,
    /// Pre-resolved XDG user config home for Cabin (already
    /// includes the `cabin` application prefix). The XDG fallback
    /// arm of [`discover_config_files`] reads `<this>/config.toml`.
    ///
    /// Production builds this via the `xdg` crate (see
    /// [`ConfigDiscoveryInputs::from_process`]); tests pass an
    /// explicit path so they exercise the fallback chain without
    /// mutating the process environment.
    ///
    /// Honored only when `CABIN_CONFIG`, `CABIN_NO_CONFIG`, and
    /// `CABIN_CONFIG_HOME` do not short-circuit the lookup; those
    /// Cabin-specific overrides keep their original semantics and
    /// are not routed through XDG.
    pub xdg_user_config_home: Option<PathBuf>,
}

impl<'a> ConfigDiscoveryInputs<'a> {
    /// Inputs that read environment variables from the running
    /// process and consult the supplied workspace layout (when
    /// any). The user config home is the platform base config
    /// directory with a `cabin` suffix: `$XDG_CONFIG_HOME/cabin`
    /// (falling back to `$HOME/.config/cabin`) on Linux,
    /// `~/Library/Application Support/cabin` on macOS, and
    /// `%APPDATA%\cabin` on Windows.
    pub fn from_process(workspace: Option<WorkspaceLayout<'a>>) -> Self {
        Self {
            workspace,
            env: Box::new(|var| std::env::var_os(var)),
            xdg_user_config_home: user_config_home(),
        }
    }
}

/// The user-global Cabin config home: the platform base config
/// directory with a `cabin` suffix. `None` only when no home
/// directory can be determined. The discovery layer appends
/// `config.toml` to whatever this returns.
fn user_config_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("cabin"))
}

/// Discovery report. Splits into the actual loaded files plus a
/// flag the caller can show in `cabin metadata` to explain that
/// `CABIN_NO_CONFIG=1` short-circuited the search. The flag is
/// useful for test harnesses that want to assert "no config was
/// loaded" without re-deriving the env state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiscovery {
    pub loaded_files: Vec<LoadedConfigFile>,
    pub disabled_by_env: bool,
}

/// Discover and read every applicable config file. Files are
/// returned in lowest-priority-first order (user → workspace /
/// package → explicit). A missing discovered file is *not* an
/// error; only files Cabin found and could not read or parse
/// surface a [`ConfigError`].
///
/// # Errors
/// Returns a [`ConfigError`] when a discovered file cannot be read
/// or parsed: [`ConfigError::ExplicitConfigRead`] when a
/// `CABIN_CONFIG` file is missing or unreadable,
/// [`ConfigError::ConfigRead`] when a located user or
/// workspace/package file exists but fails to read (errors other
/// than not-found), [`ConfigError::Parse`] when any read file
/// is not valid Cabin config TOML, and [`ConfigError::NonUtf8Path`]
/// when a discovered config file path is not valid UTF-8.
pub fn discover_config_files(
    inputs: &ConfigDiscoveryInputs<'_>,
) -> Result<ConfigDiscovery, ConfigError> {
    if env_flag_is_truthy(&inputs.env, CABIN_NO_CONFIG_ENV) {
        return Ok(ConfigDiscovery {
            loaded_files: Vec::new(),
            disabled_by_env: true,
        });
    }

    if let Some(value) = (inputs.env)(CABIN_CONFIG_ENV)
        && !value.is_empty()
    {
        let path = PathBuf::from(value);
        let file = read_explicit(&path)?;
        return Ok(ConfigDiscovery {
            loaded_files: vec![file],
            disabled_by_env: false,
        });
    }

    let mut files = Vec::new();
    if let Some(path) = locate_user_config(&inputs.env, inputs.xdg_user_config_home.as_deref())
        && let Some(file) = read_optional(&path, ConfigSource::User)?
    {
        files.push(file);
    }
    if let Some(workspace) = &inputs.workspace {
        let path = workspace.root_dir.join(".cabin").join("config.toml");
        let source = if workspace.is_workspace_root {
            ConfigSource::Workspace
        } else {
            ConfigSource::Package
        };
        if let Some(file) = read_optional(&path, source)? {
            files.push(file);
        }
    }
    Ok(ConfigDiscovery {
        loaded_files: files,
        disabled_by_env: false,
    })
}

/// Promote a discovered config-file path into Cabin's UTF-8 model
/// path. Config files are discovered from the filesystem, where a
/// path is an `OsString`; Cabin assumes config paths are UTF-8, so
/// a non-UTF-8 path surfaces as a typed [`ConfigError`] (routed
/// through Cabin's diagnostics) rather than a silent lossy
/// conversion or a panic.
fn utf8_config_path(path: &Path) -> Result<Utf8PathBuf, ConfigError> {
    Utf8Path::from_path(path)
        .map(Utf8Path::to_path_buf)
        .ok_or_else(|| ConfigError::NonUtf8Path {
            path: path.to_path_buf(),
        })
}

fn read_explicit(path: &Path) -> Result<LoadedConfigFile, ConfigError> {
    let body = fs::read_to_string(path).map_err(|source| ConfigError::ExplicitConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = parse_config_str(&body).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(LoadedConfigFile {
        source: ConfigSource::Explicit,
        path: utf8_config_path(path)?,
        parsed,
    })
}

fn read_optional(
    path: &Path,
    source: ConfigSource,
) -> Result<Option<LoadedConfigFile>, ConfigError> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ConfigError::ConfigRead {
                path: path.to_path_buf(),
                source: err,
            });
        }
    };
    let parsed = parse_config_str(&body).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(LoadedConfigFile {
        source,
        path: utf8_config_path(path)?,
        parsed,
    }))
}

fn locate_user_config(env: &EnvLookup<'_>, xdg_user_config_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = env(CABIN_CONFIG_HOME_ENV)
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("config.toml"));
    }
    xdg_user_config_home.map(|p| p.join("config.toml"))
}

fn env_flag_is_truthy(env: &EnvLookup<'_>, var: &str) -> bool {
    let Some(value) = env(var) else {
        return false;
    };
    let s = value.to_string_lossy();
    matches!(s.trim(), "1" | "true" | "TRUE" | "True" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn env_with(items: &[(&'static str, &str)]) -> EnvLookup<'static> {
        let mut map: HashMap<&'static str, OsString> = HashMap::new();
        for (k, v) in items {
            map.insert(*k, OsString::from(*v));
        }
        Box::new(move |k| map.get(k).cloned())
    }

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(".cabin").join("config.toml");
        assert_fs::fixture::ChildPath::new(&path)
            .write_str(body)
            .unwrap();
        path
    }

    fn write_user_config(home: &Path, body: &str) -> PathBuf {
        let path = home.join(".config").join("cabin").join("config.toml");
        assert_fs::fixture::ChildPath::new(&path)
            .write_str(body)
            .unwrap();
        path
    }

    /// The user config home (`<HOME>/.config/cabin`) Cabin resolves
    /// on Linux when `HOME` is `home` and `XDG_CONFIG_HOME` is unset.
    /// Tests inject this so they exercise the fallback chain without
    /// mutating the process environment.
    fn home_xdg_config_home(home: &Path) -> PathBuf {
        home.join(".config").join("cabin")
    }

    #[test]
    fn no_config_env_short_circuits_discovery() {
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_NO_CONFIG", "1")]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert!(report.loaded_files.is_empty());
        assert!(report.disabled_by_env);
    }

    #[test]
    fn explicit_config_path_loads_a_single_file() {
        let dir = TempDir::new().unwrap();
        let config = dir.child("explicit.toml");
        config
            .write_str(
                r#"
            [build]
            profile = "release"
            "#,
            )
            .unwrap();
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_CONFIG", config.path().to_str().unwrap())]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert!(!report.disabled_by_env);
        assert_eq!(report.loaded_files.len(), 1);
        let loaded = &report.loaded_files[0];
        assert_eq!(loaded.source, ConfigSource::Explicit);
        assert_eq!(loaded.path, config.to_path_buf());
        assert_eq!(loaded.parsed.build.profile.as_deref(), Some("release"));
    }

    #[test]
    fn explicit_config_path_missing_yields_clear_error() {
        let dir = TempDir::new().unwrap();
        let missing = dir.child("missing.toml").to_path_buf();
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_CONFIG", missing.to_str().unwrap())]),
            xdg_user_config_home: None,
        };
        let err = discover_config_files(&inputs).unwrap_err();
        match err {
            ConfigError::ExplicitConfigRead {
                path: requested, ..
            } => assert_eq!(requested, missing),
            other => panic!("expected ExplicitConfigRead, got {other:?}"),
        }
    }

    #[test]
    fn user_config_is_loaded_via_cabin_config_home() {
        let home = TempDir::new().unwrap();
        let cabin_dir = home.child("cabin-conf");
        cabin_dir
            .child("config.toml")
            .write_str(
                r#"
            [registry]
            index-path = "registry"
            "#,
            )
            .unwrap();
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_CONFIG_HOME", cabin_dir.path().to_str().unwrap())]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
    }

    #[test]
    fn user_config_is_loaded_via_xdg_config_home() {
        // The injected `xdg_user_config_home` represents the resolved
        // user config home (`<base config dir>/cabin`) given a
        // non-empty absolute `XDG_CONFIG_HOME` on Linux.
        let xdg = TempDir::new().unwrap();
        xdg.child("cabin/config.toml")
            .write_str(
                r#"
            [paths]
            cache-dir = "user-cache"
            "#,
            )
            .unwrap();
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[]),
            xdg_user_config_home: Some(xdg.path().join("cabin")),
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
        assert_eq!(
            report.loaded_files[0]
                .parsed
                .paths
                .cache_dir
                .as_deref()
                .map(|p| p.as_str().to_owned()),
            Some("user-cache".to_owned())
        );
    }

    #[test]
    fn home_fallback_locates_dot_config_cabin() {
        // When `XDG_CONFIG_HOME` is unset, `xdg` falls back to
        // `$HOME/.config`; the injected path simulates that.
        let home = TempDir::new().unwrap();
        write_user_config(
            home.path(),
            r#"
            [build]
            profile = "release"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[]),
            xdg_user_config_home: Some(home_xdg_config_home(home.path())),
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
    }

    #[test]
    fn cabin_config_home_overrides_xdg_user_config_home() {
        // `CABIN_CONFIG_HOME` is a Cabin-specific override and
        // wins over the xdg-resolved path. It maps directly to
        // `<value>/config.toml` with no extra `cabin` component.
        let cabin = TempDir::new().unwrap();
        cabin
            .child("config.toml")
            .write_str(
                r#"
            [build]
            profile = "release"
            "#,
            )
            .unwrap();
        let stale_xdg = TempDir::new().unwrap();
        stale_xdg
            .child("cabin/config.toml")
            .write_str("[build]\nprofile = \"dev\"\n")
            .unwrap();
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_CONFIG_HOME", cabin.path().to_str().unwrap())]),
            xdg_user_config_home: Some(stale_xdg.path().join("cabin")),
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
        assert_eq!(
            report.loaded_files[0].parsed.build.profile.as_deref(),
            Some("release")
        );
    }

    #[test]
    fn workspace_layout_loads_workspace_label_when_root_declares_workspace() {
        let workspace = TempDir::new().unwrap();
        write_config(
            workspace.path(),
            r#"
            [build]
            profile = "release"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: workspace.path(),
                is_workspace_root: true,
            }),
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::Workspace);
    }

    #[test]
    fn workspace_layout_loads_project_label_when_root_is_single_package() {
        let package = TempDir::new().unwrap();
        write_config(
            package.path(),
            r#"
            [build]
            profile = "release"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: package.path(),
                is_workspace_root: false,
            }),
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::Package);
    }

    #[test]
    fn user_then_workspace_order_is_deterministic() {
        // User config sets profile = release; workspace config
        // sets cache-dir. The merger relies on the user config
        // arriving *first* so workspace overrides win on overlap.
        let home = TempDir::new().unwrap();
        write_user_config(
            home.path(),
            r#"
            [build]
            profile = "release"
            "#,
        );
        let workspace = TempDir::new().unwrap();
        write_config(
            workspace.path(),
            r#"
            [paths]
            cache-dir = "ws-cache"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: workspace.path(),
                is_workspace_root: true,
            }),
            env: env_with(&[]),
            xdg_user_config_home: Some(home_xdg_config_home(home.path())),
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 2);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
        assert_eq!(report.loaded_files[1].source, ConfigSource::Workspace);
    }

    #[test]
    fn missing_user_or_workspace_files_are_not_an_error() {
        // No env, no workspace, no files anywhere — discovery
        // returns an empty list rather than an error.
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let report = discover_config_files(&inputs).unwrap();
        assert!(report.loaded_files.is_empty());
        assert!(!report.disabled_by_env);
    }

    #[test]
    fn parse_failure_includes_path_in_error() {
        let workspace = TempDir::new().unwrap();
        write_config(workspace.path(), "this is not toml = == ===");
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: workspace.path(),
                is_workspace_root: true,
            }),
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let err = discover_config_files(&inputs).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(".cabin")
                && message.contains("config.toml")
                && message.contains("invalid TOML"),
            "expected parse error to include the file path and reason, got: {message}"
        );
    }

    #[test]
    fn empty_explicit_value_falls_back_to_normal_discovery() {
        let home = TempDir::new().unwrap();
        write_user_config(
            home.path(),
            r#"
            [build]
            profile = "release"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: None,
            env: env_with(&[("CABIN_CONFIG", "")]),
            xdg_user_config_home: Some(home_xdg_config_home(home.path())),
        };
        let report = discover_config_files(&inputs).unwrap();
        assert_eq!(report.loaded_files.len(), 1);
        assert_eq!(report.loaded_files[0].source, ConfigSource::User);
    }

    #[test]
    fn target_conditioned_table_in_workspace_config_yields_clear_error() {
        let workspace = TempDir::new().unwrap();
        write_config(
            workspace.path(),
            r#"
            [target.'cfg(os = "linux")'.toolchain]
            cxx = "clang++"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: workspace.path(),
                is_workspace_root: true,
            }),
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let err = discover_config_files(&inputs).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("target-conditioned config tables are not supported"),
            "expected target-conditioned rejection, got: {message}"
        );
    }

    #[test]
    fn unsupported_auth_table_in_workspace_config_yields_clear_error() {
        let workspace = TempDir::new().unwrap();
        write_config(
            workspace.path(),
            r#"
            [auth]
            token = "secret"
            "#,
        );
        let inputs = ConfigDiscoveryInputs {
            workspace: Some(WorkspaceLayout {
                root_dir: workspace.path(),
                is_workspace_root: true,
            }),
            env: env_with(&[]),
            xdg_user_config_home: None,
        };
        let err = discover_config_files(&inputs).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("does not handle credentials"),
            "expected auth rejection, got: {message}"
        );
    }

    /// Sanity check that `ConfigParseError` round-trips through
    /// `Display`. Tests below match substrings from these strings,
    /// so a misformatted variant would silently break test
    /// coverage; this asserts the contract holds.
    #[test]
    fn parse_error_display_round_trips() {
        let err = crate::error::ConfigParseError::EmptyProfile;
        assert!(err.to_string().contains("non-empty profile name"));
    }
}
