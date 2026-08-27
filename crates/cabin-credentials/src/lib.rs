//! Registry credential storage for Cabin's remote-registry client.
//!
//! The stored credential is a login session minted by `cabin login`
//! (`docs/remote-registry.md`, "Login sessions"): a short-lived
//! `cabin_ses_` token plus the mint's `expires_at` and the API origin
//! it was minted for.  Sessions live in the platform keychain (macOS
//! Keychain, Windows Credential Manager, Linux secret-service) when
//! one is available, falling back to a `credentials.toml` file inside
//! the user config home - the same directory resolution as the
//! user-level `config.toml` (`CABIN_CONFIG_HOME` verbatim, else the
//! platform user config home with the `cabin` suffix via `etcetera`).
//! When `CABIN_CONFIG_HOME` is set the keychain is bypassed entirely:
//! the override marks a hermetic environment (tests, CI sandboxes),
//! and a platform keychain would leak sessions across them.
//! Credentials are deliberately *not* part of `cabin-config`: the
//! config parser rejects credential-shaped tables so a secret can
//! never ride along in a published archive.
//!
//! ```toml
//! [registries."https://registry.cabinpkg.com"]
//! token = "cabin_ses_..."
//! expires-at = "2026-01-01T00:00:00.000Z"
//! api-url = "https://cabinpkg.com"
//! ```
//!
//! Keys are normalized index origins (scheme + host + port, no path,
//! no trailing slash).  The `CABIN_REGISTRY_TOKEN` environment
//! variable, when set and non-empty, wins over stored sessions - but
//! only for the origins its caller declares eligible (see
//! [`lookup_token`]), because the variable carries no origin key of
//! its own.
//!
//! Token values must never appear in logs, error messages, or debug
//! output: [`Token`]'s `Debug` / `Display` impls redact, and every
//! error produced here avoids echoing token bytes.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// Payload marker of a trusted-publishing token (`cabin_tp_...`),
/// whose minted secret is unpadded base64url rather than base62.
const TRUSTPUB_MARKER: &str = "tp_";

/// Payload marker of a login-session token (`cabin_ses_...`), whose
/// minted secret is unpadded base64url like a trusted-publishing one.
const SESSION_MARKER: &str = "ses_";

/// A registry bearer token.  The wrapped value is deliberately
/// unreachable except through [`Token::expose`], and both `Debug`
/// and `Display` redact so a token cannot leak through logging or
/// error formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Validate and wrap a raw token: the `cabin_` prefix followed
    /// by 8 to 512 ASCII alphanumeric (base62) characters, except
    /// that a minted token - a trusted-publishing exchange
    /// (`cabin_tp_`) or a login session (`cabin_ses_`) - carries an
    /// unpadded-base64url payload, which adds `-` and `_`.  Either
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
        if let Some(minted) = payload
            .strip_prefix(TRUSTPUB_MARKER)
            .or_else(|| payload.strip_prefix(SESSION_MARKER))
        {
            if !minted
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(CredentialsError::InvalidToken {
                    reason: "expected only base64url characters after the \
                             `cabin_tp_` or `cabin_ses_` prefix",
                });
            }
        } else if !payload.bytes().all(|b| b.is_ascii_alphanumeric()) {
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

/// A stored login session: the minted token, the mint response's
/// `expires_at` verbatim, and the API origin the token was minted
/// for - the origin `cabin logout` revokes against, pinned at mint
/// time so a later `config.json` change cannot re-route the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub token: Token,
    /// RFC 3339 expiry, exactly as the registry's mint answered it.
    pub expires_at: String,
    /// The registry's `api` origin at mint time.
    pub api_url: String,
}

impl Session {
    /// Whether the session's `expires_at` has passed.  The parse is
    /// UTC-only RFC 3339 (`Z` / `+00:00`) - the same shapes the mint
    /// accepts and stores - and the check is advisory UX (the
    /// registry enforces expiry server-side), so an `expires_at` this
    /// client cannot parse reads as not expired rather than locking
    /// the session out.
    #[must_use]
    pub fn expired_at(&self, now: SystemTime) -> bool {
        humantime::parse_rfc3339(&self.expires_at).is_ok_and(|expiry| expiry <= now)
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

/// In-memory view of `credentials.toml`: normalized origin -> session.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    registries: BTreeMap<String, Session>,
}

impl Credentials {
    /// Session stored for `origin` (already normalized), if any.
    #[must_use]
    pub fn session_for(&self, origin: &str) -> Option<&Session> {
        self.registries.get(origin)
    }

    /// Store `session` for `origin` (already normalized), replacing
    /// any previous entry.
    pub fn set_session(&mut self, origin: String, session: Session) {
        self.registries.insert(origin, session);
    }

    /// Remove the entry for `origin`.  Returns whether one existed.
    pub fn remove_session(&mut self, origin: &str) -> bool {
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
/// override first, then the stored session) plus any warning from
/// reading the fallback file (loose permissions, or an unreadable
/// file), for the caller to surface once per invocation.
#[derive(Debug)]
pub struct TokenLookup {
    pub token: Option<Token>,
    /// Set (to the stored `expires_at`) when a session was stored for
    /// the origin but has expired; the token is withheld so callers
    /// can say "expired at ..., run `cabin login`" instead of sending
    /// a credential the registry will refuse.
    pub expired_at: Option<String>,
    pub warning: Option<String>,
}

/// One credential backend: the platform keychain, the fallback file,
/// or a test double.  Implementations signal a backend that cannot
/// serve this process at all (no keychain daemon, no D-Bus session)
/// with [`CredentialsError::KeychainUnavailable`], which
/// [`SessionStorage`] treats as "try the next backend".
pub trait SessionStore {
    /// The stored session for `origin`, plus any warning to surface.
    ///
    /// # Errors
    /// Backend-specific read failures.
    fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError>;

    /// Store `session` for `origin`, replacing any previous entry.
    ///
    /// # Errors
    /// Backend-specific write failures.
    fn store(&self, origin: &str, session: &Session) -> Result<(), CredentialsError>;

    /// Remove the entry for `origin`.  Returns whether one existed.
    ///
    /// # Errors
    /// Backend-specific write failures.
    fn remove(&self, origin: &str) -> Result<bool, CredentialsError>;
}

/// Result of one backend's [`SessionStore::load`].
#[derive(Debug, Default)]
pub struct SessionLoad {
    pub session: Option<Session>,
    /// Set by the file backend for a file that is group- or
    /// world-readable on Unix, or one this client cannot read.
    pub warning: Option<String>,
}

/// Handle to the on-disk credential store - the fallback backend.
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
    /// valid credentials TOML (unknown fields included, and the
    /// pre-session `token`-only shape, whose long-lived keys no
    /// registry accepts any more), [`CredentialsError::InvalidToken`]
    /// when a stored token fails validation, and
    /// [`CredentialsError::NonNormalizedKey`] when a registry key is
    /// not a normalized origin.
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
            credentials.set_session(
                key,
                Session {
                    token: Token::parse(&entry.token)?,
                    expires_at: entry.expires_at,
                    api_url: entry.api_url,
                },
            );
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
                .map(|(origin, session)| {
                    (
                        origin.as_str(),
                        RawRegistryCredentialOut {
                            token: session.token.expose(),
                            expires_at: &session.expires_at,
                            api_url: &session.api_url,
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
}

/// Whether a [`CredentialStore::load`] failure means the file holds
/// no session this client can use - the pre-session `token`-only
/// shape (whose long-lived keys no registry accepts any more), an
/// unparsable or hand-edited file, a malformed token or key.  As a
/// `SessionStore` such a file mirrors the keychain doctrine: it reads
/// as absent (with a warning naming the state), the next store
/// replaces it wholesale, and a removal is a no-op - so the `cabin
/// login` every such warning recommends actually works.  I/O
/// failures stay errors.
fn file_unreadable(err: &CredentialsError) -> bool {
    match err {
        CredentialsError::Parse { .. }
        | CredentialsError::InvalidToken { .. }
        | CredentialsError::NonNormalizedKey { .. } => true,
        // Invalid UTF-8 is content damage like unparsable TOML, not
        // an environment problem: replaceable.  Other I/O failures
        // (permissions, disk) stay errors - a replacement write would
        // fail the same way.
        CredentialsError::Io { source, .. } => source.kind() == std::io::ErrorKind::InvalidData,
        _ => false,
    }
}

impl SessionStore for CredentialStore {
    fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError> {
        match Self::load(self) {
            Ok(loaded) => Ok(SessionLoad {
                session: loaded.credentials.session_for(origin).cloned(),
                warning: loaded.permissions_warning,
            }),
            Err(err) if file_unreadable(&err) => Ok(SessionLoad {
                session: None,
                warning: Some(format!("ignoring unreadable credentials file: {err}")),
            }),
            Err(err) => Err(err),
        }
    }

    fn store(&self, origin: &str, session: &Session) -> Result<(), CredentialsError> {
        let mut credentials = match Self::load(self) {
            Ok(loaded) => loaded.credentials,
            Err(err) if file_unreadable(&err) => Credentials::default(),
            Err(err) => return Err(err),
        };
        credentials.set_session(origin.to_owned(), session.clone());
        self.save(&credentials)
    }

    fn remove(&self, origin: &str) -> Result<bool, CredentialsError> {
        let mut credentials = match Self::load(self) {
            Ok(loaded) => loaded.credentials,
            Err(err) if file_unreadable(&err) => return Ok(false),
            Err(err) => return Err(err),
        };
        if !credentials.remove_session(origin) {
            return Ok(false);
        }
        self.save(&credentials)?;
        Ok(true)
    }
}

/// Keychain service name every session entry lives under; the entry's
/// account is the normalized index origin.
const KEYCHAIN_SERVICE: &str = "cabin-registry";

/// The platform-keychain backend, via the `keyring` crate: macOS
/// Keychain, Windows Credential Manager, or the Linux secret-service.
/// Every platform failure surfaces as
/// [`CredentialsError::KeychainUnavailable`] so [`SessionStorage`]
/// falls back to the file - a headless Linux box without a D-Bus
/// session must degrade, never fail.
#[derive(Debug, Clone, Copy)]
pub struct KeychainSessionStore;

/// Serde shape of the keychain entry's value: the session as a small
/// JSON object.  Private so token strings never travel outside this
/// crate un-redacted; no `Debug` derive for the same reason.
#[derive(serde::Serialize, Deserialize)]
struct RawKeychainSession {
    token: String,
    expires_at: String,
    api_url: String,
}

impl KeychainSessionStore {
    fn entry(origin: &str) -> Result<keyring::Entry, CredentialsError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, origin).map_err(|err| keychain_unavailable(&err))
    }
}

fn keychain_unavailable(err: &keyring::Error) -> CredentialsError {
    CredentialsError::KeychainUnavailable {
        message: err.to_string(),
    }
}

impl SessionStore for KeychainSessionStore {
    fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError> {
        let value = match Self::entry(origin)?.get_password() {
            Ok(value) => value,
            Err(keyring::Error::NoEntry) => return Ok(SessionLoad::default()),
            Err(err) => return Err(keychain_unavailable(&err)),
        };
        // An entry this client cannot read back (hand-edited, or a
        // format from a future Cabin) reads as absent: the next
        // `cabin login` simply overwrites it.
        let session = serde_json::from_str::<RawKeychainSession>(&value)
            .ok()
            .and_then(|raw| {
                Some(Session {
                    token: Token::parse(&raw.token).ok()?,
                    expires_at: raw.expires_at,
                    api_url: raw.api_url,
                })
            });
        Ok(SessionLoad {
            session,
            warning: None,
        })
    }

    fn store(&self, origin: &str, session: &Session) -> Result<(), CredentialsError> {
        let raw = RawKeychainSession {
            token: session.token.expose().to_owned(),
            expires_at: session.expires_at.clone(),
            api_url: session.api_url.clone(),
        };
        let value =
            serde_json::to_string(&raw).map_err(|err| CredentialsError::KeychainUnavailable {
                message: err.to_string(),
            })?;
        Self::entry(origin)?
            .set_password(&value)
            .map_err(|err| keychain_unavailable(&err))
    }

    fn remove(&self, origin: &str) -> Result<bool, CredentialsError> {
        match Self::entry(origin)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(keychain_unavailable(&err)),
        }
    }
}

/// The composed session storage the CLI uses: the platform keychain
/// first, the `credentials.toml` file as the fallback.  A keychain
/// that answers [`CredentialsError::KeychainUnavailable`] degrades to
/// the file on every operation; any other backend error propagates.
pub struct SessionStorage {
    keychain: Option<Box<dyn SessionStore>>,
    file: Option<Box<dyn SessionStore>>,
}

/// What [`SessionStorage::store`] did, so `cabin login` can print the
/// one-line fallback notice - only for an actual fallback: a
/// keychain-bypassed storage (`CABIN_CONFIG_HOME`) *chose* the file,
/// and warning there would name a cause that never occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredIn {
    Keychain,
    /// The 0600 `credentials.toml` file, as the chosen destination
    /// (no keychain backend was configured).
    File,
    /// The keychain was expected but unavailable; the session fell
    /// back to the 0600 `credentials.toml` file.
    FileFallback,
}

impl SessionStorage {
    /// Resolve the backends from the process environment.  With
    /// `CABIN_CONFIG_HOME` set the keychain is bypassed entirely (the
    /// override marks a hermetic environment - tests, CI sandboxes -
    /// where the platform keychain would leak sessions across runs);
    /// a missing user config home degrades to "no file backend"
    /// rather than an error, so unauthenticated flows keep working in
    /// home-less environments.
    #[must_use]
    pub fn from_env() -> Self {
        let hermetic =
            std::env::var_os(cabin_env::CABIN_CONFIG_HOME).is_some_and(|dir| !dir.is_empty());
        Self {
            keychain: (!hermetic).then(|| Box::new(KeychainSessionStore) as Box<dyn SessionStore>),
            file: CredentialStore::from_env()
                .ok()
                .map(|store| Box::new(store) as Box<dyn SessionStore>),
        }
    }

    /// Storage from explicit backends, for tests.
    #[must_use]
    pub fn from_parts(
        keychain: Option<Box<dyn SessionStore>>,
        file: Option<Box<dyn SessionStore>>,
    ) -> Self {
        Self { keychain, file }
    }

    /// The stored session for `origin`.  A store that fell back to
    /// the file during a transient keychain failure leaves the old
    /// keychain entry behind, and it must not shadow the fresh
    /// session: the file's session wins when the keychain's is
    /// expired, and also when the file's expires strictly later - a
    /// later mint, since the unexpired keychain entry may already be
    /// revoked (which is what prompted the re-login).
    ///
    /// # Errors
    /// Propagates file-backend errors; an unavailable keychain is a
    /// fallback, not an error.
    pub fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError> {
        let mut keychain_load = None;
        if let Some(keychain) = &self.keychain {
            match keychain.load(origin) {
                Ok(load) if load.session.is_some() => keychain_load = Some(load),
                Ok(_) | Err(CredentialsError::KeychainUnavailable { .. }) => {}
                Err(err) => return Err(err),
            }
        }
        let file_load = match &self.file {
            Some(file) => file.load(origin)?,
            None => SessionLoad::default(),
        };
        let Some(mut keychain_load) = keychain_load else {
            return Ok(file_load);
        };
        let superseded = match (&keychain_load.session, &file_load.session) {
            (Some(keychain), Some(file)) => {
                keychain.expired_at(SystemTime::now()) || expires_strictly_after(file, keychain)
            }
            _ => false,
        };
        if superseded {
            return Ok(file_load);
        }
        // Nothing fresher on file: even an expired keychain session
        // is answered, so the "session expired" advice still names
        // the cause, carrying any file warning along.
        keychain_load.warning = keychain_load.warning.or(file_load.warning);
        Ok(keychain_load)
    }

    /// Every backend's stored session for `origin`, keychain first.
    /// Distinct entries can accumulate when a store falls back to
    /// the file during a keychain outage; `cabin logout` revokes
    /// each of them before removing, so neither can outlive the
    /// logout server-side.  An unavailable keychain contributes
    /// nothing, exactly as in [`SessionStorage::load`].
    ///
    /// # Errors
    /// Propagates file-backend errors.
    pub fn load_each(&self, origin: &str) -> Result<Vec<Session>, CredentialsError> {
        let mut sessions = Vec::new();
        if let Some(keychain) = &self.keychain {
            match keychain.load(origin) {
                Ok(load) => sessions.extend(load.session),
                Err(CredentialsError::KeychainUnavailable { .. }) => {}
                Err(err) => return Err(err),
            }
        }
        if let Some(file) = &self.file {
            sessions.extend(file.load(origin)?.session);
        }
        Ok(sessions)
    }

    /// Store `session` for `origin` in the keychain, falling back to
    /// the file when the keychain is unavailable.
    ///
    /// # Errors
    /// Propagates file-backend write failures, and
    /// [`CredentialsError::NoConfigHome`] when the keychain is
    /// unavailable and no file location can be resolved either.
    pub fn store(&self, origin: &str, session: &Session) -> Result<StoredIn, CredentialsError> {
        let mut fell_back = false;
        if let Some(keychain) = &self.keychain {
            match keychain.store(origin, session) {
                Ok(()) => return Ok(StoredIn::Keychain),
                Err(CredentialsError::KeychainUnavailable { .. }) => fell_back = true,
                Err(err) => return Err(err),
            }
        }
        match &self.file {
            Some(file) => {
                file.store(origin, session)?;
                Ok(if fell_back {
                    StoredIn::FileFallback
                } else {
                    StoredIn::File
                })
            }
            None => Err(CredentialsError::NoConfigHome),
        }
    }

    /// Remove the entry for `origin` from every backend.
    ///
    /// # Errors
    /// Propagates file-backend write failures; an unavailable
    /// keychain is skipped, but the receipt says so - a session
    /// stored there survives the removal and would resurface when the
    /// keychain recovers, so `cabin logout` must not claim otherwise.
    pub fn remove(&self, origin: &str) -> Result<Removal, CredentialsError> {
        let mut removal = Removal {
            removed: false,
            keychain_unreachable: false,
        };
        if let Some(keychain) = &self.keychain {
            match keychain.remove(origin) {
                Ok(hit) => removal.removed |= hit,
                Err(CredentialsError::KeychainUnavailable { .. }) => {
                    removal.keychain_unreachable = true;
                }
                Err(err) => return Err(err),
            }
        }
        if let Some(file) = &self.file {
            removal.removed |= file.remove(origin)?;
        }
        Ok(removal)
    }
}

/// Whether `a`'s `expires_at` is strictly later than `b`'s.  Only
/// comparable when both parse; like [`Session::expired_at`], an
/// unparsable expiry is advisory and never supersedes.
fn expires_strictly_after(a: &Session, b: &Session) -> bool {
    match (
        humantime::parse_rfc3339(&a.expires_at),
        humantime::parse_rfc3339(&b.expires_at),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

/// What [`SessionStorage::remove`] did: whether any backend held an
/// entry, and whether the keychain leg could not even be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removal {
    pub removed: bool,
    /// The keychain answered
    /// [`CredentialsError::KeychainUnavailable`]: an entry stored
    /// there (if any) was not removed.
    pub keychain_unreachable: bool,
}

/// The environment override under the caller's origin-trust
/// decision: disallowed means the value is not consulted at all - a
/// malformed value is irrelevant to an origin the override does not
/// apply to.
fn gated_env_token(
    env_value: Option<&OsStr>,
    allow_env_override: bool,
) -> Result<Option<Token>, CredentialsError> {
    if !allow_env_override {
        return Ok(None);
    }
    token_from_env_value(env_value)
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
/// environment override when `allow_env_override` is set, else the
/// stored session.  The override is consulted *before* storage is
/// even located, so it keeps working in home-less environments (CI
/// containers) where no user config home can be resolved; there, a
/// missing config home simply means "no stored credential" rather
/// than an error, so unauthenticated flows never fail either.
///
/// `allow_env_override` is the caller's origin-trust decision: the
/// environment override is a single credential with no origin key of
/// its own, and an invocation's index origin can come from
/// project-level config or `[source-replacement]` - inputs a checked
/// out project controls.  Callers must only allow the override for
/// origins the *user* chose (Cabin's default registry, loopback
/// testing); stored sessions are origin-keyed by construction and are
/// always consulted.
///
/// # Errors
/// Rejects a malformed environment override with
/// [`CredentialsError::InvalidToken`] rather than sending garbage
/// bytes in an `Authorization` header, and propagates
/// [`CredentialStore::load`] errors for an unreadable or invalid
/// credentials file.
pub fn lookup_token(
    origin: &str,
    allow_env_override: bool,
) -> Result<TokenLookup, CredentialsError> {
    lookup_token_with_env(
        std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).as_deref(),
        origin,
        allow_env_override,
    )
}

/// [`lookup_token`] with the environment value injected for tests.
///
/// # Errors
/// Same as [`lookup_token`].
pub fn lookup_token_with_env(
    env_value: Option<&OsStr>,
    origin: &str,
    allow_env_override: bool,
) -> Result<TokenLookup, CredentialsError> {
    if let Some(token) = gated_env_token(env_value, allow_env_override)? {
        return Ok(TokenLookup {
            token: Some(token),
            expired_at: None,
            warning: None,
        });
    }
    stored_token(origin)
}

/// The environment-override leg of [`lookup_token`] alone, for
/// callers that interpose another credential source (the trusted
/// publishing auto-exchange) between the override and the store.
/// `allow_env_override` carries the same origin-trust contract as
/// [`lookup_token`].
///
/// # Errors
/// Rejects a malformed environment override with
/// [`CredentialsError::InvalidToken`].
pub fn env_token(allow_env_override: bool) -> Result<Option<Token>, CredentialsError> {
    gated_env_token(
        std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).as_deref(),
        allow_env_override,
    )
}

/// The stored-session leg of [`lookup_token`] alone: the session for
/// `origin` from [`SessionStorage`] (keychain first, file fallback),
/// withheld with `expired_at` set when its expiry has passed.
///
/// # Errors
/// Propagates file-backend errors.
pub fn stored_token(origin: &str) -> Result<TokenLookup, CredentialsError> {
    let load = SessionStorage::from_env().load(origin)?;
    let (token, expired_at) = match load.session {
        Some(session) if session.expired_at(SystemTime::now()) => (None, Some(session.expires_at)),
        Some(session) => (Some(session.token), None),
        None => (None, None),
    };
    Ok(TokenLookup {
        token,
        expired_at,
        warning: load.warning,
    })
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
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawRegistryCredential {
    token: String,
    expires_at: String,
    api_url: String,
}

/// Write-side mirror of [`RawCredentials`], borrowing so the token
/// is never copied around more than the serializer requires.
#[derive(serde::Serialize)]
struct RawCredentialsOut<'a> {
    registries: BTreeMap<&'a str, RawRegistryCredentialOut<'a>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct RawRegistryCredentialOut<'a> {
    token: &'a str,
    expires_at: &'a str,
    api_url: &'a str,
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

    #[error(
        "invalid credentials file {path}: {message}; run `cabin login` to store a fresh session"
    )]
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

    /// The platform keychain cannot serve this process (no keychain
    /// daemon, no D-Bus session, access denied).  [`SessionStorage`]
    /// treats this as "fall back to the file", so it only escapes
    /// when no fallback exists either.
    #[error("the platform keychain is unavailable: {message}")]
    KeychainUnavailable { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::Duration;

    const SECRET: &str = "cabin_ses_abcDEF12345";
    const EXPIRES: &str = "2999-01-01T00:00:00.000Z";
    const API: &str = "https://cabinpkg.com";

    fn token() -> Token {
        Token::parse(SECRET).unwrap()
    }

    fn session() -> Session {
        Session {
            token: token(),
            expires_at: EXPIRES.to_owned(),
            api_url: API.to_owned(),
        }
    }

    #[test]
    fn token_parse_accepts_base62_payloads() {
        for raw in ["cabin_12345678", "cabin_abcDEF12345"] {
            assert_eq!(Token::parse(raw).unwrap().expose(), raw);
        }
    }

    /// Trusted-publishing tokens (`cabin_tp_<base64url>`) carry `-`
    /// and `_` in their payload; the widened charset is confined to
    /// the `tp_` marker, so plain user tokens stay base62-only
    /// (`cabin_with-dash1` below keeps rejecting).
    #[test]
    fn token_parse_accepts_trustpub_base64url_payloads() {
        for raw in [
            "cabin_tp_pVp-p_Wl",
            // A real minted shape: 32 CSPRNG bytes render as 43
            // unpadded base64url characters.
            &format!("cabin_tp_{}", &"aB1-_c9Z".repeat(6)[..43]),
        ] {
            assert_eq!(Token::parse(raw).unwrap().expose(), raw);
        }
    }

    #[test]
    fn token_parse_rejects_malformed_trustpub_payloads() {
        for raw in [
            "cabin_tp_",              // payload under the length floor
            "cabin_tp_has space9",    // spaces stay rejected
            "cabin_tp_evil\r\nabc12", // header smuggling stays rejected
            "cabin_tp_has+plus99",    // standard-base64 alphabet is not base64url
            "cabin_tp_has=pad999",    // padding never appears unpadded
        ] {
            let err = Token::parse(raw).unwrap_err();
            assert!(
                matches!(err, CredentialsError::InvalidToken { .. }),
                "{raw:?} should be rejected, got {err:?}"
            );
        }
    }

    /// Login-session tokens (`cabin_ses_<base64url>`) carry `-` and `_`
    /// like trusted-publishing ones; the widened charset is confined to
    /// the `ses_` marker, so plain user tokens stay base62-only
    /// (`cabin_with-dash1` above keeps rejecting).
    #[test]
    fn token_parse_accepts_session_base64url_payloads() {
        for raw in [
            "cabin_ses_pVp-p_Wl",
            // A real minted shape: 32 CSPRNG bytes render as 43
            // unpadded base64url characters.
            &format!("cabin_ses_{}", &"aB1-_c9Z".repeat(6)[..43]),
        ] {
            assert_eq!(Token::parse(raw).unwrap().expose(), raw);
        }
    }

    #[test]
    fn token_parse_rejects_malformed_session_payloads() {
        for raw in [
            "cabin_ses_",             // payload under the length floor
            "cabin_ses_has space",    // spaces stay rejected
            "cabin_ses_evil\r\nabc1", // header smuggling stays rejected
            "cabin_ses_has+plus9",    // standard-base64 alphabet is not base64url
            "cabin_ses_has=pad99",    // padding never appears unpadded
        ] {
            let err = Token::parse(raw).unwrap_err();
            assert!(
                matches!(err, CredentialsError::InvalidToken { .. }),
                "{raw:?} should be rejected, got {err:?}"
            );
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
        credentials.set_session("https://example.com".to_owned(), session());
        let rendered = format!("{credentials:?}");
        assert!(
            !rendered.contains("abcDEF12345"),
            "token bytes leaked through Credentials: {rendered:?}"
        );
    }

    /// The expiry check compares the stored RFC 3339 stamp against
    /// the injected clock; an unparsable stamp reads as not expired
    /// (the registry enforces expiry server-side).
    #[test]
    fn session_expiry_compares_against_the_injected_clock() {
        let expiry = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let session = Session {
            token: token(),
            expires_at: humantime::format_rfc3339(expiry).to_string(),
            api_url: API.to_owned(),
        };
        assert!(!session.expired_at(expiry - Duration::from_secs(1)));
        assert!(session.expired_at(expiry));
        assert!(session.expired_at(expiry + Duration::from_secs(1)));

        // The registry's JS `toISOString` shape (millisecond
        // fraction, `Z`) parses.
        let session = Session {
            token: token(),
            expires_at: "1970-01-02T00:00:00.000Z".to_owned(),
            api_url: API.to_owned(),
        };
        assert!(session.expired_at(SystemTime::now()));

        let unparsable = Session {
            token: token(),
            expires_at: "sometime later".to_owned(),
            api_url: API.to_owned(),
        };
        assert!(!unparsable.expired_at(SystemTime::now()));
    }

    #[test]
    fn normalize_origin_strips_path_slash_and_default_port() {
        for (input, expected) in [
            (
                "https://registry.cabinpkg.com",
                "https://registry.cabinpkg.com",
            ),
            (
                "https://registry.cabinpkg.com/",
                "https://registry.cabinpkg.com",
            ),
            (
                "https://Registry.CabinPkg.com/index/path?q=1#frag",
                "https://registry.cabinpkg.com",
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
        credentials.set_session("https://registry.cabinpkg.com".to_owned(), session());
        store.save(&credentials).unwrap();

        let body = std::fs::read_to_string(store.path()).unwrap();
        assert_eq!(
            body,
            format!(
                "[registries.\"https://registry.cabinpkg.com\"]\ntoken = \"{SECRET}\"\n\
                 expires-at = \"{EXPIRES}\"\napi-url = \"{API}\"\n"
            )
        );

        let loaded = CredentialStore::load(&store).unwrap();
        assert_eq!(
            loaded
                .credentials
                .session_for("https://registry.cabinpkg.com")
                .unwrap(),
            &session()
        );
        assert!(
            loaded
                .credentials
                .session_for("https://other.example")
                .is_none()
        );
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let loaded = CredentialStore::load(&store).unwrap();
        assert!(
            loaded
                .credentials
                .session_for("https://example.com")
                .is_none()
        );
        assert!(loaded.permissions_warning.is_none());
    }

    #[test]
    fn remove_session_reports_whether_an_entry_existed() {
        let mut credentials = Credentials::default();
        credentials.set_session("https://example.com".to_owned(), session());
        assert!(credentials.remove_session("https://example.com"));
        assert!(!credentials.remove_session("https://example.com"));
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_the_file_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("nested").join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_session("https://example.com".to_owned(), session());
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
        let warning = CredentialStore::load(&store)
            .unwrap()
            .permissions_warning
            .unwrap();
        assert!(warning.contains("chmod 600"), "{warning}");
        assert!(warning.contains("644"), "{warning}");

        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            CredentialStore::load(&store)
                .unwrap()
                .permissions_warning
                .is_none()
        );
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
                "[registries.\"https://example.com\"]\ntoken = \"{SECRET}\"\n\
                 expires-at = \"{EXPIRES}\"\napi-url = \"{API}\"\nscope = \"publish\"\n"
            ),
            // Typo'd `token` key.
            format!(
                "[registries.\"https://example.com\"]\ntokn = \"{SECRET}\"\n\
                 expires-at = \"{EXPIRES}\"\napi-url = \"{API}\"\n"
            ),
        ] {
            std::fs::write(store.path(), &body).unwrap();
            let err = CredentialStore::load(&store).unwrap_err();
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

    /// The pre-session `token`-only entry shape (a long-lived pasted
    /// key) no longer parses: the registry deleted those keys, and a
    /// stale file must fail toward `cabin login`, not send a dead
    /// credential.
    #[test]
    fn parse_rejects_the_legacy_token_only_shape() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        std::fs::write(
            store.path(),
            format!("[registries.\"https://example.com\"]\ntoken = \"{SECRET}\"\n"),
        )
        .unwrap();
        let err = CredentialStore::load(&store).unwrap_err();
        assert!(
            matches!(err, CredentialsError::Parse { .. }),
            "expected Parse error, got {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("cabin login"), "{message}");
        assert!(!message.contains(SECRET), "token leaked: {message}");
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
                format!(
                    "[registries.\"{key}\"]\ntoken = \"{SECRET}\"\n\
                     expires-at = \"{EXPIRES}\"\napi-url = \"{API}\"\n"
                ),
            )
            .unwrap();
            let err = CredentialStore::load(&store).unwrap_err();
            assert!(
                matches!(err, CredentialsError::NonNormalizedKey { .. }),
                "{key:?} should be rejected as non-normalized, got {err:?}"
            );
        }
    }

    /// Unset and empty environment values are "no override", so an
    /// allowed lookup still answers from parsing alone.
    #[test]
    fn empty_or_absent_env_is_no_override() {
        for env in [None, Some(OsStr::new(""))] {
            assert!(gated_env_token(env, true).unwrap().is_none());
        }
    }

    /// The env override is honored before storage is even located,
    /// so it works in home-less environments where no user config
    /// home resolves.
    #[test]
    fn lookup_token_env_override_applies_before_the_store_is_located() {
        let lookup = lookup_token_with_env(
            Some(OsStr::new("cabin_envToken12345")),
            "https://x.example",
            true,
        )
        .unwrap();
        assert_eq!(lookup.token.unwrap().expose(), "cabin_envToken12345");
        assert!(lookup.expired_at.is_none());
    }

    /// The caller's origin-trust decision is binding: with the
    /// override disallowed, the env token is never returned - not
    /// even validated.
    #[test]
    fn gated_env_token_ignores_the_override_when_disallowed() {
        for env in [
            Some(OsStr::new("cabin_envToken12345")),
            // A malformed value is irrelevant to an origin the
            // override does not apply to.
            Some(OsStr::new("not-a-token")),
        ] {
            assert!(gated_env_token(env, false).unwrap().is_none());
        }
        // Allowed, the same values keep their original semantics.
        assert_eq!(
            gated_env_token(Some(OsStr::new("cabin_envToken12345")), true)
                .unwrap()
                .unwrap()
                .expose(),
            "cabin_envToken12345"
        );
        assert!(gated_env_token(Some(OsStr::new("not-a-token")), true).is_err());
    }

    #[test]
    fn malformed_env_override_is_rejected_not_sent() {
        let err =
            lookup_token_with_env(Some(OsStr::new("not-a-token")), "https://example.com", true)
                .unwrap_err();
        assert!(matches!(err, CredentialsError::InvalidToken { .. }));
    }

    #[test]
    fn saved_file_is_deterministic_and_sorted() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        let mut credentials = Credentials::default();
        credentials.set_session("https://zeta.example".to_owned(), session());
        credentials.set_session("https://alpha.example".to_owned(), session());
        store.save(&credentials).unwrap();
        let body = std::fs::read_to_string(store.path()).unwrap();
        let alpha = body.find("alpha.example").unwrap();
        let zeta = body.find("zeta.example").unwrap();
        assert!(
            alpha < zeta,
            "origins must be written in sorted order:\n{body}"
        );
        // Round-trips through the parser.
        CredentialStore::load(&store).unwrap();
    }

    // -----------------------------------------------------------------
    // SessionStorage composition: keychain-first, file fallback
    // -----------------------------------------------------------------

    /// In-memory [`SessionStore`] test double.  `unavailable` makes
    /// every operation answer the keychain-unavailable error, the
    /// shape a keychain-less host produces; a `Cell` so outage tests
    /// can toggle it mid-scenario through a shared handle.
    struct MemoryStore {
        sessions: RefCell<HashMap<String, Session>>,
        unavailable: std::cell::Cell<bool>,
    }

    impl MemoryStore {
        fn new(unavailable: bool) -> Self {
            Self {
                sessions: RefCell::new(HashMap::new()),
                unavailable: std::cell::Cell::new(unavailable),
            }
        }
    }

    /// Shared-handle delegation, so a test can keep toggling a store
    /// after boxing it into a [`SessionStorage`].
    impl SessionStore for std::rc::Rc<MemoryStore> {
        fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError> {
            self.as_ref().load(origin)
        }

        fn store(&self, origin: &str, session: &Session) -> Result<(), CredentialsError> {
            self.as_ref().store(origin, session)
        }

        fn remove(&self, origin: &str) -> Result<bool, CredentialsError> {
            self.as_ref().remove(origin)
        }
    }

    impl SessionStore for MemoryStore {
        fn load(&self, origin: &str) -> Result<SessionLoad, CredentialsError> {
            if self.unavailable.get() {
                return Err(CredentialsError::KeychainUnavailable {
                    message: "no backend".to_owned(),
                });
            }
            Ok(SessionLoad {
                session: self.sessions.borrow().get(origin).cloned(),
                warning: None,
            })
        }

        fn store(&self, origin: &str, session: &Session) -> Result<(), CredentialsError> {
            if self.unavailable.get() {
                return Err(CredentialsError::KeychainUnavailable {
                    message: "no backend".to_owned(),
                });
            }
            self.sessions
                .borrow_mut()
                .insert(origin.to_owned(), session.clone());
            Ok(())
        }

        fn remove(&self, origin: &str) -> Result<bool, CredentialsError> {
            if self.unavailable.get() {
                return Err(CredentialsError::KeychainUnavailable {
                    message: "no backend".to_owned(),
                });
            }
            Ok(self.sessions.borrow_mut().remove(origin).is_some())
        }
    }

    const ORIGIN: &str = "https://registry.cabinpkg.com";

    #[test]
    fn storage_prefers_an_available_keychain() {
        let storage = SessionStorage::from_parts(
            Some(Box::new(MemoryStore::new(false))),
            Some(Box::new(MemoryStore::new(false))),
        );
        assert_eq!(
            storage.store(ORIGIN, &session()).unwrap(),
            StoredIn::Keychain
        );
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), session());
        assert!(storage.remove(ORIGIN).unwrap().removed);
        assert!(storage.load(ORIGIN).unwrap().session.is_none());
    }

    /// The fallback path: an unavailable keychain degrades every
    /// operation to the file backend, and the store receipt says so
    /// (that is what triggers `cabin login`'s one-line notice).
    #[test]
    fn storage_falls_back_to_the_file_when_the_keychain_is_unavailable() {
        let storage = SessionStorage::from_parts(
            Some(Box::new(MemoryStore::new(true))),
            Some(Box::new(MemoryStore::new(false))),
        );
        assert_eq!(
            storage.store(ORIGIN, &session()).unwrap(),
            StoredIn::FileFallback
        );
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), session());
        assert!(storage.remove(ORIGIN).unwrap().removed);
        assert!(!storage.remove(ORIGIN).unwrap().removed);
    }

    /// A keychain-less storage (the `CABIN_CONFIG_HOME` bypass) stores
    /// to the file as the *chosen* destination: no fallback receipt,
    /// so `cabin login` prints no keychain-unavailable notice.
    #[test]
    fn storage_without_a_keychain_chooses_the_file_without_a_fallback_receipt() {
        let storage = SessionStorage::from_parts(None, Some(Box::new(MemoryStore::new(false))));
        assert_eq!(storage.store(ORIGIN, &session()).unwrap(), StoredIn::File);
    }

    /// A keychain outage spanning login and logout: the fresh session
    /// falls back to the file, the removal receipt reports the
    /// unreachable keychain (that is what triggers `cabin logout`'s
    /// warning - the entry stored there survives), and once the
    /// keychain recovers a repeat removal clears the residue.
    #[test]
    fn storage_reports_the_unreachable_keychain_on_removal() {
        let keychain = std::rc::Rc::new(MemoryStore::new(false));
        let storage = SessionStorage::from_parts(
            Some(Box::new(std::rc::Rc::clone(&keychain))),
            Some(Box::new(MemoryStore::new(false))),
        );
        assert_eq!(
            storage.store(ORIGIN, &session()).unwrap(),
            StoredIn::Keychain
        );

        keychain.unavailable.set(true);
        assert_eq!(
            storage.store(ORIGIN, &session()).unwrap(),
            StoredIn::FileFallback
        );
        let removal = storage.remove(ORIGIN).unwrap();
        assert!(removal.removed, "the file copy must be removed");
        assert!(removal.keychain_unreachable);

        keychain.unavailable.set(false);
        let removal = storage.remove(ORIGIN).unwrap();
        assert!(removal.removed, "the keychain residue clears on recovery");
        assert!(!removal.keychain_unreachable);
        assert!(storage.load(ORIGIN).unwrap().session.is_none());
    }

    /// A stale expired keychain entry must not shadow a fresher file
    /// session: a store that fell back to the file during a transient
    /// keychain failure leaves the old entry in the keychain, and the
    /// next load has to answer the live session, not the stale one.
    #[test]
    fn storage_prefers_the_file_over_an_expired_keychain_session() {
        let keychain = Box::new(MemoryStore::new(false));
        let expired = Session {
            expires_at: "2000-01-01T00:00:00.000Z".to_owned(),
            ..session()
        };
        keychain.store(ORIGIN, &expired).unwrap();
        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &session()).unwrap();
        let storage = SessionStorage::from_parts(Some(keychain), Some(file));
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), session());

        // With nothing on file, the expired keychain session is still
        // answered, so the "session expired" advice can name the
        // cause.
        let keychain = Box::new(MemoryStore::new(false));
        keychain.store(ORIGIN, &expired).unwrap();
        let storage =
            SessionStorage::from_parts(Some(keychain), Some(Box::new(MemoryStore::new(false))));
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), expired);
    }

    /// The stale keychain entry can also be *unexpired* - a revoked
    /// session is what prompted the re-login - so between two live
    /// sessions the load answers whichever expires later: the one
    /// minted later.
    #[test]
    fn storage_prefers_the_later_expiring_live_session() {
        let older = Session {
            expires_at: "2999-01-01T00:00:00.000Z".to_owned(),
            ..session()
        };
        let newer = Session {
            expires_at: "2999-06-01T00:00:00.000Z".to_owned(),
            ..session()
        };

        // A fresh login fell back to the file; the keychain kept the
        // older entry.
        let keychain = Box::new(MemoryStore::new(false));
        keychain.store(ORIGIN, &older).unwrap();
        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &newer).unwrap();
        let storage = SessionStorage::from_parts(Some(keychain), Some(file));
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), newer);

        // The mirror: a later login reached the recovered keychain
        // while the file kept an older fallback entry.
        let keychain = Box::new(MemoryStore::new(false));
        keychain.store(ORIGIN, &newer).unwrap();
        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &older).unwrap();
        let storage = SessionStorage::from_parts(Some(keychain), Some(file));
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), newer);
    }

    /// `load_each` answers every backend's session - `load` picks
    /// one to send, but `cabin logout` must revoke both when a
    /// keychain outage left them divergent - and an unavailable
    /// keychain contributes nothing rather than failing.
    #[test]
    fn load_each_answers_every_backend_session() {
        let older = Session {
            expires_at: "2999-01-01T00:00:00.000Z".to_owned(),
            ..session()
        };
        let newer = Session {
            expires_at: "2999-06-01T00:00:00.000Z".to_owned(),
            ..session()
        };
        let keychain = Box::new(MemoryStore::new(false));
        keychain.store(ORIGIN, &older).unwrap();
        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &newer).unwrap();
        let storage = SessionStorage::from_parts(Some(keychain), Some(file));
        assert_eq!(
            storage.load_each(ORIGIN).unwrap(),
            vec![older, newer.clone()]
        );

        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &newer).unwrap();
        let storage =
            SessionStorage::from_parts(Some(Box::new(MemoryStore::new(true))), Some(file));
        assert_eq!(storage.load_each(ORIGIN).unwrap(), vec![newer]);
    }

    /// A session that landed in the file (an earlier fallback) is
    /// still found when the keychain is present but empty, and
    /// removal clears every backend.
    #[test]
    fn storage_load_and_remove_reach_the_file_behind_an_empty_keychain() {
        let keychain = Box::new(MemoryStore::new(false));
        let file = Box::new(MemoryStore::new(false));
        file.store(ORIGIN, &session()).unwrap();
        let storage = SessionStorage::from_parts(Some(keychain), Some(file));
        assert_eq!(storage.load(ORIGIN).unwrap().session.unwrap(), session());
        assert!(storage.remove(ORIGIN).unwrap().removed);
        assert!(storage.load(ORIGIN).unwrap().session.is_none());
    }

    #[test]
    fn storage_store_without_any_backend_is_a_hard_error() {
        let storage = SessionStorage::from_parts(Some(Box::new(MemoryStore::new(true))), None);
        let err = storage.store(ORIGIN, &session()).unwrap_err();
        assert!(matches!(err, CredentialsError::NoConfigHome), "{err:?}");
        // Loads and removals just answer "nothing stored".
        assert!(storage.load(ORIGIN).unwrap().session.is_none());
        assert!(!storage.remove(ORIGIN).unwrap().removed);
    }

    /// The file-backend `SessionStore` impl round-trips through the
    /// real `credentials.toml` reader/writer.
    #[test]
    fn credential_store_implements_the_session_store_contract() {
        let dir = TempDir::new().unwrap();
        let store = CredentialStore::at(dir.path().join("credentials.toml"));
        assert!(
            SessionStore::load(&store, ORIGIN)
                .unwrap()
                .session
                .is_none()
        );
        SessionStore::store(&store, ORIGIN, &session()).unwrap();
        assert_eq!(
            SessionStore::load(&store, ORIGIN).unwrap().session.unwrap(),
            session()
        );
        assert!(SessionStore::remove(&store, ORIGIN).unwrap());
        assert!(!SessionStore::remove(&store, ORIGIN).unwrap());
    }

    /// A file this client cannot read - here the pre-session
    /// `token`-only shape - must not lock the store: as a
    /// `SessionStore` it reads as absent (with a warning), a removal
    /// is a no-op, and the next store replaces it wholesale, so the
    /// `cabin login` the warning recommends actually works.
    #[test]
    fn an_unreadable_file_reads_as_absent_and_is_replaced_by_store() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(
            &path,
            format!("[registries.\"{ORIGIN}\"]\ntoken = \"{SECRET}\"\n"),
        )
        .unwrap();
        let store = CredentialStore::at(&path);

        let load = SessionStore::load(&store, ORIGIN).unwrap();
        assert!(load.session.is_none());
        let warning = load.warning.unwrap();
        assert!(
            warning.contains("unreadable credentials file") && !warning.contains(SECRET),
            "{warning}"
        );
        assert!(!SessionStore::remove(&store, ORIGIN).unwrap());

        SessionStore::store(&store, ORIGIN, &session()).unwrap();
        let load = SessionStore::load(&store, ORIGIN).unwrap();
        assert_eq!(load.session.unwrap(), session());
        assert!(load.warning.is_none());

        // Invalid UTF-8 is content damage like unparsable TOML:
        // absent, then replaced.
        std::fs::write(&path, b"[registries]\xff\xfe").unwrap();
        let load = SessionStore::load(&store, ORIGIN).unwrap();
        assert!(load.session.is_none());
        assert!(load.warning.is_some());
        SessionStore::store(&store, ORIGIN, &session()).unwrap();
        assert_eq!(
            SessionStore::load(&store, ORIGIN).unwrap().session.unwrap(),
            session()
        );
    }

    /// The stored-session lookup withholds an expired session's token
    /// and says why, so callers can advise `cabin login` instead of
    /// sending a credential the registry will refuse.
    #[test]
    fn expired_sessions_are_withheld_with_the_expired_flag() {
        let storage = SessionStorage::from_parts(None, Some(Box::new(MemoryStore::new(false))));
        let expired = Session {
            token: token(),
            expires_at: "1970-01-02T00:00:00.000Z".to_owned(),
            api_url: API.to_owned(),
        };
        storage.store(ORIGIN, &expired).unwrap();
        let load = storage.load(ORIGIN).unwrap();
        // `SessionStorage` itself hands back the raw session; the
        // expiry decision is `stored_token`'s (tested through
        // `Session::expired_at` above) so callers that need the
        // session itself - `cabin logout` revoking - still see it.
        assert!(load.session.unwrap().expired_at(SystemTime::now()));
    }
}
