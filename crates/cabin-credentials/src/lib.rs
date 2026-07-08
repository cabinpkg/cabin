//! Registry credential storage for Cabin's experimental
//! remote-registry client (`-Z remote-registry`).
//!
//! Tokens live in `credentials.toml` inside the user config home -
//! the same directory resolution as the user-level `config.toml`
//! (`CABIN_CONFIG_HOME` verbatim, else the platform user config home
//! with the `cabin` suffix via `etcetera`).  Credentials are
//! deliberately *not* part of `cabin-config`: the config parser
//! rejects credential-shaped tables so a secret can never ride along
//! in a published archive.
//!
//! ```toml
//! [registries."https://dev-registry.cabinpkg.com"]
//! token = "cabin_..."
//! ```
//!
//! Keys are normalized index origins (scheme + host + port, no path,
//! no trailing slash).  The `CABIN_REGISTRY_TOKEN` environment
//! variable, when set and non-empty, wins over the file for every
//! registry an invocation touches.
//!
//! Token values must never appear in logs, error messages, or debug
//! output: [`Token`]'s `Debug` / `Display` impls redact, and every
//! error produced here avoids echoing token bytes.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use etcetera::{BaseStrategy, choose_base_strategy};
use serde::Deserialize;
use thiserror::Error;

/// File name of the credential store inside the user config home.
pub const CREDENTIALS_FILENAME: &str = "credentials.toml";

/// Required prefix of every Cabin registry token.
const TOKEN_PREFIX: &str = "cabin_";

/// Bounds on the token payload (the part after `cabin_`).  Generous
/// enough for any realistic issuance scheme, tight enough to catch
/// pasting the wrong thing.
const TOKEN_PAYLOAD_LEN: std::ops::RangeInclusive<usize> = 8..=512;

/// A registry bearer token.  The wrapped value is deliberately
/// unreachable except through [`Token::expose`], and both `Debug`
/// and `Display` redact so a token cannot leak through logging or
/// error formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Validate and wrap a raw token: the `cabin_` prefix followed
    /// by 8 to 512 ASCII alphanumeric (base62) characters.  The
    /// character restriction doubles as header hygiene - a value
    /// that passes can never smuggle CR/LF or other control bytes
    /// into an `Authorization` header.
    ///
    /// # Errors
    /// Returns [`CredentialsError::InvalidToken`] naming what is
    /// wrong; the raw value is never echoed.
    pub fn parse(raw: &str) -> Result<Self, CredentialsError> {
        let Some(payload) = raw.strip_prefix(TOKEN_PREFIX) else {
            return Err(CredentialsError::InvalidToken {
                reason: "expected the `cabin_` prefix",
            });
        };
        if !TOKEN_PAYLOAD_LEN.contains(&payload.len()) {
            return Err(CredentialsError::InvalidToken {
                reason: "unexpected length",
            });
        }
        if !payload.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(CredentialsError::InvalidToken {
                reason: "expected only ASCII letters and digits after the prefix",
            });
        }
        Ok(Self(raw.to_owned()))
    }

    /// The raw token value.  The name is deliberately loud: call
    /// sites should be auditable for where the secret leaves the
    /// newtype (writing the file, building the `Authorization`
    /// header).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Normalize an index URL to its origin: lower-cased scheme + host
/// plus the port when it is not the scheme default - no path, no
/// trailing slash.  This is the key shape `credentials.toml` uses
/// and the granularity a token is scoped to.
///
/// # Errors
/// Returns [`CredentialsError::InvalidOrigin`] when the URL is
/// malformed, is not `http(s)`, has no host, or carries `userinfo`
/// credentials (which are never echoed back).
pub fn normalize_origin(url: &str) -> Result<String, CredentialsError> {
    let parsed = url::Url::parse(url).map_err(|err| CredentialsError::InvalidOrigin {
        url: redact_userinfo(url),
        message: err.to_string(),
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CredentialsError::InvalidOrigin {
                url: redact_userinfo(url),
                message: format!("unsupported URL scheme {other:?}"),
            });
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CredentialsError::InvalidOrigin {
            url: redact_userinfo(url),
            message: "URL must not contain credentials (userinfo)".to_owned(),
        });
    }
    if parsed.host_str().is_none() {
        return Err(CredentialsError::InvalidOrigin {
            url: redact_userinfo(url),
            message: "URL has no host".to_owned(),
        });
    }
    Ok(parsed.origin().ascii_serialization())
}

/// Whether `url`'s host is loopback: an IPv4 address in
/// `127.0.0.0/8`, the IPv6 loopback `::1`, or the literal
/// `localhost` name.  These are the only hosts a token may reach
/// over plain `http`; the rule is shared by `cabin login` (which
/// refuses to store a token that could never be attached) and the
/// sparse HTTP client's per-request cleartext check, so the two
/// cannot drift.  Unparsable URLs are not loopback.
#[must_use]
pub fn url_is_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Replace any `user:password@` authority prefix in a raw URL with
/// `***@` so origin errors never leak credentials.  The authority
/// starts after `://` for absolute URLs, after a leading `//` for
/// scheme-relative ones, and at the start of the string otherwise -
/// over-redacting a scheme-less paste is preferable to echoing one
/// that carried a credential.
fn redact_userinfo(raw: &str) -> String {
    let authority_start = if raw.starts_with("//") {
        2
    } else if let Some(pos) = raw.find("://") {
        pos + 3
    } else {
        0
    };
    let authority_end = raw[authority_start..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |pos| authority_start + pos);
    match raw[authority_start..authority_end].rfind('@') {
        Some(at) => format!(
            "{}***@{}",
            &raw[..authority_start],
            &raw[authority_start + at + 1..]
        ),
        None => raw.to_owned(),
    }
}

/// In-memory view of `credentials.toml`: normalized origin -> token.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    registries: BTreeMap<String, Token>,
}

impl Credentials {
    /// Token stored for `origin` (already normalized), if any.
    #[must_use]
    pub fn token_for(&self, origin: &str) -> Option<&Token> {
        self.registries.get(origin)
    }

    /// Store `token` for `origin` (already normalized), replacing
    /// any previous entry.
    pub fn set_token(&mut self, origin: String, token: Token) {
        self.registries.insert(origin, token);
    }

    /// Remove the entry for `origin`.  Returns whether one existed.
    pub fn remove_token(&mut self, origin: &str) -> bool {
        self.registries.remove(origin).is_some()
    }
}

/// Result of loading the credential store: the parsed credentials
/// plus an optional permissions warning the caller should surface
/// once per invocation (this crate never prints).
#[derive(Debug)]
pub struct LoadedCredentials {
    pub credentials: Credentials,
    /// Set on Unix when an existing file is group- or
    /// world-readable.
    pub permissions_warning: Option<String>,
}

/// Result of a token lookup for one origin: the winning token (env
/// override first, then the file) plus any permissions warning from
/// reading the file.
#[derive(Debug)]
pub struct TokenLookup {
    pub token: Option<Token>,
    pub permissions_warning: Option<String>,
}

/// Handle to the on-disk credential store.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// Resolve the store location from the process environment:
    /// `$CABIN_CONFIG_HOME/credentials.toml` when the override is
    /// set and non-empty, else `<user config home>/cabin/credentials.toml`
    /// via `etcetera` - exactly the user-level `config.toml`
    /// resolution.
    ///
    /// # Errors
    /// Returns [`CredentialsError::NoConfigHome`] when no user
    /// config home can be determined.
    pub fn from_env() -> Result<Self, CredentialsError> {
        if let Some(dir) = std::env::var_os(cabin_env::CABIN_CONFIG_HOME)
            && !dir.is_empty()
        {
            return Ok(Self::at(PathBuf::from(dir).join(CREDENTIALS_FILENAME)));
        }
        let home = choose_base_strategy()
            .ok()
            .map(|dirs| dirs.config_dir().join("cabin"))
            .ok_or(CredentialsError::NoConfigHome)?;
        Ok(Self::at(home.join(CREDENTIALS_FILENAME)))
    }

    /// Store backed by an explicit file path.  Used by tests and by
    /// callers that already resolved the config home.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path of the backing `credentials.toml`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read and parse the store.  A missing file is an empty store,
    /// not an error.  On Unix, an existing file that is group- or
    /// world-readable produces a `permissions_warning` for the
    /// caller to surface.
    ///
    /// # Errors
    /// Returns [`CredentialsError::Io`] when the file exists but
    /// cannot be read, [`CredentialsError::Parse`] when it is not
    /// valid credentials TOML (unknown fields included),
    /// [`CredentialsError::InvalidToken`] when a stored token fails
    /// validation, and [`CredentialsError::NonNormalizedKey`] when a
    /// registry key is not a normalized origin.
    pub fn load(&self) -> Result<LoadedCredentials, CredentialsError> {
        let body = match std::fs::read_to_string(&self.path) {
            Ok(body) => body,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedCredentials {
                    credentials: Credentials::default(),
                    permissions_warning: None,
                });
            }
            Err(source) => {
                return Err(CredentialsError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        // Surface only the parser's message, never a snippet of the
        // file: a rendered snippet could echo a stored token.
        let raw: RawCredentials = toml::from_str(&body).map_err(|err| CredentialsError::Parse {
            path: self.path.clone(),
            message: err.message().to_owned(),
        })?;
        let mut credentials = Credentials::default();
        for (key, entry) in raw.registries {
            if normalize_origin(&key)? != key {
                return Err(CredentialsError::NonNormalizedKey { key });
            }
            credentials.set_token(key, Token::parse(&entry.token)?);
        }
        Ok(LoadedCredentials {
            credentials,
            permissions_warning: self.permissions_warning(),
        })
    }

    #[cfg(unix)]
    fn permissions_warning(&self) -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&self.path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "credentials file {} is readable by other users (mode {:03o}); run `chmod 600 {}`",
                self.path.display(),
                mode & 0o777,
                self.path.display()
            ));
        }
        None
    }

    #[cfg(not(unix))]
    fn permissions_warning(&self) -> Option<String> {
        None
    }

    /// Serialize and atomically replace the store: the bytes are
    /// staged in a sibling temp file and renamed into place, like
    /// `cabin-registry-file`'s writers.  On Unix the
    /// file is (re)created with mode `0600`, regardless of any
    /// looser mode a previous file had.  The parent directory is
    /// created when missing.
    ///
    /// # Errors
    /// Returns [`CredentialsError::Io`] when creating the parent
    /// directory or writing the file fails.
    pub fn save(&self, credentials: &Credentials) -> Result<(), CredentialsError> {
        let io_err = |source| CredentialsError::Io {
            path: self.path.clone(),
            source,
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        let raw = RawCredentialsOut {
            registries: credentials
                .registries
                .iter()
                .map(|(origin, token)| {
                    (
                        origin.as_str(),
                        RawRegistryCredentialOut {
                            token: token.expose(),
                        },
                    )
                })
                .collect(),
        };
        // `BTreeMap` iteration keeps the origins sorted, so the file
        // is byte-deterministic for a given credential set.
        let body = toml::to_string(&raw).map_err(|err| CredentialsError::Parse {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        #[cfg(unix)]
        let options = {
            use atomic_write_file::unix::OpenOptionsExt as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut options = atomic_write_file::OpenOptions::new();
            options.preserve_mode(false);
            options.mode(0o600);
            options
        };
        #[cfg(not(unix))]
        let options = atomic_write_file::OpenOptions::new();
        let mut file = options.open(&self.path).map_err(io_err)?;
        file.write_all(body.as_bytes()).map_err(io_err)?;
        file.commit().map_err(io_err)?;
        Ok(())
    }

    /// Resolve the token to use for `origin`: the
    /// `CABIN_REGISTRY_TOKEN` environment override when set and
    /// non-empty, else the file entry for `origin`.
    ///
    /// # Errors
    /// Propagates [`CredentialStore::load`] errors and rejects a
    /// malformed environment override with
    /// [`CredentialsError::InvalidToken`] rather than sending
    /// garbage bytes in an `Authorization` header.
    pub fn token_for_origin(&self, origin: &str) -> Result<TokenLookup, CredentialsError> {
        self.token_for_origin_with_env(
            std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).as_deref(),
            origin,
        )
    }

    /// [`CredentialStore::token_for_origin`] with the environment
    /// value injected, so tests can drive the precedence without
    /// mutating the process environment.
    ///
    /// # Errors
    /// Same as [`CredentialStore::token_for_origin`].
    pub fn token_for_origin_with_env(
        &self,
        env_value: Option<&OsStr>,
        origin: &str,
    ) -> Result<TokenLookup, CredentialsError> {
        if let Some(token) = token_from_env_value(env_value)? {
            return Ok(TokenLookup {
                token: Some(token),
                permissions_warning: None,
            });
        }
        let loaded = self.load()?;
        Ok(TokenLookup {
            token: loaded.credentials.token_for(origin).cloned(),
            permissions_warning: loaded.permissions_warning,
        })
    }
}

/// Parse a raw `CABIN_REGISTRY_TOKEN` environment value: unset and
/// empty are "no override"; anything else must be a valid token.
fn token_from_env_value(env_value: Option<&OsStr>) -> Result<Option<Token>, CredentialsError> {
    let Some(raw) = env_value.filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let raw = raw.to_str().ok_or(CredentialsError::InvalidToken {
        reason: "CABIN_REGISTRY_TOKEN is not valid UTF-8",
    })?;
    Token::parse(raw).map(Some)
}

/// Read-path token lookup for one origin: the `CABIN_REGISTRY_TOKEN`
/// environment override when set and non-empty, else the
/// `credentials.toml` entry.  The override is consulted *before* the
/// store is even located, so it keeps working in home-less
/// environments (CI containers) where no user config home can be
/// resolved; there, a missing config home simply means "no stored
/// credential" rather than an error, so unauthenticated flows never
/// fail either.
///
/// # Errors
/// Rejects a malformed environment override with
/// [`CredentialsError::InvalidToken`] rather than sending garbage
/// bytes in an `Authorization` header, and propagates
/// [`CredentialStore::load`] errors for an unreadable or invalid
/// credentials file.
pub fn lookup_token(origin: &str) -> Result<TokenLookup, CredentialsError> {
    lookup_token_with_env(
        std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).as_deref(),
        origin,
    )
}

/// [`lookup_token`] with the environment value injected for tests.
///
/// # Errors
/// Same as [`lookup_token`].
pub fn lookup_token_with_env(
    env_value: Option<&OsStr>,
    origin: &str,
) -> Result<TokenLookup, CredentialsError> {
    if let Some(token) = token_from_env_value(env_value)? {
        return Ok(TokenLookup {
            token: Some(token),
            permissions_warning: None,
        });
    }
    match CredentialStore::from_env() {
        Ok(store) => store.token_for_origin_with_env(None, origin),
        Err(CredentialsError::NoConfigHome) => Ok(TokenLookup {
            token: None,
            permissions_warning: None,
        }),
        Err(err) => Err(err),
    }
}

/// Raw serde shape of `credentials.toml`.  Private so token strings
/// never travel outside this crate un-redacted; no `Debug` derive
/// for the same reason.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCredentials {
    #[serde(default)]
    registries: BTreeMap<String, RawRegistryCredential>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryCredential {
    token: String,
}

/// Write-side mirror of [`RawCredentials`], borrowing so the token
/// is never copied around more than the serializer requires.
#[derive(serde::Serialize)]
struct RawCredentialsOut<'a> {
    registries: BTreeMap<&'a str, RawRegistryCredentialOut<'a>>,
}

#[derive(serde::Serialize)]
struct RawRegistryCredentialOut<'a> {
    token: &'a str,
}

/// Errors produced by the credential store.  No variant ever embeds
/// token bytes.
#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("failed to access credentials file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("invalid credentials file {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("invalid registry token: {reason}")]
    InvalidToken { reason: &'static str },

    #[error("invalid registry index URL `{url}`: {message}")]
    InvalidOrigin { url: String, message: String },

    #[error(
        "credentials key `{key}` is not a normalized origin (scheme + host + port, no path, no \
         trailing slash)"
    )]
    NonNormalizedKey { key: String },

    #[error("cannot determine the user config home for credentials.toml")]
    NoConfigHome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;

    const SECRET: &str = "cabin_abcDEF12345";

    fn token() -> Token {
        Token::parse(SECRET).unwrap()
    }

    #[test]
    fn token_parse_accepts_base62_payloads() {
        for raw in ["cabin_12345678", "cabin_abcDEF12345", SECRET] {
            assert_eq!(Token::parse(raw).unwrap().expose(), raw);
        }
    }

    #[test]
    fn token_parse_rejects_bad_prefix_length_and_charset() {
        for raw in [
            "",
            "cabin_",
            "cabin_short",                         // 5-char payload
            &format!("cabin_{}", "a".repeat(513)), // over-long payload
            "notcabin_12345678",
            "cabin_with-dash1",
            "cabin_with space",
            "cabin_evil\r\nHeader: x1",
        ] {
            let err = Token::parse(raw).unwrap_err();
            assert!(
                matches!(err, CredentialsError::InvalidToken { .. }),
                "{raw:?} should be rejected, got {err:?}"
            );
        }
    }

    /// The redaction contract: neither `Debug` nor `Display` output
    /// contains any token bytes.
    #[test]
    fn token_debug_and_display_redact() {
        let token = token();
        let debug = format!("{token:?}");
        let display = format!("{token}");
        for rendered in [&debug, &display] {
            assert!(
                !rendered.contains("abcDEF12345") && !rendered.contains(SECRET),
                "token bytes leaked: {rendered:?}"
            );
        }
        // The containers that can hold tokens redact through the
        // newtype too.
        let mut credentials = Credentials::default();
        credentials.set_token("https://example.com".to_owned(), token);
        let rendered = format!("{credentials:?}");
        assert!(
            !rendered.contains("abcDEF12345"),
            "token bytes leaked through Credentials: {rendered:?}"
        );
    }

    #[test]
    fn normalize_origin_strips_path_slash_and_default_port() {
        for (input, expected) in [
            (
                "https://dev-registry.cabinpkg.com",
                "https://dev-registry.cabinpkg.com",
            ),
            (
                "https://dev-registry.cabinpkg.com/",
                "https://dev-registry.cabinpkg.com",
            ),
            (
                "https://Dev-Registry.CabinPkg.com/index/path?q=1#frag",
                "https://dev-registry.cabinpkg.com",
            ),
            ("https://example.com:443/index", "https://example.com"),
            ("http://example.com:80/index", "http://example.com"),
            ("http://example.com:8080/index", "http://example.com:8080"),
            ("http://127.0.0.1:3000/reg/", "http://127.0.0.1:3000"),
        ] {
            assert_eq!(normalize_origin(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn normalize_origin_rejects_non_http_hostless_and_userinfo() {
        for input in ["file:///tmp/reg", "not a url", "data:text/plain,x"] {
            assert!(normalize_origin(input).is_err(), "{input}");
        }
        let err = normalize_origin("https://user:pw@example.com/index").unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted: {message}"
        );
        assert!(message.contains("userinfo"), "{message}");
    }

    /// The redaction also covers unparsable inputs whose authority
    /// carries a credential: scheme-relative and scheme-less pastes
    /// must not echo the `user:pw` back.
    #[test]
    fn normalize_origin_redacts_userinfo_in_unparsable_inputs() {
        for input in [
            "//user:pw@registry.example.com",
            "user:pw@registry.example.com/index",
            "htp://user:pw@registry.example.com",
        ] {
            let err = normalize_origin(input).unwrap_err();
            let message = err.to_string();
            assert!(
                !message.contains("user:pw"),
                "credentials must be redacted for {input:?}: {message}"
            );
        }
    }

    #[test]
    fn url_is_loopback_recognizes_only_loopback_hosts() {
        for url in [
            "http://127.0.0.1:8080/registry",
            "http://127.5.6.7/",
            "http://[::1]:3000/",
            "http://localhost:8080/",
            "http://LOCALHOST/",
        ] {
            assert!(url_is_loopback(url), "{url}");
        }
        for url in [
            "http://registry.example.com/",
            "http://10.0.0.1/",
            "http://[::2]/",
            "http://localhost.example.com/",
            "not a url",
        ] {
            assert!(!url_is_loopback(url), "{url}");
        }
    }

    #[test]
    fn round_trip_set_save_load() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_token("https://dev-registry.cabinpkg.com".to_owned(), token());
        store.save(&credentials).unwrap();

        let body = std::fs::read_to_string(store.path()).unwrap();
        assert_eq!(
            body,
            format!("[registries.\"https://dev-registry.cabinpkg.com\"]\ntoken = \"{SECRET}\"\n")
        );

        let loaded = store.load().unwrap();
        assert_eq!(
            loaded
                .credentials
                .token_for("https://dev-registry.cabinpkg.com")
                .unwrap()
                .expose(),
            SECRET
        );
        assert!(
            loaded
                .credentials
                .token_for("https://other.example")
                .is_none()
        );
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let loaded = store.load().unwrap();
        assert!(
            loaded
                .credentials
                .token_for("https://example.com")
                .is_none()
        );
        assert!(loaded.permissions_warning.is_none());
    }

    #[test]
    fn remove_token_reports_whether_an_entry_existed() {
        let mut credentials = Credentials::default();
        credentials.set_token("https://example.com".to_owned(), token());
        assert!(credentials.remove_token("https://example.com"));
        assert!(!credentials.remove_token("https://example.com"));
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_the_file_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("nested").join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_token("https://example.com".to_owned(), token());
        store.save(&credentials).unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:03o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn save_tightens_a_loose_existing_file_back_to_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        std::fs::write(store.path(), "").unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        store.save(&Credentials::default()).unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:03o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn load_warns_once_about_group_or_world_readable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        store.save(&Credentials::default()).unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let warning = store.load().unwrap().permissions_warning.unwrap();
        assert!(warning.contains("chmod 600"), "{warning}");
        assert!(warning.contains("644"), "{warning}");

        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.load().unwrap().permissions_warning.is_none());
    }

    #[test]
    fn parse_rejects_unknown_fields_without_echoing_values() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        for body in [
            // Unknown top-level table.
            format!("[auth]\ntoken = \"{SECRET}\"\n"),
            // Unknown field inside a registry entry.
            format!(
                "[registries.\"https://example.com\"]\ntoken = \"{SECRET}\"\nscope = \"publish\"\n"
            ),
            // Typo'd `token` key.
            format!("[registries.\"https://example.com\"]\ntokn = \"{SECRET}\"\n"),
        ] {
            std::fs::write(store.path(), &body).unwrap();
            let err = store.load().unwrap_err();
            let message = err.to_string();
            assert!(
                matches!(err, CredentialsError::Parse { .. }),
                "expected Parse error for {body:?}, got {err:?}"
            );
            assert!(
                !message.contains(SECRET),
                "token bytes leaked into parse error: {message}"
            );
        }
    }

    #[test]
    fn load_rejects_non_normalized_keys() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        for key in [
            "https://example.com/",
            "https://example.com/index",
            "https://Example.com",
            "https://example.com:443",
        ] {
            std::fs::write(
                store.path(),
                format!("[registries.\"{key}\"]\ntoken = \"{SECRET}\"\n"),
            )
            .unwrap();
            let err = store.load().unwrap_err();
            assert!(
                matches!(err, CredentialsError::NonNormalizedKey { .. }),
                "{key:?} should be rejected as non-normalized, got {err:?}"
            );
        }
    }

    #[test]
    fn env_override_wins_over_the_file_for_every_origin() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_token("https://example.com".to_owned(), token());
        store.save(&credentials).unwrap();

        let env = OsStr::new("cabin_envToken12345");
        // Even an origin with a stored file entry sees the override...
        let lookup = store
            .token_for_origin_with_env(Some(env), "https://example.com")
            .unwrap();
        assert_eq!(lookup.token.unwrap().expose(), "cabin_envToken12345");
        // ...and so does an origin the file knows nothing about.
        let lookup = store
            .token_for_origin_with_env(Some(env), "https://other.example")
            .unwrap();
        assert_eq!(lookup.token.unwrap().expose(), "cabin_envToken12345");
    }

    #[test]
    fn empty_or_absent_env_falls_back_to_the_file() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_token("https://example.com".to_owned(), token());
        store.save(&credentials).unwrap();

        for env in [None, Some(OsStr::new(""))] {
            let lookup = store
                .token_for_origin_with_env(env, "https://example.com")
                .unwrap();
            assert_eq!(lookup.token.unwrap().expose(), SECRET);
            let lookup = store
                .token_for_origin_with_env(env, "https://other.example")
                .unwrap();
            assert!(lookup.token.is_none());
        }
    }

    /// The env override is honored before the store is even
    /// located, so it works in home-less environments where no
    /// user config home resolves.
    #[test]
    fn lookup_token_env_override_applies_before_the_store_is_located() {
        let lookup =
            lookup_token_with_env(Some(OsStr::new("cabin_envToken12345")), "https://x.example")
                .unwrap();
        assert_eq!(lookup.token.unwrap().expose(), "cabin_envToken12345");
    }

    #[test]
    fn malformed_env_override_is_rejected_not_sent() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let err = store
            .token_for_origin_with_env(Some(OsStr::new("not-a-token")), "https://example.com")
            .unwrap_err();
        assert!(matches!(err, CredentialsError::InvalidToken { .. }));
    }

    #[test]
    fn saved_file_is_deterministic_and_sorted() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_token("https://zeta.example".to_owned(), token());
        credentials.set_token("https://alpha.example".to_owned(), token());
        store.save(&credentials).unwrap();
        let body = std::fs::read_to_string(store.path()).unwrap();
        let alpha = body.find("alpha.example").unwrap();
        let zeta = body.find("zeta.example").unwrap();
        assert!(
            alpha < zeta,
            "origins must be written in sorted order:\n{body}"
        );
        // Round-trips through the parser.
        store.load().unwrap();
    }
}
