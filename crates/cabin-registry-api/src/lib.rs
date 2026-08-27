//! Typed HTTP client for the remote registry API's mutation
//! routes (experimental, `-Z remote-registry`).
//!
//! This crate owns the *mutating* half of the remote-registry
//! protocol specified in `docs/remote-registry.md`.  Registry
//! packages are always scoped, so the package routes address the
//! `<scope>/<name>` pair:
//!
//! - [`RegistryApi::publish`] -
//!   `PUT /api/v1/packages/<scope>/<name>/<version>` with the
//!   crates.io-style length-prefixed body
//!   (`[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`);
//! - [`RegistryApi::set_yanked`] -
//!   `PATCH /api/v1/packages/<scope>/<name>/<version>/yank` with a
//!   JSON `{"yanked": bool}` body;
//! - [`exchange_trusted_publishing`] / -
//!   [`RegistryApi::revoke_trusted_publishing`] -
//!   `PUT` / `DELETE /api/v1/trusted_publishing/tokens`, plus
//!   [`fetch_github_actions_jwt`], the GitHub Actions runner OIDC
//!   fetch the exchange consumes;
//! - [`exchange_login_session`] / [`RegistryApi::revoke_session`] -
//!   `PUT` / `DELETE /api/v1/sessions/tokens`, plus
//!   [`request_device_authorization`] and [`poll_device_token`], the
//!   GitHub OAuth device flow `cabin login` drives to obtain the
//!   access token the mint consumes.  The GitHub calls are the
//!   crate's non-registry calls, kept here for the shared transport
//!   rules and secret hygiene.
//!
//! Every route lives on the registry's `api` origin (the `api` field
//! of its `config.json`) and, package routes and revocation alike,
//! authenticates with the `Authorization: Bearer <token>` credential -
//! except the trusted-publishing exchange, which deliberately sends no
//! `Authorization` header: the OIDC JWT in its body is the credential,
//! and API discovery on that leg is an unauthenticated `config.json`
//! read (there is no token yet).  The caller resolves credentials
//! through `cabin-credentials` (and decides *whether* to exchange -
//! the GitHub Actions detection and precedence live in the CLI's
//! orchestration layer) and hands in typed values; this crate never
//! reads `credentials.toml` or the environment itself.
//!
//! Crate boundaries:
//! - no staging, validation, or lint logic - `cabin-package` /
//!   `cabin-publish` produce the archive and metadata bytes, this
//!   crate only frames and ships them;
//! - no read routes - `config.json`, package metadata, and artifact
//!   downloads stay in `cabin-index-http`;
//! - token bytes never surface through errors or `Debug` output
//!   ([`Token`] redacts).

use std::io::Read as _;
use std::time::Duration;

use cabin_core::escape_control_chars;
use cabin_credentials::Token;
use serde::Deserialize;
use thiserror::Error;

/// Per-request timeout, matching the sparse HTTP read client.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on how much of a non-2xx response body is read while looking
/// for the error envelope.  Envelopes are tiny; anything bigger is
/// not one.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// The envelope `code` the registry's budget breaker sets
/// (`docs/remote-registry.md`, "Error envelope").  It is what separates
/// a breaker refusal from every other `503` on the wire.
const OVER_BUDGET_CODE: &str = "registry_over_budget";

/// Client for one registry's API origin.  Construction validates the
/// URL (http(s), no userinfo) and enforces the same cleartext rule as
/// the read path: the credential-bearing mutation routes are refused
/// over plain `http` beyond loopback hosts.
pub struct RegistryApi {
    agent: ureq::Agent,
    /// Validated API base URL, always with a trailing `/`.
    base: url::Url,
    /// Normalized API origin, for error messages.
    origin: String,
    token: Option<Token>,
}

/// What a successful [`RegistryApi::publish`] meant on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// `201`: the version did not exist and was created.
    Created,
    /// `200`: byte-identical metadata and archive were already
    /// published; the request was an idempotent no-op.
    AlreadyPublished,
}

/// A successful publish: the outcome plus the response body's optional
/// `"verification"` field, read tolerantly - `Some("pending")` on a
/// registry with the asynchronous verification lifecycle, `None` on one
/// without it (or an unreadable body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishReceipt {
    pub outcome: PublishOutcome,
    pub verification: Option<String>,
    /// The response body's optional `"revision"` field: the
    /// packaging-revision id the archive published under, read as
    /// tolerantly as `verification`.
    pub revision: Option<String>,
}

impl RegistryApi {
    /// Build a client for the registry API at `api_url` (the `api`
    /// field of the registry's `config.json`), attaching `token` to
    /// every request when one is supplied.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::InvalidApiUrl`] when `api_url` is
    /// not a valid `http(s)` URL or carries userinfo credentials, and
    /// [`RegistryApiError::CleartextApiUrl`] when it uses plain
    /// `http` beyond loopback hosts - a bearer token must never
    /// travel in cleartext, mirroring the read path's rule.
    pub fn new(api_url: &str, token: Option<Token>) -> Result<Self, RegistryApiError> {
        // `normalize_origin` performs the full hygiene check
        // (scheme, host, userinfo) with userinfo redacted from its
        // own error messages, so this crate cannot drift on the rule.
        let origin = cabin_credentials::normalize_origin(api_url).map_err(|err| {
            RegistryApiError::InvalidApiUrl {
                message: err.to_string(),
            }
        })?;
        if !origin.starts_with("https://") && !cabin_credentials::url_is_loopback(api_url) {
            return Err(RegistryApiError::CleartextApiUrl { origin });
        }
        let mut base = url::Url::parse(api_url).map_err(|err| RegistryApiError::InvalidApiUrl {
            message: err.to_string(),
        })?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self {
            // Redirects are refused so a mutation can never be
            // bounced to a different origin than the one the
            // registry's `config.json` declared.
            agent: ureq::AgentBuilder::new()
                .timeout(DEFAULT_TIMEOUT)
                .redirects(0)
                .build(),
            base,
            origin,
            token,
        })
    }

    /// `PUT <api>/api/v1/packages/<scope>/<name>/<version>` with the
    /// framed metadata + archive body.  `name` is the full scoped
    /// `<scope>/<name>` string.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` is bare or fails the hosted registry's
    /// name grammar (scope: lowercase/digits/interior `-`; name part:
    /// `[a-z0-9][a-z0-9_-]*`), and
    /// [`RegistryApiError::FrameTooLarge`] when either payload
    /// exceeds the `u32` framing limit.  Response statuses map per
    /// `docs/remote-registry.md`: `409` becomes
    /// [`RegistryApiError::VersionConflict`], `400` / `401` map like
    /// the read path, a token-authenticated `403` surfaces the
    /// server's envelope detail as [`RegistryApiError::Forbidden`]
    /// (unless a `quota_*` code marks it as
    /// [`RegistryApiError::QuotaExceeded`]),
    /// the quota and budget refusals map to
    /// [`RegistryApiError::ArchiveTooLarge`] (`413`),
    /// [`RegistryApiError::RateLimited`] (`429`), and
    /// [`RegistryApiError::RegistryOverBudget`] (`503`) - the last two
    /// carrying the response's `Retry-After` seconds when usable - and
    /// any other non-success status surfaces as
    /// [`RegistryApiError::ServerError`] with the error envelope's
    /// `detail` when the body carries one.
    pub fn publish(
        &self,
        name: &str,
        version: &semver::Version,
        metadata_json: &[u8],
        archive: &[u8],
        new_revision: bool,
    ) -> Result<PublishReceipt, RegistryApiError> {
        // The `--new-revision` opt-in rides as a query parameter so
        // the route itself stays the immutable-unit address.
        let suffix = if new_revision {
            "?new-revision=true"
        } else {
            ""
        };
        let url = self.package_route(name, version, suffix)?;
        let body = encode_publish_body(metadata_json, archive)?;
        let request = self
            .request("PUT", &url)
            .set("Content-Type", "application/octet-stream");
        let (status, response) = self.send(request.send_bytes(&body), name, version)?;
        let outcome = match status {
            201 => PublishOutcome::Created,
            200 => PublishOutcome::AlreadyPublished,
            status => {
                return Err(RegistryApiError::ServerError {
                    status,
                    detail: None,
                });
            }
        };
        let body = success_body(response);
        Ok(PublishReceipt {
            outcome,
            verification: body.as_ref().and_then(|b| b.verification.clone()),
            revision: body.and_then(|b| b.revision),
        })
    }

    /// `PATCH <api>/api/v1/packages/<scope>/<name>/<version>/yank`
    /// with a JSON `{"yanked": bool}` body.  `true` yanks, `false`
    /// un-yanks; the route is idempotent.  `name` is the full scoped
    /// `<scope>/<name>` string.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` is bare or fails the hosted registry's
    /// name grammar (scope: lowercase/digits/interior `-`; name part:
    /// `[a-z0-9][a-z0-9_-]*`).
    /// Response
    /// statuses map per `docs/remote-registry.md`; a `404` for an
    /// unknown package or version becomes
    /// [`RegistryApiError::NotFound`].
    pub fn set_yanked(
        &self,
        name: &str,
        version: &semver::Version,
        yanked: bool,
    ) -> Result<(), RegistryApiError> {
        let url = self.package_route(name, version, "/yank")?;
        let body = serde_json::json!({ "yanked": yanked }).to_string();
        let request = self
            .request("PATCH", &url)
            .set("Content-Type", "application/json");
        match self.send(request.send_string(&body), name, version)? {
            (200, _) => Ok(()),
            (status, _) => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `DELETE <api>/api/v1/trusted_publishing/tokens`
    /// (`docs/remote-registry.md`, "Revoking the exchanged token"):
    /// the presented token - the one this client was built with -
    /// revokes itself.  `204` is the deletion; anything else,
    /// including the uniform `401` a repeat DELETE answers, surfaces
    /// as an error the caller is expected to tolerate (the token
    /// expires on its own).
    ///
    /// # Errors
    /// Returns [`RegistryApiError::TrustedPublishingRefused`] on the
    /// uniform `401` and the shared protocol mappings otherwise.
    pub fn revoke_trusted_publishing(&self) -> Result<(), RegistryApiError> {
        let url = self.trustpub_route()?;
        let request = self.request("DELETE", &url);
        match self.send_minted(request.call(), trustpub_refused)? {
            (200 | 204, _) => Ok(()),
            (status, _) => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `DELETE <api>/api/v1/sessions/tokens`
    /// (`docs/remote-registry.md`, "Revoking a session token"): the
    /// presented session token - the one this client was built with -
    /// revokes itself.  `204` is the deletion; anything else,
    /// including the uniform `401` a repeat DELETE answers, surfaces
    /// as an error the caller is expected to tolerate (the token
    /// expires on its own).
    ///
    /// # Errors
    /// Returns [`RegistryApiError::SessionRefused`] on the uniform
    /// `401` and the shared protocol mappings otherwise.
    pub fn revoke_session(&self) -> Result<(), RegistryApiError> {
        let url = self.sessions_route()?;
        let request = self.request("DELETE", &url);
        match self.send_minted(request.call(), session_refused)? {
            (200 | 204, _) => Ok(()),
            (status, _) => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `<api>/api/v1/trusted_publishing/tokens`.
    fn trustpub_route(&self) -> Result<url::Url, RegistryApiError> {
        self.base
            .join("api/v1/trusted_publishing/tokens")
            .map_err(|err| RegistryApiError::InvalidApiUrl {
                message: format!("cannot build the trusted-publishing route: {err}"),
            })
    }

    /// `<api>/api/v1/sessions/tokens`.
    fn sessions_route(&self) -> Result<url::Url, RegistryApiError> {
        self.base
            .join("api/v1/sessions/tokens")
            .map_err(|err| RegistryApiError::InvalidApiUrl {
                message: format!("cannot build the sessions route: {err}"),
            })
    }

    /// [`Self::send`] for the minted-token routes (trusted publishing
    /// and login sessions), which have no package to name in
    /// `404`/`409` diagnostics and whose `401` is the registry's
    /// deliberately uniform refusal - advising `cabin login` there
    /// would point at the wrong fix, so `refused` maps it to the
    /// route family's own wording.
    fn send_minted(
        &self,
        result: Result<ureq::Response, ureq::Error>,
        refused: impl FnOnce(String) -> RegistryApiError,
    ) -> Result<(u16, ureq::Response), RegistryApiError> {
        match result {
            Ok(response) => {
                let status = response.status();
                if (300..400).contains(&status) {
                    return Err(RegistryApiError::ServerError {
                        status,
                        detail: None,
                    });
                }
                Ok((status, response))
            }
            Err(ureq::Error::Status(status, response)) => {
                let retry_after_secs = response
                    .header("Retry-After")
                    .and_then(|value| value.trim().parse::<u64>().ok());
                let (detail, code) = match envelope_entry(response) {
                    Some(entry) => (Some(entry.detail), entry.code),
                    None => (None, None),
                };
                Err(match status {
                    401 => refused(self.origin.clone()),
                    429 => RegistryApiError::RateLimited { retry_after_secs },
                    503 if code.as_deref() == Some(OVER_BUDGET_CODE) => {
                        RegistryApiError::RegistryOverBudget { retry_after_secs }
                    }
                    _ => RegistryApiError::ServerError { status, detail },
                })
            }
            Err(ureq::Error::Transport(transport)) => Err(RegistryApiError::Transport {
                message: transport.to_string(),
            }),
        }
    }

    /// `<api>/api/v1/packages/<scope>/<name>/<version><suffix>`.  The
    /// hosted routes have no bare-name form, so a bare name fails
    /// here, before any request; the scoped name is re-validated
    /// against the full `PackageName` grammar plus the registry's
    /// stricter publish grammar for the name part at this URL
    /// boundary (defense in depth, mirroring `cabin-index-http`), so
    /// both segments it embeds are path-safe by construction.
    fn package_route(
        &self,
        name: &str,
        version: &semver::Version,
        suffix: &str,
    ) -> Result<url::Url, RegistryApiError> {
        let safe = cabin_core::PackageName::new(name).is_ok_and(|parsed| {
            parsed.is_scoped() && is_valid_registry_package_name(parsed.base_name())
        });
        if !safe {
            return Err(RegistryApiError::UnsafePackageName {
                name: name.to_owned(),
            });
        }
        // Registry versions are plain upstream versions; the hosted
        // routes reject build metadata, so refuse it before any
        // request (and before a `+` lands un-encoded in a URL).
        if !version.build.is_empty() {
            return Err(RegistryApiError::VersionBuildMetadata {
                version: version.to_string(),
            });
        }
        let relative = format!("api/v1/packages/{name}/{version}{suffix}");
        self.base
            .join(&relative)
            .map_err(|err| RegistryApiError::InvalidApiUrl {
                message: format!("cannot build route `{relative}`: {err}"),
            })
    }

    fn request(&self, method: &str, url: &url::Url) -> ureq::Request {
        let mut request = self.agent.request(method, url.as_str());
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {}", token.expose()));
        }
        request
    }

    /// Map a `ureq` result into either a success status (2xx, with the
    /// response for the caller to interpret) or the typed error for
    /// the shared protocol statuses.
    fn send(
        &self,
        result: Result<ureq::Response, ureq::Error>,
        name: &str,
        version: &semver::Version,
    ) -> Result<(u16, ureq::Response), RegistryApiError> {
        match result {
            Ok(response) => {
                let status = response.status();
                // `.redirects(0)` refuses to follow, but ureq still
                // returns the 3xx as `Ok`; reject it explicitly.
                if (300..400).contains(&status) {
                    return Err(RegistryApiError::ServerError {
                        status,
                        detail: None,
                    });
                }
                Ok((status, response))
            }
            Err(ureq::Error::Status(status, response)) => {
                // `Retry-After` (delta seconds) rides on the 429 and 503
                // refusals; an absent or non-numeric value (an HTTP date,
                // say) degrades to no hint rather than failing the
                // mapping.  Read before the body consumes the response.
                let retry_after_secs = response
                    .header("Retry-After")
                    .and_then(|value| value.trim().parse::<u64>().ok());
                let (detail, code) = match envelope_entry(response) {
                    Some(entry) => (Some(entry.detail), entry.code),
                    None => (None, None),
                };
                Err(match status {
                    400 => RegistryApiError::BadRequest { detail },
                    401 if self.token.is_some() => RegistryApiError::TokenRejected {
                        origin: self.origin.clone(),
                    },
                    401 => RegistryApiError::AuthRequired {
                        origin: self.origin.clone(),
                    },
                    // A 403 whose envelope carries a `quota_*` code is a
                    // per-user quota refusal (`docs/remote-registry.md`,
                    // "Error envelope"), not a scope problem: the server
                    // detail - which embeds the registry's own usage URL -
                    // reaches the user verbatim. The client never derives
                    // a web URL itself.
                    403 if code
                        .as_deref()
                        .is_some_and(|code| code.starts_with("quota_")) =>
                    {
                        RegistryApiError::QuotaExceeded {
                            // The envelope requires `detail`, so a parsed
                            // `code` guarantees one.
                            detail: detail.unwrap_or_default(),
                        }
                    }
                    // A token-authenticated, code-less 403 covers two
                    // distinct server refusals that differ only in
                    // their `detail`: a token permission the user did
                    // not grant, and a scope the token's user is not a
                    // member of.  The detail is surfaced verbatim so
                    // the user fixes the right one; only an
                    // envelope-less response falls back to the generic
                    // token-permission wording.  A tokenless 403 is
                    // neither case (no credential was presented), and
                    // an unknown code falls through to the generic
                    // mapping so its detail still reaches the user.
                    403 if self.token.is_some() && code.is_none() => RegistryApiError::Forbidden {
                        origin: self.origin.clone(),
                        detail,
                    },
                    404 => RegistryApiError::NotFound {
                        name: name.to_owned(),
                        version: version.to_string(),
                    },
                    409 => RegistryApiError::VersionConflict {
                        name: name.to_owned(),
                        version: version.to_string(),
                        detail,
                    },
                    413 => RegistryApiError::ArchiveTooLarge { detail },
                    429 => RegistryApiError::RateLimited { retry_after_secs },
                    // The service-wide budget breaker
                    // (`registry/docs/architecture.md`, "Why 503, not
                    // 402"): an operator-side, temporary refusal, so the
                    // registry answers `503` rather than the `402` it
                    // used before - `503` has explicit `Retry-After`
                    // semantics where `402` has none, and nothing the
                    // caller can pay clears the breaker.  Unlike the `402` it
                    // replaces, `503` is a status Cloudflare's own edge
                    // and runtime emit too, so the code identifies the
                    // breaker; an uncoded `503` stays the generic server
                    // error it was before, rather than blaming a
                    // platform outage on the registry's budget.
                    503 if code.as_deref() == Some(OVER_BUDGET_CODE) => {
                        RegistryApiError::RegistryOverBudget { retry_after_secs }
                    }
                    _ => RegistryApiError::ServerError { status, detail },
                })
            }
            Err(ureq::Error::Transport(transport)) => Err(RegistryApiError::Transport {
                message: transport.to_string(),
            }),
        }
    }
}

impl std::fmt::Debug for RegistryApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Token`'s own `Debug` redacts; keep the origin visible.
        f.debug_struct("RegistryApi")
            .field("origin", &self.origin)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// Exchange a GitHub Actions OIDC JWT for a short-lived registry
/// token: `PUT <api>/api/v1/trusted_publishing/tokens` with
/// `{"jwt": ...}` as the body (`docs/remote-registry.md`, "Exchanging
/// an Actions OIDC token").  The one mutation route that carries no
/// `Authorization` header - the JWT is the credential - so the client
/// is built tokenless on purpose.
///
/// # Errors
/// Returns [`RegistryApiError::TrustedPublishingRefused`] on the
/// registry's deliberately uniform `401`, and
/// [`RegistryApiError::ServerError`] when a success response carries
/// no parseable minted token; URL hygiene and the shared protocol
/// mappings otherwise.
pub fn exchange_trusted_publishing(api_url: &str, jwt: &str) -> Result<Token, RegistryApiError> {
    /// Serde shape of the exchange success body; `expires_at` is
    /// deliberately ignored - the token's server-side lifetime needs
    /// no client bookkeeping.
    #[derive(Deserialize)]
    struct ExchangeSuccessBody {
        token: String,
    }

    let api = RegistryApi::new(api_url, None)?;
    let url = api.trustpub_route()?;
    let body = serde_json::json!({ "jwt": jwt }).to_string();
    let request = api
        .request("PUT", &url)
        .set("Content-Type", "application/json");
    let (status, response) = api.send_minted(request.send_string(&body), trustpub_refused)?;
    if status != 200 {
        return Err(RegistryApiError::ServerError {
            status,
            detail: None,
        });
    }
    let mut body = Vec::new();
    let unparsable = || RegistryApiError::ServerError {
        status: 200,
        detail: Some("the trusted-publishing exchange answered without a usable token".to_owned()),
    };
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64)
        .read_to_end(&mut body)
        .map_err(|_| unparsable())?;
    let parsed: ExchangeSuccessBody = serde_json::from_slice(&body).map_err(|_| unparsable())?;
    Token::parse(&parsed.token).map_err(|_| unparsable())
}

/// The trusted-publishing routes' uniform `401`.
fn trustpub_refused(origin: String) -> RegistryApiError {
    RegistryApiError::TrustedPublishingRefused { origin }
}

/// The session routes' uniform `401`.
fn session_refused(origin: String) -> RegistryApiError {
    RegistryApiError::SessionRefused { origin }
}

/// A minted login session: the registry token plus the mint
/// response's `expires_at`, verbatim (terminal-safe).  `Token`'s own
/// `Debug` redacts, so the derive cannot leak the secret.
#[derive(Debug)]
pub struct SessionGrant {
    pub token: Token,
    pub expires_at: String,
}

/// Exchange a GitHub access token for a login-session registry token:
/// `PUT <api>/api/v1/sessions/tokens` with `{"github_token": ...}` as
/// the body (`docs/remote-registry.md`, "Minting a session token").
/// Like the trusted-publishing exchange the route carries no
/// `Authorization` header - the GitHub token in the body is the
/// credential - so the client is built tokenless on purpose, and the
/// GitHub token lives only on this call's stack.
///
/// # Errors
/// Returns [`RegistryApiError::SessionRefused`] on the registry's
/// deliberately uniform `401`, and [`RegistryApiError::ServerError`]
/// when a success response carries no parseable minted token; URL
/// hygiene and the shared protocol mappings otherwise.
pub fn exchange_login_session(
    api_url: &str,
    github_token: &str,
) -> Result<SessionGrant, RegistryApiError> {
    /// Serde shape of the mint success body; unknown fields are the
    /// registry's business.
    #[derive(Deserialize)]
    struct MintSuccessBody {
        token: String,
        expires_at: String,
    }

    let api = RegistryApi::new(api_url, None)?;
    let url = api.sessions_route()?;
    let body = serde_json::json!({ "github_token": github_token }).to_string();
    let request = api
        .request("PUT", &url)
        .set("Content-Type", "application/json");
    // The mint's error contract is the uniform 401 (plus the shared
    // breaker/rate-limit answers, which carry no free text).  Any
    // other error detail is registry-controlled prose on the one call
    // whose request body held a live GitHub token - a registry could
    // reflect it - so it is dropped, never rendered.
    let (status, response) = api
        .send_minted(request.send_string(&body), session_refused)
        .map_err(|err| match err {
            RegistryApiError::ServerError { status, .. } => RegistryApiError::ServerError {
                status,
                detail: None,
            },
            other => other,
        })?;
    if status != 200 {
        return Err(RegistryApiError::ServerError {
            status,
            detail: None,
        });
    }
    let mut body = Vec::new();
    let unparsable = || RegistryApiError::ServerError {
        status: 200,
        detail: Some("the login-session mint answered without a usable token".to_owned()),
    };
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64)
        .read_to_end(&mut body)
        .map_err(|_| unparsable())?;
    let parsed: MintSuccessBody = serde_json::from_slice(&body).map_err(|_| unparsable())?;
    // `expires_at` gets stored on disk and echoed in the login
    // confirmation, so it must actually be a timestamp - not an
    // arbitrary registry-chosen string (which could even be the
    // GitHub token, reflected).  The accepted shape is deliberately
    // UTC-only RFC 3339 (`Z` or `+00:00`, `humantime`'s parse; the
    // hosted registry mints the millisecond `Z` form) - a non-UTC
    // offset refuses.  Validation subsumes control-char escaping: a
    // stamp that parses is constrained ASCII.
    match humantime::parse_rfc3339(&parsed.expires_at) {
        Err(_) => {
            return Err(RegistryApiError::ServerError {
                status: 200,
                detail: Some("the login-session mint answered without a usable expiry".to_owned()),
            });
        }
        // An already-expired grant could be stored but never used -
        // every credential lookup would immediately withhold it - so
        // a mint answering one fails the login instead of reporting
        // a success the next command contradicts.
        Ok(expiry) if expiry <= std::time::SystemTime::now() => {
            return Err(RegistryApiError::ServerError {
                status: 200,
                detail: Some(
                    "the login-session mint answered an already-expired session".to_owned(),
                ),
            });
        }
        Ok(_) => {}
    }
    // The mint's grant is specifically a session token: any other
    // Cabin credential shape (`cabin_tp_...`) would carry the wrong
    // scopes, and the session revocation route deletes only
    // session-kind rows, so `cabin logout` could never retire it.
    if !parsed.token.starts_with("cabin_ses_") {
        return Err(unparsable());
    }
    Ok(SessionGrant {
        token: Token::parse(&parsed.token).map_err(|_| unparsable())?,
        expires_at: parsed.expires_at,
    })
}

/// A pending GitHub OAuth device-flow authorization
/// (`POST <github>/login/device/code`): what the user must do
/// (`user_code` at `verification_uri`) and how to poll for the
/// outcome.  Both user-facing strings arrive terminal-safe.
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds to wait between polls, floored at 1 so a degenerate
    /// answer cannot turn the poll loop into a hammer.
    pub interval_secs: u64,
    /// Seconds until the device code itself expires.
    pub expires_in_secs: u64,
}

/// Start the GitHub OAuth device flow: request a device + user code
/// pair for `client_id` with an empty scope.  `github_url` is the
/// OAuth host (`https://github.com` in production; tests point it at
/// a loopback mock, the only cleartext host allowed).
///
/// # Errors
/// Returns [`RegistryApiError::GithubDeviceFlow`] for every failure
/// shape: a cleartext non-loopback URL, transport errors, a non-200
/// answer, or an unparsable body.
pub fn request_device_authorization(
    github_url: &str,
    client_id: &str,
) -> Result<DeviceAuthorization, RegistryApiError> {
    /// Serde shape of the device-code response; unknown fields are
    /// GitHub's business.
    #[derive(Deserialize)]
    struct DeviceCodeResponse {
        device_code: String,
        user_code: String,
        verification_uri: String,
        expires_in: u64,
        #[serde(default)]
        interval: u64,
    }

    let agent = github_oauth_agent(github_url)?;
    let url = format!("{}/login/device/code", github_url.trim_end_matches('/'));
    let response = agent
        .request("POST", &url)
        .set("Accept", "application/json")
        .send_form(&[("client_id", client_id), ("scope", "")])
        .map_err(|err| RegistryApiError::GithubDeviceFlow {
            message: match err {
                ureq::Error::Status(status, _) => {
                    format!("GitHub's device-code endpoint answered {status}")
                }
                ureq::Error::Transport(transport) => transport.to_string(),
            },
        })?;
    let parsed: DeviceCodeResponse = serde_json::from_reader(
        response.into_reader().take(MAX_ERROR_BODY_BYTES as u64),
    )
    .map_err(|_| RegistryApiError::GithubDeviceFlow {
        message: "GitHub's device-code response was not the expected JSON".to_owned(),
    })?;
    Ok(DeviceAuthorization {
        device_code: parsed.device_code,
        user_code: escape_control_chars(&parsed.user_code),
        verification_uri: escape_control_chars(&parsed.verification_uri),
        interval_secs: parsed.interval.max(1),
        expires_in_secs: parsed.expires_in,
    })
}

/// Poll GitHub's device-flow token endpoint
/// (`POST <github>/login/oauth/access_token`, grant type
/// `urn:ietf:params:oauth:grant-type:device_code`) until the user
/// approves or the flow ends: `authorization_pending` keeps polling
/// at the current interval, `slow_down` adds five seconds to it, and
/// `expired_token` / `access_denied` end the flow with an actionable
/// error.  `sleep` runs before every poll (tests inject a recorder);
/// the loop gives up before the next poll once the slept time - or
/// the real elapsed time, since slow responses consume lifetime no
/// sleep accounts for - exceeds the device code's own lifetime, so a
/// server that answers pending forever (or slowly) cannot hang the
/// login.  A token the server does grant is still accepted: GitHub
/// is the authority on the code's real lifetime.  The slept-time leg
/// keeps the bound deterministic under the tests' no-op sleep.
///
/// The returned GitHub access token is a secret: callers use it for
/// exactly one [`exchange_login_session`] call and drop it - never
/// stored, never logged.  GitHub's response may also carry
/// `refresh_token` fields (the OAuth app has token expiration
/// enabled); the tolerant parse discards them unread, since Cabin
/// re-runs the device flow instead of refreshing.
///
/// # Errors
/// Returns [`RegistryApiError::GithubDeviceFlow`] naming the terminal
/// outcome (expired, denied, transport failure, or an unexpected
/// answer).
pub fn poll_device_token(
    github_url: &str,
    client_id: &str,
    authorization: &DeviceAuthorization,
    mut sleep: impl FnMut(Duration),
) -> Result<String, RegistryApiError> {
    /// Serde shape of the token-poll response: exactly one of the
    /// two fields answers, and everything else (`refresh_token`
    /// included) is discarded unread.
    #[derive(Deserialize)]
    struct DeviceTokenResponse {
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    let agent = github_oauth_agent(github_url)?;
    let url = format!(
        "{}/login/oauth/access_token",
        github_url.trim_end_matches('/')
    );
    let mut interval_secs = authorization.interval_secs.max(1);
    let mut slept_secs = 0u64;
    let started = std::time::Instant::now();
    loop {
        sleep(Duration::from_secs(interval_secs));
        slept_secs += interval_secs;
        // Give up once the device code's own lifetime has passed -
        // measured both in requested sleep (deterministic under the
        // tests' no-op sleep) and in real elapsed time (slow
        // responses consume lifetime no sleep accounts for) - before
        // spending another poll on a code the server would refuse.
        if slept_secs > authorization.expires_in_secs
            || started.elapsed().as_secs() > authorization.expires_in_secs
        {
            return Err(RegistryApiError::GithubDeviceFlow {
                message: "the device code expired before the login was approved; run `cabin \
                          login` again"
                    .to_owned(),
            });
        }
        // GitHub signals the flow's own outcomes in the body under
        // varying HTTP statuses, so a status error still carries the
        // answer; only transport failures have no body to read.
        let response = match agent
            .request("POST", &url)
            .set("Accept", "application/json")
            .send_form(&[
                ("client_id", client_id),
                ("device_code", &authorization.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]) {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return Err(RegistryApiError::GithubDeviceFlow {
                    message: transport.to_string(),
                });
            }
        };
        let parsed: DeviceTokenResponse =
            serde_json::from_reader(response.into_reader().take(MAX_ERROR_BODY_BYTES as u64))
                .map_err(|_| RegistryApiError::GithubDeviceFlow {
                    message: "GitHub's token response was not the expected JSON".to_owned(),
                })?;
        match (parsed.access_token, parsed.error.as_deref()) {
            // A token the server grants is accepted even when the
            // local deadline lapsed mid-poll: GitHub owns the device
            // code's real lifetime (an expired code answers
            // `expired_token`), and the local bound exists only so a
            // non-conforming server cannot hold the loop open.
            (Some(token), None) => return Ok(token),
            (_, Some("authorization_pending")) => {}
            (_, Some("slow_down")) => interval_secs += 5,
            (_, Some("expired_token")) => {
                return Err(RegistryApiError::GithubDeviceFlow {
                    message: "the device code expired before the login was approved; run `cabin \
                              login` again"
                        .to_owned(),
                });
            }
            (_, Some("access_denied")) => {
                return Err(RegistryApiError::GithubDeviceFlow {
                    message: "the login request was denied on github.com".to_owned(),
                });
            }
            (_, Some(error)) => {
                return Err(RegistryApiError::GithubDeviceFlow {
                    message: format!("GitHub answered `{}`", escape_control_chars(error)),
                });
            }
            (None, None) => {
                return Err(RegistryApiError::GithubDeviceFlow {
                    message: "GitHub's token response carried neither a token nor an error"
                        .to_owned(),
                });
            }
        }
    }
}

/// Shared agent + URL hygiene for the GitHub OAuth calls: the device
/// flow carries a secret in its responses, so cleartext is refused
/// beyond loopback, mirroring the registry-API rule.
fn github_oauth_agent(github_url: &str) -> Result<ureq::Agent, RegistryApiError> {
    if !github_url.starts_with("https://") && !cabin_credentials::url_is_loopback(github_url) {
        return Err(RegistryApiError::GithubDeviceFlow {
            message: "the GitHub OAuth endpoint is not an https URL".to_owned(),
        });
    }
    Ok(ureq::AgentBuilder::new()
        .timeout(DEFAULT_TIMEOUT)
        .redirects(0)
        .build())
}

/// Fetch the workflow run's OIDC JWT from the GitHub Actions runner:
/// `GET <ACTIONS_ID_TOKEN_REQUEST_URL>&audience=<audience>` with the
/// runner-supplied request token as the bearer, returning the
/// response's `value` field.  The runner URL always carries a query
/// string already, so the audience is appended with `&`.
///
/// # Errors
/// Returns [`RegistryApiError::GithubOidc`] for every failure shape -
/// a cleartext non-loopback URL, transport errors, a non-200 answer
/// (typically the workflow missing `permissions: id-token: write`
/// scope for the requested audience), or a body without `value`.
pub fn fetch_github_actions_jwt(
    request_url: &str,
    request_token: &str,
    audience: &str,
) -> Result<String, RegistryApiError> {
    /// Serde shape of the runner's OIDC response.
    #[derive(Deserialize)]
    struct OidcResponse {
        value: String,
    }

    // The bearer request token must not travel in cleartext; loopback
    // is carved out for tests, mirroring the registry-API rule.
    if !request_url.starts_with("https://") && !cabin_credentials::url_is_loopback(request_url) {
        return Err(RegistryApiError::GithubOidc {
            message: "the runner's OIDC endpoint is not an https URL".to_owned(),
        });
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(DEFAULT_TIMEOUT)
        .redirects(0)
        .build();
    let url = format!("{request_url}&audience={audience}");
    // The runner's token service is known-flaky under load: GitHub's
    // own OIDC client retries this exact GET on 5xx.  A short bounded
    // retry (idempotent request), never on 4xx - those are the
    // misconfiguration answers the caller must surface unchanged.
    let mut attempt = 0;
    let response = loop {
        attempt += 1;
        let result = agent
            .request("GET", &url)
            .set("Authorization", &format!("Bearer {request_token}"))
            .call();
        let retryable = match &result {
            Ok(response) => response.status() >= 500,
            Err(ureq::Error::Status(status, _)) => *status >= 500,
            Err(ureq::Error::Transport(_)) => true,
        };
        if retryable && attempt < 3 {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        break result.map_err(|err| RegistryApiError::GithubOidc {
            message: match err {
                ureq::Error::Status(status, _) => {
                    format!("the runner's OIDC endpoint answered {status}")
                }
                ureq::Error::Transport(transport) => transport.to_string(),
            },
        })?;
    };
    if response.status() != 200 {
        return Err(RegistryApiError::GithubOidc {
            message: format!("the runner's OIDC endpoint answered {}", response.status()),
        });
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64)
        .read_to_end(&mut body)
        .map_err(|err| RegistryApiError::GithubOidc {
            message: format!("cannot read the runner's OIDC response: {err}"),
        })?;
    let parsed: OidcResponse =
        serde_json::from_slice(&body).map_err(|_| RegistryApiError::GithubOidc {
            message: "the runner's OIDC response carried no token value".to_owned(),
        })?;
    Ok(parsed.value)
}

/// Mirror of the hosted registry's package-name grammar
/// (`registry/src/routes.rs`, `is_valid_name`):
/// `^[a-z0-9][a-z0-9_-]*$`.  `PackageName`'s own grammar is looser
/// (uppercase and `.` are legal in local-only names), so without this
/// check a name the registry refuses would fail publish only after
/// staging and network work - and 404 a yank misleadingly.
fn is_valid_registry_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Encode the publish request body:
/// `[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`.
///
/// # Errors
/// Returns [`RegistryApiError::FrameTooLarge`] when either payload
/// does not fit the `u32` length prefix.
pub fn encode_publish_body(
    metadata_json: &[u8],
    archive: &[u8],
) -> Result<Vec<u8>, RegistryApiError> {
    let metadata_len = u32::try_from(metadata_json.len())
        .map_err(|_| RegistryApiError::FrameTooLarge { part: "metadata" })?;
    let archive_len = u32::try_from(archive.len())
        .map_err(|_| RegistryApiError::FrameTooLarge { part: "archive" })?;
    let mut body = Vec::with_capacity(8 + metadata_json.len() + archive.len());
    body.extend_from_slice(&metadata_len.to_le_bytes());
    body.extend_from_slice(metadata_json);
    body.extend_from_slice(&archive_len.to_le_bytes());
    body.extend_from_slice(archive);
    Ok(body)
}

/// Serde shape of the protocol's error envelope:
/// `{"errors":[{"detail":"...","code":"..."}]}`; `code` is the optional
/// machine-readable refusal code quota and budget errors carry.
#[derive(Deserialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorEntry>,
}

#[derive(Deserialize)]
struct ErrorEntry {
    detail: String,
    #[serde(default)]
    code: Option<String>,
}

/// Serde shape of a publish success body's optional `"verification"`
/// field; every other field is ignored on purpose - a registry without
/// the verification lifecycle simply omits it.
#[derive(Deserialize)]
struct PublishSuccessBody {
    #[serde(default)]
    verification: Option<String>,
    #[serde(default)]
    revision: Option<String>,
}

/// Read a publish success body (capped like the error envelope)
/// tolerantly: a missing, oversized, or malformed body yields `None`
/// rather than an error.
fn success_body(response: ureq::Response) -> Option<PublishSuccessBody> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64)
        .read_to_end(&mut body)
        .ok()?;
    serde_json::from_slice::<PublishSuccessBody>(&body).ok()
}

/// Read a non-2xx response body (capped) and extract the first error
/// envelope entry.  A malformed or missing envelope yields `None`, so
/// the caller's message degrades to the raw status.
///
/// The cap is a rejection, not a truncation: reading one byte past it and
/// refusing what overflows is what stops an oversized body whose first
/// 64 KiB happens to parse - a coded envelope followed by padding - from
/// being accepted as the envelope it is not.  The `code` this returns
/// decides the `503` mapping, so a near-miss here is a wrong diagnosis,
/// not just a wrong message.
fn envelope_entry(response: ureq::Response) -> Option<ErrorEntry> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() > MAX_ERROR_BODY_BYTES {
        return None;
    }
    let envelope: ErrorEnvelope = serde_json::from_slice(&body).ok()?;
    let mut entry = envelope.errors.into_iter().next()?;
    entry.detail = escape_control_chars(&entry.detail);
    Some(entry)
}

/// The publish `409` explanation: the server's envelope detail
/// verbatim when present - the refusal has several distinct causes
/// (missing `--new-revision`, a resolver-metadata change, a
/// revision-id collision, a racing conflict), and only the server
/// knows which fired - with the opt-in guidance as the fallback for
/// an envelope-less response.
fn version_conflict_message(name: &str, version: &str, detail: Option<&String>) -> String {
    match detail {
        Some(detail) => format!("cannot publish `{name} {version}`: {detail}"),
        None => format!(
            "`{name} {version}` is already published with different bytes; published revisions \
             are immutable - pass `--new-revision` to publish the changed bytes as a new \
             packaging revision of this version, or bump the version"
        ),
    }
}

/// Append the server's envelope `detail` to a base message when one
/// was present.
fn with_detail(base: String, detail: Option<&String>) -> String {
    match detail {
        Some(detail) => format!("{base}: {detail}"),
        None => base,
    }
}

/// The token-authenticated 403 message: the server's terminal-safe envelope
/// `detail` when present (it distinguishes a missing token
/// permission from missing scope membership), else the generic
/// token-permission wording for registries that answer without an
/// envelope.
fn forbidden_message(origin: &str, detail: Option<&String>) -> String {
    let reason = detail.map_or(
        "the stored token does not have the required scope",
        String::as_str,
    );
    format!("registry API `{origin}` refused the request: {reason}")
}

/// Append the retry hint: the server's `Retry-After` seconds when the
/// response carried a usable one, a plain "try again later" otherwise.
fn with_retry(base: &str, retry_after_secs: Option<u64>) -> String {
    match retry_after_secs {
        Some(1) => format!("{base}; try again in 1 second"),
        Some(secs) => format!("{base}; try again in {secs} seconds"),
        None => format!("{base}; try again later"),
    }
}

/// Errors produced by the registry API client.  No variant ever
/// embeds token bytes.
#[derive(Debug, Error)]
pub enum RegistryApiError {
    #[error("invalid registry API URL: {message}")]
    InvalidApiUrl { message: String },

    #[error(
        "refusing to send requests to registry API `{origin}` over plain `http`: bearer tokens \
         are never sent in cleartext except to loopback hosts; use an `https` API URL"
    )]
    CleartextApiUrl { origin: String },

    #[error(
        "package name `{name}` cannot be used on remote registry routes; registry packages are named `<scope>/<name>` (exactly one `/`), where the scope is lowercase ASCII letters, digits, and interior `-` (at most 39 characters) and the name part matches `[a-z0-9][a-z0-9_-]*`"
    )]
    UnsafePackageName { name: String },

    #[error("cannot frame the publish request: the {part} exceeds the u32 length prefix")]
    FrameTooLarge { part: &'static str },

    #[error("{}", with_detail("registry rejected the request (status 400)".to_owned(), .detail.as_ref()))]
    BadRequest { detail: Option<String> },

    #[error(
        "authentication required by registry API `{origin}`; run `cabin login --index-url <URL>` \
         to store a token for this registry"
    )]
    AuthRequired { origin: String },

    #[error(
        "registry API `{origin}` rejected the stored token (revoked or expired); re-run `cabin \
         login --index-url <URL>` for this registry"
    )]
    TokenRejected { origin: String },

    #[error("{}", forbidden_message(.origin, .detail.as_ref()))]
    Forbidden {
        origin: String,
        detail: Option<String>,
    },

    #[error("{detail}")]
    QuotaExceeded { detail: String },

    #[error("{}", with_retry(
        "the registry is temporarily not accepting publishes (over its free budget)",
        *.retry_after_secs,
    ))]
    RegistryOverBudget { retry_after_secs: Option<u64> },

    #[error("{}", with_retry("the registry rate limited this request", *.retry_after_secs))]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("{}", with_detail(
        "the package archive is too large for this registry".to_owned(),
        .detail.as_ref(),
    ))]
    ArchiveTooLarge { detail: Option<String> },

    #[error("`{name}@{version}` is not published on this registry")]
    NotFound { name: String, version: String },

    #[error("{}", version_conflict_message(name, version, .detail.as_ref()))]
    VersionConflict {
        name: String,
        version: String,
        detail: Option<String>,
    },

    #[error(
        "version `{version}` carries build metadata; registry versions are plain upstream versions - drop the `+...` suffix"
    )]
    VersionBuildMetadata { version: String },

    #[error("{}", with_detail(format!("registry API request failed: server returned {status}"), .detail.as_ref()))]
    ServerError { status: u16, detail: Option<String> },

    #[error("registry API transport error: {message}")]
    Transport { message: String },

    #[error("cannot obtain the workflow's GitHub Actions OIDC token: {message}")]
    GithubOidc { message: String },

    #[error("cannot complete the GitHub device login: {message}")]
    GithubDeviceFlow { message: String },

    #[error(
        "registry API `{origin}` refused the session request; the refusal is deliberately \
         uniform - for a login, check that the GitHub account has signed in to the registry and \
         is admitted; for a revocation, the session was likely already expired or revoked"
    )]
    SessionRefused { origin: String },

    #[error(
        "registry API `{origin}` refused the trusted-publishing request; the refusal is \
         deliberately uniform, so check that this repository, workflow file, and ref are \
         registered for trusted publishing on the registry and that the run's OIDC token was not \
         already exchanged"
    )]
    TrustedPublishingRefused { origin: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    const TEST_TOKEN: &str = "cabin_apiTestToken12";

    fn token() -> Token {
        Token::parse(TEST_TOKEN).unwrap()
    }

    fn version(raw: &str) -> semver::Version {
        semver::Version::parse(raw).unwrap()
    }

    /// Decode a framed publish body back into (metadata, archive).
    /// Test-side inverse of [`encode_publish_body`]; asserts the
    /// frame is exactly consumed.
    fn decode_publish_body(body: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let metadata_len = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
        let metadata = body[4..4 + metadata_len].to_vec();
        let rest = &body[4 + metadata_len..];
        let archive_len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
        let archive = rest[4..4 + archive_len].to_vec();
        assert_eq!(
            body.len(),
            8 + metadata_len + archive_len,
            "frame must be exactly consumed"
        );
        (metadata, archive)
    }

    /// The retry hint degrades to "later" without a usable
    /// `Retry-After` and pluralizes correctly ("in 1 second", not
    /// "1 seconds" - live-verified wording).
    #[test]
    fn retry_hints_degrade_and_pluralize() {
        assert_eq!(with_retry("x", None), "x; try again later");
        assert_eq!(with_retry("x", Some(1)), "x; try again in 1 second");
        assert_eq!(with_retry("x", Some(2)), "x; try again in 2 seconds");
    }

    #[test]
    fn publish_body_round_trips_through_the_decoder() {
        let metadata = br#"{"schema":1,"name":"fmtlib/fmt"}"#;
        let archive = [0x1fu8, 0x8b, 0x08, 0x00, 0xff];
        let body = encode_publish_body(metadata, &archive).unwrap();
        assert_eq!(
            &body[0..4],
            &u32::try_from(metadata.len()).unwrap().to_le_bytes()
        );
        let (decoded_metadata, decoded_archive) = decode_publish_body(&body);
        assert_eq!(decoded_metadata, metadata);
        assert_eq!(decoded_archive, archive);

        // Empty payloads still frame correctly.
        let empty = encode_publish_body(b"", b"").unwrap();
        assert_eq!(empty, vec![0u8; 8]);
    }

    #[test]
    fn new_rejects_invalid_and_cleartext_api_urls() {
        for api in [
            "ftp://registry.example.com",
            "https://user:pw@registry.example.com",
            "not a url",
        ] {
            let err = RegistryApi::new(api, None).unwrap_err();
            let message = err.to_string();
            assert!(
                matches!(err, RegistryApiError::InvalidApiUrl { .. }),
                "{api}: {err:?}"
            );
            assert!(
                !message.contains("user:pw"),
                "credentials leaked: {message}"
            );
        }
        let err = RegistryApi::new("http://registry.example.com", Some(token())).unwrap_err();
        assert!(
            matches!(err, RegistryApiError::CleartextApiUrl { .. }),
            "{err:?}"
        );
        // Loopback http is the documented local-testing exception.
        RegistryApi::new("http://127.0.0.1:8080", Some(token())).unwrap();
        RegistryApi::new("http://localhost:8080/base", None).unwrap();
    }

    #[test]
    fn unsafe_and_bare_package_names_never_reach_the_wire() {
        // No server bound: an attempted request would surface as a
        // transport error, so getting `UnsafePackageName` proves the
        // gate fires first.  Bare names are rejected alongside unsafe
        // segments: the hosted routes have no bare-name form.
        let api = RegistryApi::new("http://127.0.0.1:9", Some(token())).unwrap();
        for name in [
            "fmt",
            "../evil",
            ".hidden",
            "-flag",
            "acme/../evil",
            "../evil/fmt",
            "acme/.hidden",
            "acme/fmt/extra",
            "acme//fmt",
            "/fmt",
            "acme/",
            // The full grammar applies, not just path safety: a scope
            // is lowercase-only, and the name part follows the
            // registry's publish grammar (`[a-z0-9][a-z0-9_-]*`), so
            // uppercase or `.`-bearing local-only names are refused
            // before any request.
            "ACME/fmt",
            "acme/Foo",
            "acme/foo.bar",
            "acme/_foo",
        ] {
            let err = api
                .publish(name, &version("1.0.0"), b"{}", b"", false)
                .unwrap_err();
            assert!(
                matches!(err, RegistryApiError::UnsafePackageName { .. }),
                "{name}: {err:?}"
            );
            let err = api.set_yanked(name, &version("1.0.0"), true).unwrap_err();
            assert!(
                matches!(err, RegistryApiError::UnsafePackageName { .. }),
                "{name}: {err:?}"
            );
        }
    }

    /// Registry versions are plain upstream versions: a `+`-bearing
    /// version is refused before any request on both mutation routes.
    #[test]
    fn build_metadata_versions_never_reach_the_wire() {
        let api = RegistryApi::new("http://127.0.0.1:9", Some(token())).unwrap();
        let versioned = version("1.3.1+cabin.1");
        let err = api
            .publish("fmtlib/fmt", &versioned, b"{}", b"", false)
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::VersionBuildMetadata { .. }),
            "{err:?}"
        );
        let err = api.set_yanked("fmtlib/fmt", &versioned, true).unwrap_err();
        assert!(
            matches!(err, RegistryApiError::VersionBuildMetadata { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let api = RegistryApi::new("https://registry.example.com", Some(token())).unwrap();
        let rendered = format!("{api:?}");
        assert!(!rendered.contains("apiTestToken"), "leaked: {rendered}");
        assert!(rendered.contains("https://registry.example.com"));
    }

    // -----------------------------------------------------------------
    // Mock registry: wire-level assertions per response status
    // -----------------------------------------------------------------

    /// One captured request: everything the protocol tests assert on.
    struct Captured {
        method: String,
        path: String,
        authorization: Option<String>,
        body: Vec<u8>,
    }

    /// Mock registry API server answering every request with a fixed
    /// status + body, capturing requests into a channel.
    struct MockApi {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
        captured: mpsc::Receiver<Captured>,
    }

    impl MockApi {
        fn respond_with(status: u16, body: impl Into<String>) -> Self {
            Self::respond_with_headers(status, body, &[])
        }

        fn respond_with_headers(
            status: u16,
            body: impl Into<String>,
            headers: &[(&str, &str)],
        ) -> Self {
            Self::respond_with_script(vec![(status, body.into())], headers)
        }

        /// Mock whose responses follow `script` in request order, the
        /// last entry repeating once the script is exhausted - how the
        /// device-flow poll tests walk pending -> `slow_down` -> success.
        fn respond_with_script(script: Vec<(u16, String)>, headers: &[(&str, &str)]) -> Self {
            assert!(!script.is_empty(), "a mock needs at least one response");
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let (sender, captured) = mpsc::channel();
            let server_for_thread = Arc::clone(&server);
            let response_headers: Vec<tiny_http::Header> = headers
                .iter()
                .map(|(name, value)| {
                    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                        .expect("valid test header")
                })
                .collect();
            let thread = std::thread::spawn(move || {
                let mut answered = 0usize;
                while let Ok(mut req) = server_for_thread.recv() {
                    let mut body_bytes = Vec::new();
                    let _ = req.as_reader().read_to_end(&mut body_bytes);
                    let _ = sender.send(Captured {
                        method: req.method().as_str().to_owned(),
                        path: req.url().to_owned(),
                        authorization: req
                            .headers()
                            .iter()
                            .find(|h| h.field.equiv("Authorization"))
                            .map(|h| h.value.to_string()),
                        body: body_bytes,
                    });
                    let (status, body) = &script[answered.min(script.len() - 1)];
                    answered += 1;
                    let mut response =
                        tiny_http::Response::from_string(body.clone()).with_status_code(*status);
                    for header in &response_headers {
                        response.add_header(header.clone());
                    }
                    let _ = req.respond(response);
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
                captured,
            }
        }

        fn client(&self, token: Option<Token>) -> RegistryApi {
            RegistryApi::new(&self.url, token).unwrap()
        }

        fn captured(&self) -> Captured {
            self.captured
                .recv_timeout(Duration::from_secs(5))
                .expect("a request should have reached the mock registry")
        }
    }

    impl Drop for MockApi {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    /// The 201 path: outcome, route, method, bearer header, and the
    /// exact frame bytes on the wire.
    #[test]
    fn publish_created_sends_the_framed_body_and_bearer_token() {
        let mock = MockApi::respond_with(201, r#"{"ok":true}"#);
        let metadata = br#"{"schema":1,"name":"fmtlib/fmt","version":"10.2.1"}"#;
        let archive = b"\x1f\x8b\x08\x00fake-gzip-bytes";

        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), metadata, archive, false)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        // No "verification" field: a registry without the lifecycle.
        assert_eq!(receipt.verification, None);

        let captured = mock.captured();
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path, "/api/v1/packages/fmtlib/fmt/10.2.1");
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str())
        );
        assert_eq!(
            &captured.body[0..4],
            &u32::try_from(metadata.len()).unwrap().to_le_bytes()
        );
        let (decoded_metadata, decoded_archive) = decode_publish_body(&captured.body);
        assert_eq!(decoded_metadata, metadata);
        assert_eq!(decoded_archive, archive);
    }

    /// The `--new-revision` opt-in travels as a query parameter, and
    /// the success body's `"revision"` field is read tolerantly.
    #[test]
    fn publish_new_revision_sends_the_query_parameter() {
        let mock = MockApi::respond_with(
            201,
            r#"{"ok":true,"revision":"0011223344556677","verification":"pending"}"#,
        );
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", true)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        assert_eq!(receipt.revision.as_deref(), Some("0011223344556677"));
        let captured = mock.captured();
        assert_eq!(
            captured.path,
            "/api/v1/packages/fmtlib/fmt/10.2.1?new-revision=true"
        );
    }

    /// The 200 path: byte-identical re-publish reports the no-op.
    #[test]
    fn publish_maps_200_to_already_published() {
        let mock = MockApi::respond_with(200, r#"{"ok":true,"no_op":true}"#);
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::AlreadyPublished);
        assert_eq!(receipt.verification, None);
    }

    /// The optional `"verification"` field is read tolerantly: present
    /// on either success status it is surfaced verbatim, and a body
    /// that is not the expected JSON degrades to `None` instead of
    /// failing the publish.
    #[test]
    fn publish_reads_the_verification_field_tolerantly() {
        let mock = MockApi::respond_with(
            201,
            r#"{"ok":true,"name":"fmtlib/fmt","version":"10.2.1","checksum":"sha256:aa","verification":"pending"}"#,
        );
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        assert_eq!(receipt.verification.as_deref(), Some("pending"));

        let mock =
            MockApi::respond_with(200, r#"{"ok":true,"no_op":true,"verification":"verified"}"#);
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::AlreadyPublished);
        assert_eq!(receipt.verification.as_deref(), Some("verified"));

        // A body that is not the expected JSON shape never fails the
        // publish: the field just reads as absent.
        let mock = MockApi::respond_with(201, "not json at all");
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        assert_eq!(receipt.verification, None);
    }

    /// 409: the server's envelope detail is authoritative - the
    /// refusal has several distinct causes and only the server knows
    /// which fired, so the client must not paper over (say) an
    /// invariance refusal with `--new-revision` advice.
    #[test]
    fn publish_maps_409_to_version_conflict() {
        let mock = MockApi::respond_with(
            409,
            r#"{"errors":[{"detail":"a packaging revision must not change dependencies"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::VersionConflict {
                name,
                version,
                detail,
            } => {
                assert_eq!(name, "fmtlib/fmt");
                assert_eq!(version, "10.2.1");
                assert_eq!(
                    detail.as_deref(),
                    Some("a packaging revision must not change dependencies")
                );
            }
            other => panic!("expected VersionConflict, got {other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains("must not change dependencies"),
            "{message}"
        );
        assert!(
            !message.contains("--new-revision"),
            "the fallback advice must not override the server's reason: {message}"
        );
    }

    /// 409 without an envelope: the opt-in guidance fallback.
    #[test]
    fn publish_maps_an_envelope_less_409_to_the_opt_in_guidance() {
        let mock = MockApi::respond_with(409, "conflict");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("different bytes"), "{message}");
        assert!(message.contains("immutable"), "{message}");
        assert!(message.contains("--new-revision"), "{message}");
    }

    /// 401 without a token asks for a login; 401 despite one reports
    /// the token as rejected.  Neither leaks token bytes.
    #[test]
    fn publish_maps_401_by_whether_a_token_was_sent() {
        let mock =
            MockApi::respond_with(401, r#"{"errors":[{"detail":"authentication required"}]}"#);
        let err = mock
            .client(None)
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::AuthRequired { .. }),
            "{err:?}"
        );
        assert_eq!(mock.captured().authorization, None);

        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::TokenRejected { .. }),
            "{err:?}"
        );
        assert!(
            !err.to_string().contains("apiTestToken"),
            "token bytes leaked: {err}"
        );
    }

    /// A token-authenticated, code-less 403 surfaces a printable server
    /// `detail` unchanged: it distinguishes a token permission the
    /// user did not grant from a scope the token's user is not a
    /// member of, and the client must not collapse the second into
    /// the first.  Without an envelope the message degrades to the
    /// generic token-permission wording.
    #[test]
    fn publish_maps_printable_403_details_unchanged() {
        for detail in [
            "the token does not have the publish scope",
            "the scope does not exist or the token's user is not a member of it",
        ] {
            let body: &'static str =
                Box::leak(format!(r#"{{"errors":[{{"detail":"{detail}"}}]}}"#).into_boxed_str());
            let mock = MockApi::respond_with(403, body);
            let err = mock
                .client(Some(token()))
                .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
                .unwrap_err();
            match &err {
                RegistryApiError::Forbidden {
                    origin,
                    detail: Some(got),
                } => {
                    assert_eq!(origin, &mock.url);
                    assert_eq!(got, detail);
                }
                other => panic!("{detail}: expected Forbidden with the detail, got {other:?}"),
            }
            assert!(err.to_string().contains(detail), "{err}");
        }

        let mock = MockApi::respond_with(403, "no envelope here");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::Forbidden { detail: None, .. } => {}
            other => panic!("expected Forbidden without detail, got {other:?}"),
        }
        assert!(
            err.to_string()
                .contains("the stored token does not have the required scope"),
            "{err}"
        );
    }

    #[test]
    fn registry_details_cannot_inject_terminal_controls() {
        let mock = MockApi::respond_with(
            400,
            r#"{"errors":[{"detail":"denied\u001b[2J\nforged\u202ereordered"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();

        let rendered = err.to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "escape reached terminal: {rendered:?}"
        );
        assert!(
            !rendered.contains('\n'),
            "newline reached terminal: {rendered:?}"
        );
        assert!(
            rendered.contains(r"denied\u{1b}[2J\nforged\u{202e}reordered"),
            "{rendered:?}"
        );
    }

    /// A 403 whose envelope carries a `quota_*` code is a per-user quota
    /// refusal, not the missing-scope case: the server detail - which
    /// embeds the registry's own usage URL - must reach the user
    /// unchanged when printable. The client never builds a web URL itself.
    #[test]
    fn publish_maps_coded_403_quota_refusals_to_the_server_detail() {
        for code in [
            "quota_storage",
            "quota_packages_daily",
            "quota_packages_total",
            "quota_versions_daily",
        ] {
            let body: &'static str = Box::leak(
                format!(
                    r#"{{"errors":[{{"detail":"the quota is exhausted; see https://cabinpkg.com/dashboard for current usage","code":"{code}"}}]}}"#
                )
                .into_boxed_str(),
            );
            let mock = MockApi::respond_with(403, body);
            let err = mock
                .client(Some(token()))
                .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
                .unwrap_err();
            match &err {
                RegistryApiError::QuotaExceeded { detail } => {
                    assert_eq!(
                        detail,
                        "the quota is exhausted; \
                         see https://cabinpkg.com/dashboard for current usage"
                    );
                }
                other => panic!("{code}: expected QuotaExceeded, got {other:?}"),
            }
            let message = err.to_string();
            assert!(
                message.contains("see https://cabinpkg.com/dashboard for current usage"),
                "{code}: expected the server-embedded usage URL verbatim in: {message}"
            );
            assert!(
                !message.contains(&mock.url),
                "{code}: the client must not derive a URL from the API origin: {message}"
            );
            assert!(!message.contains("scope"), "{code}: {message}");
        }
    }

    /// A 403 with an unknown (non-`quota_*`) code falls back to the
    /// generic mapping carrying the detail string - never the misleading
    /// scope message, never a guessed quota message.
    #[test]
    fn publish_falls_back_to_the_detail_on_unknown_codes() {
        let mock = MockApi::respond_with(
            403,
            r#"{"errors":[{"detail":"refused for a brand-new reason","code":"shiny_new_refusal"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::ServerError {
                status: 403,
                detail: Some(detail),
            } => assert_eq!(detail, "refused for a brand-new reason"),
            other => panic!("expected the generic mapping, got {other:?}"),
        }
        assert!(!err.to_string().contains("scope"), "{err}");

        // A code on a status with its own mapping does not hijack it:
        // the 400 stays a BadRequest with the detail.
        let mock = MockApi::respond_with(
            400,
            r#"{"errors":[{"detail":"metadata name mismatch","code":"quota_storage"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::BadRequest { detail: Some(_) } => {}
            other => panic!("expected BadRequest, got {other:?}"),
        }
        assert!(err.to_string().contains("metadata name mismatch"), "{err}");
    }

    /// A 503 carrying `registry_over_budget`: the service-wide budget
    /// breaker has writes paused. The message says so and carries the
    /// `Retry-After` seconds when present.
    #[test]
    fn publish_maps_a_coded_503_to_registry_over_budget() {
        let mock = MockApi::respond_with_headers(
            503,
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#,
            &[("Retry-After", "900")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: Some(900),
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("over its free budget"), "{message}");
        assert!(
            message.contains("900"),
            "expected Retry-After in: {message}"
        );

        // Without a usable Retry-After header the mapping still holds and
        // degrades to "try again later".
        let mock = MockApi::respond_with(
            503,
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: None,
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        assert!(err.to_string().contains("try again later"), "{err}");

        // A body exactly at the read cap is still an envelope; the
        // rejection starts one byte later.
        let mock = MockApi::respond_with(503, padded_envelope(MAX_ERROR_BODY_BYTES));
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget { .. } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
    }

    /// A coded envelope padded out to `len` bytes: valid JSON on its own,
    /// but only within the cap does the padding stay harmless.
    fn padded_envelope(len: usize) -> String {
        let mut body =
            r#"{"errors":[{"detail":"over budget","code":"registry_over_budget"}]}"#.to_owned();
        // Trailing whitespace keeps the JSON parseable, which is exactly
        // what a truncating cap would let through.
        body.extend(std::iter::repeat_n(' ', len - body.len()));
        body
    }

    /// The code, not the status, identifies the breaker: Cloudflare's
    /// edge and the Workers runtime emit bare 503s of their own, and a
    /// platform outage must not be reported as the registry being over
    /// budget. A different code, a near-miss code, no entry, no code, an
    /// envelope-less body, and a body past the read cap all stay the
    /// generic server error - as does the breaker's old 402, which has
    /// no mapping left (the registry is pre-launch, so there is no
    /// legacy arm to keep).
    #[test]
    fn publish_keeps_uncoded_503s_and_the_old_402_generic() {
        for (status, body) in [
            (
                503,
                r#"{"errors":[{"detail":"origin is unreachable"}]}"#.to_owned(),
            ),
            (
                503,
                r#"{"errors":[{"detail":"nope","code":"something_else"}]}"#.to_owned(),
            ),
            // The comparison is exact: a code the real one is a prefix
            // of must not match.
            (
                503,
                r#"{"errors":[{"detail":"nope","code":"registry_over_budgets"}]}"#.to_owned(),
            ),
            (503, r#"{"errors":[]}"#.to_owned()),
            (503, "no envelope here".to_owned()),
            // One byte past the cap.  The envelope prefix parses on its
            // own, so only a rejecting (not truncating) cap keeps this
            // generic.
            (503, padded_envelope(MAX_ERROR_BODY_BYTES + 1)),
            (
                402,
                r#"{"errors":[{"detail":"over budget","code":"registry_over_budget"}]}"#.to_owned(),
            ),
        ] {
            let label = format!("{status} {}", &body[..body.len().min(72)]);
            let mock = MockApi::respond_with(status, body);
            let err = mock
                .client(Some(token()))
                .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
                .unwrap_err();
            match &err {
                RegistryApiError::ServerError { status: got, .. } => {
                    assert_eq!(*got, status, "case: {label}");
                }
                other => panic!("expected ServerError({status}), got {other:?}"),
            }
            // The generic mapping echoes the server's own `detail`, so
            // the check is that the *breaker's* wording never appears.
            assert!(
                !err.to_string().contains("not accepting publishes"),
                "{err}"
            );
        }
    }

    /// The breaker blocks yanks too: the shared mapping covers PATCH.
    #[test]
    fn set_yanked_maps_503_to_registry_over_budget() {
        let mock = MockApi::respond_with_headers(
            503,
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#,
            &[("Retry-After", "900")],
        );
        let err = mock
            .client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), true)
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: Some(900),
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
    }

    /// 429: the publish token bucket is empty; `Retry-After` says when
    /// the next publish will be accepted.
    #[test]
    fn publish_maps_429_to_rate_limited() {
        let mock = MockApi::respond_with_headers(
            429,
            r#"{"errors":[{"detail":"publish rate limit exceeded; retry after the token bucket refills","code":"rate_limited"}]}"#,
            &[("Retry-After", "42")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::RateLimited {
                retry_after_secs: Some(42),
            } => {}
            other => panic!("expected RateLimited, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("rate limit"), "{message}");
        assert!(message.contains("42"), "expected Retry-After in: {message}");

        // A missing or non-numeric Retry-After (an HTTP date, say)
        // degrades to no hint rather than failing the mapping.
        let mock = MockApi::respond_with_headers(
            429,
            r#"{"errors":[{"detail":"publish rate limit exceeded","code":"rate_limited"}]}"#,
            &[("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::RateLimited {
                retry_after_secs: None,
            } => {}
            other => panic!("expected RateLimited, got {other:?}"),
        }
        assert!(err.to_string().contains("try again later"), "{err}");
    }

    /// 413: the archive exceeds the per-archive size limit. The
    /// server detail (which carries the limit when the server states it)
    /// is appended; without an envelope the fixed message stands alone.
    #[test]
    fn publish_maps_413_to_archive_too_large() {
        let mock = MockApi::respond_with(
            413,
            r#"{"errors":[{"detail":"archive exceeds the per-archive size limit (16777216 bytes)","code":"archive_too_large"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::ArchiveTooLarge { detail: Some(_) } => {}
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("too large"), "{message}");
        assert!(
            message.contains("16777216 bytes"),
            "expected the limit from the detail in: {message}"
        );

        let mock = MockApi::respond_with(413, "not an envelope");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::ArchiveTooLarge { detail: None } => {}
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        assert!(err.to_string().contains("too large"), "{err}");
    }

    /// A well-formed envelope's `detail` reaches the 400 message; a
    /// malformed one degrades to the raw status.
    #[test]
    fn error_envelope_parses_and_degrades_to_the_raw_status() {
        let mock =
            MockApi::respond_with(400, r#"{"errors":[{"detail":"metadata name mismatch"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        assert!(
            err.to_string().contains("metadata name mismatch"),
            "expected the envelope detail in: {err}"
        );

        let mock = MockApi::respond_with(400, "<html>not the envelope</html>");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::BadRequest { detail: None } => {}
            other => panic!("expected BadRequest without detail, got {other:?}"),
        }
        assert!(
            err.to_string().contains("400"),
            "expected the raw status in: {err}"
        );

        let mock = MockApi::respond_with(500, "garbage");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes", false)
            .unwrap_err();
        match &err {
            RegistryApiError::ServerError {
                status: 500,
                detail: None,
            } => {}
            other => panic!("expected ServerError(500), got {other:?}"),
        }
    }

    /// The yank route: method, path, JSON body, idempotent 200, and
    /// the 404 mapping for unknown versions.
    #[test]
    fn set_yanked_patches_the_yank_route() {
        let mock = MockApi::respond_with(200, r#"{"ok":true}"#);
        mock.client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), true)
            .unwrap();
        let captured = mock.captured();
        assert_eq!(captured.method, "PATCH");
        assert_eq!(captured.path, "/api/v1/packages/fmtlib/fmt/10.2.1/yank");
        assert_eq!(captured.body, br#"{"yanked":true}"#);
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str())
        );

        mock.client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), false)
            .unwrap();
        assert_eq!(mock.captured().body, br#"{"yanked":false}"#);

        let mock = MockApi::respond_with(404, r#"{"errors":[{"detail":"unknown version"}]}"#);
        let err = mock
            .client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("9.9.9"), true)
            .unwrap_err();
        match &err {
            RegistryApiError::NotFound { name, version } => {
                assert_eq!(name, "fmtlib/fmt");
                assert_eq!(version, "9.9.9");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// The API base's own path prefix is preserved when building
    /// routes.
    #[test]
    fn routes_join_under_a_base_path() {
        let api = RegistryApi::new("https://registry.example.com/base", None).unwrap();
        let url = api
            .package_route("fmtlib/fmt", &version("10.2.1"), "/yank")
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://registry.example.com/base/api/v1/packages/fmtlib/fmt/10.2.1/yank"
        );
    }

    /// The GitHub Actions OIDC fetch: `GET <request URL>&audience=...`
    /// with the runner-supplied bearer, returning the response's
    /// `value` field.
    #[test]
    fn fetch_github_actions_jwt_sends_the_bearer_and_audience() {
        let mock = MockApi::respond_with(200, r#"{"value":"header.payload.signature"}"#);
        let request_url = format!("{}/token?api-version=2", mock.url);
        let jwt =
            fetch_github_actions_jwt(&request_url, "runner-request-token", "cabinpkg.com").unwrap();
        assert_eq!(jwt, "header.payload.signature");
        let captured = mock.captured();
        assert_eq!(captured.method, "GET");
        assert_eq!(captured.path, "/token?api-version=2&audience=cabinpkg.com");
        assert_eq!(
            captured.authorization.as_deref(),
            Some("Bearer runner-request-token")
        );
    }

    /// A 5xx from the runner's OIDC endpoint is retried (GitHub's own
    /// client retries this exact GET); the fetch succeeds when a later
    /// attempt answers.
    #[test]
    fn fetch_github_actions_jwt_retries_a_transient_5xx() {
        let server =
            Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"));
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let request_url = format!("http://{addr}/token?api-version=2");
        let server_for_thread = Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            let mut answered = 0;
            while let Ok(req) = server_for_thread.recv() {
                answered += 1;
                let response = if answered == 1 {
                    tiny_http::Response::from_string("bad gateway").with_status_code(502)
                } else {
                    tiny_http::Response::from_string(r#"{"value":"header.payload.signature"}"#)
                        .with_status_code(200)
                };
                let _ = req.respond(response);
            }
        });
        let jwt =
            fetch_github_actions_jwt(&request_url, "runner-request-token", "cabinpkg.com").unwrap();
        assert_eq!(jwt, "header.payload.signature");
        server.unblock();
        let _ = thread.join();
    }

    /// A non-200 or bodyless answer from the runner's OIDC endpoint is
    /// a [`RegistryApiError::GithubOidc`] naming the endpoint's role,
    /// never a registry-flavored error.
    #[test]
    fn fetch_github_actions_jwt_maps_failures_to_the_oidc_error() {
        let mock = MockApi::respond_with(500, "runner exploded");
        let request_url = format!("{}/token?api-version=2", mock.url);
        let err = fetch_github_actions_jwt(&request_url, "runner-request-token", "cabinpkg.com")
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::GithubOidc { .. }),
            "{err:?}"
        );

        let mock = MockApi::respond_with(200, r#"{"unexpected":true}"#);
        let request_url = format!("{}/token?api-version=2", mock.url);
        let err = fetch_github_actions_jwt(&request_url, "runner-request-token", "cabinpkg.com")
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::GithubOidc { .. }),
            "{err:?}"
        );
    }

    /// The exchange: `PUT /api/v1/trusted_publishing/tokens` with the
    /// JWT as the JSON body and no `Authorization` header (the JWT is
    /// the credential), parsing the minted token.
    #[test]
    fn exchange_trusted_publishing_puts_the_jwt_and_parses_the_token() {
        let mock = MockApi::respond_with(
            200,
            r#"{"token":"cabin_tp_pVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVp","expires_at":"2026-08-15T12:00:00.000Z"}"#,
        );
        let minted = exchange_trusted_publishing(&mock.url, "header.payload.signature").unwrap();
        assert_eq!(
            minted.expose(),
            "cabin_tp_pVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVp"
        );
        let captured = mock.captured();
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path, "/api/v1/trusted_publishing/tokens");
        assert_eq!(captured.authorization, None);
        assert_eq!(captured.body, br#"{"jwt":"header.payload.signature"}"#);
    }

    /// The registry's refusal is a deliberately uniform `401`; the
    /// mapped error must explain the trusted-publishing failure modes
    /// rather than advising `cabin login`.
    #[test]
    fn exchange_trusted_publishing_maps_the_uniform_401() {
        let mock = MockApi::respond_with(401, r#"{"errors":[{"detail":"unauthorized"}]}"#);
        let err = exchange_trusted_publishing(&mock.url, "header.payload.signature").unwrap_err();
        assert!(
            matches!(err, RegistryApiError::TrustedPublishingRefused { .. }),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(
            !message.contains("cabin login"),
            "misleading advice: {message}"
        );
    }

    /// A minted token the client cannot parse is a server-shaped
    /// failure, surfaced as [`RegistryApiError::GithubOidc`]-distinct
    /// server error, never a panic or a silent success.
    #[test]
    fn exchange_trusted_publishing_rejects_an_unparsable_token() {
        let mock = MockApi::respond_with(200, r#"{"token":"not a cabin token"}"#);
        let err = exchange_trusted_publishing(&mock.url, "header.payload.signature").unwrap_err();
        assert!(
            matches!(err, RegistryApiError::ServerError { .. }),
            "{err:?}"
        );
    }

    /// Self-revocation: `DELETE /api/v1/trusted_publishing/tokens`
    /// authorized by the exchanged token itself, answering `204`.
    #[test]
    fn revoke_trusted_publishing_deletes_with_the_bearer() {
        let mock = MockApi::respond_with(204, "");
        let minted = Token::parse("cabin_tp_pVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVp").unwrap();
        mock.client(Some(minted))
            .revoke_trusted_publishing()
            .unwrap();
        let captured = mock.captured();
        assert_eq!(captured.method, "DELETE");
        assert_eq!(captured.path, "/api/v1/trusted_publishing/tokens");
        assert_eq!(
            captured.authorization.as_deref(),
            Some("Bearer cabin_tp_pVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVp")
        );
    }

    // -----------------------------------------------------------------
    // Login sessions: the GitHub device flow and the session routes
    // -----------------------------------------------------------------

    const SESSION_TOKEN: &str = "cabin_ses_pVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVpVp";

    fn authorization(interval_secs: u64, expires_in_secs: u64) -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: "device-code-1".to_owned(),
            user_code: "ABCD-1234".to_owned(),
            verification_uri: "https://github.com/login/device".to_owned(),
            interval_secs,
            expires_in_secs,
        }
    }

    /// The device-code request: `POST /login/device/code` with the
    /// client id and an empty scope as the form body, parsing the
    /// code pair and the poll parameters.
    #[test]
    fn request_device_authorization_posts_the_client_id() {
        let mock = MockApi::respond_with(
            200,
            r#"{"device_code":"dc-1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        );
        let granted = request_device_authorization(&mock.url, "Ov23xTest").unwrap();
        assert_eq!(granted.device_code, "dc-1");
        assert_eq!(granted.user_code, "ABCD-1234");
        assert_eq!(granted.verification_uri, "https://github.com/login/device");
        assert_eq!(granted.interval_secs, 5);
        assert_eq!(granted.expires_in_secs, 900);

        let captured = mock.captured();
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/login/device/code");
        assert_eq!(captured.authorization, None);
        assert_eq!(captured.body, b"client_id=Ov23xTest&scope=");
    }

    /// The poll state machine: `authorization_pending` keeps the
    /// interval, `slow_down` adds five seconds, and the success
    /// response yields the access token - `refresh_token` fields and
    /// the like discarded unread.  The injected sleeper proves the
    /// pacing without a real wait.
    #[test]
    fn poll_device_token_walks_pending_slow_down_success() {
        let mock = MockApi::respond_with_script(
            vec![
                (200, r#"{"error":"authorization_pending"}"#.to_owned()),
                (200, r#"{"error":"slow_down","interval":10}"#.to_owned()),
                (
                    200,
                    r#"{"access_token":"gho_secret1","token_type":"bearer","scope":"","expires_in":28800,"refresh_token":"ghr_secret2","refresh_token_expires_in":15811200}"#
                        .to_owned(),
                ),
            ],
            &[],
        );
        let mut slept = Vec::new();
        let token = poll_device_token(&mock.url, "Ov23xTest", &authorization(5, 900), |pause| {
            slept.push(pause.as_secs());
        })
        .unwrap();
        assert_eq!(token, "gho_secret1");
        assert_eq!(slept, vec![5, 5, 10], "slow_down must add five seconds");

        let captured = mock.captured();
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/login/oauth/access_token");
        assert_eq!(
            captured.body,
            b"client_id=Ov23xTest&device_code=device-code-1&\
              grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
                .as_slice()
        );
    }

    /// The two terminal refusals end the flow with actionable
    /// wording, whatever HTTP status carries them.
    #[test]
    fn poll_device_token_maps_expired_and_denied() {
        for (body, expected) in [
            (r#"{"error":"expired_token"}"#, "expired"),
            (r#"{"error":"access_denied"}"#, "denied"),
        ] {
            // GitHub answers flow errors under varying statuses; both
            // shapes must map identically.
            for status in [200, 400] {
                let mock = MockApi::respond_with(status, body);
                let err =
                    poll_device_token(&mock.url, "Ov23xTest", &authorization(5, 900), |_pause| {})
                        .unwrap_err();
                match &err {
                    RegistryApiError::GithubDeviceFlow { message } => {
                        assert!(message.contains(expected), "{status}: {message}");
                    }
                    other => panic!("expected GithubDeviceFlow, got {other:?}"),
                }
            }
        }
    }

    /// A server that answers pending forever cannot hang the login:
    /// the loop gives up once the slept time reaches the device
    /// code's own lifetime.
    #[test]
    fn poll_device_token_gives_up_when_the_device_code_lifetime_elapses() {
        let mock = MockApi::respond_with(200, r#"{"error":"authorization_pending"}"#);
        let mut polls = 0u64;
        let err = poll_device_token(&mock.url, "Ov23xTest", &authorization(5, 12), |_pause| {
            polls += 1;
        })
        .unwrap_err();
        match &err {
            RegistryApiError::GithubDeviceFlow { message } => {
                assert!(message.contains("expired"), "{message}");
            }
            other => panic!("expected GithubDeviceFlow, got {other:?}"),
        }
        assert_eq!(polls, 3, "12s lifetime at a 5s interval is three polls");
    }

    /// The mint: `PUT /api/v1/sessions/tokens` with the GitHub token
    /// as the JSON body and no `Authorization` header (the GitHub
    /// token is the credential), parsing the minted token and its
    /// expiry.
    #[test]
    fn exchange_login_session_puts_the_github_token_and_parses_the_grant() {
        let mock = MockApi::respond_with(
            200,
            format!(r#"{{"token":"{SESSION_TOKEN}","expires_at":"2999-01-01T12:00:00.000Z"}}"#),
        );
        let grant = exchange_login_session(&mock.url, "gho_secret1").unwrap();
        assert_eq!(grant.token.expose(), SESSION_TOKEN);
        assert_eq!(grant.expires_at, "2999-01-01T12:00:00.000Z");
        let captured = mock.captured();
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path, "/api/v1/sessions/tokens");
        assert_eq!(captured.authorization, None);
        assert_eq!(captured.body, br#"{"github_token":"gho_secret1"}"#);
    }

    /// The registry's refusal is a deliberately uniform `401`; a
    /// mint the client cannot parse is a server-shaped failure.
    #[test]
    fn exchange_login_session_maps_the_uniform_401_and_unparsable_grants() {
        let mock = MockApi::respond_with(401, r#"{"errors":[{"detail":"unauthorized"}]}"#);
        let err = exchange_login_session(&mock.url, "gho_secret1").unwrap_err();
        assert!(
            matches!(err, RegistryApiError::SessionRefused { .. }),
            "{err:?}"
        );

        let mock = MockApi::respond_with(200, r#"{"token":"not a cabin token"}"#);
        let err = exchange_login_session(&mock.url, "gho_secret1").unwrap_err();
        assert!(
            matches!(err, RegistryApiError::ServerError { .. }),
            "{err:?}"
        );

        // A syntactically valid NON-session Cabin credential is not
        // a session grant: wrong scopes, and beyond the session
        // revocation route's reach.
        let mock = MockApi::respond_with(
            200,
            r#"{"token":"cabin_tp_pVp-p_Wl","expires_at":"2999-01-01T00:00:00.000Z"}"#.to_owned(),
        );
        let err = exchange_login_session(&mock.url, "gho_secret1").unwrap_err();
        assert!(err.to_string().contains("without a usable token"), "{err}");

        // A well-formed but already-expired grant is a server fault:
        // login would report a success every later lookup contradicts.
        let mock = MockApi::respond_with(
            200,
            format!(r#"{{"token":"{SESSION_TOKEN}","expires_at":"2000-01-01T00:00:00.000Z"}}"#),
        );
        let err = exchange_login_session(&mock.url, "gho_secret1").unwrap_err();
        assert!(err.to_string().contains("already-expired"), "{err}");
    }

    /// The mint request body held a live GitHub token, so nothing
    /// registry-controlled from the response may reach diagnostics or
    /// disk: an error envelope's detail is dropped (a registry could
    /// reflect the token there), and a grant whose `expires_at` is
    /// not an RFC 3339 stamp - which login would store and print - is
    /// refused.
    #[test]
    fn exchange_login_session_never_surfaces_registry_chosen_text() {
        let mock = MockApi::respond_with(400, r#"{"errors":[{"detail":"gho_reflected"}]}"#);
        let err = exchange_login_session(&mock.url, "gho_reflected").unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains("gho_reflected") && !format!("{err:?}").contains("gho_reflected"),
            "a reflected detail must be dropped: {message}"
        );

        let mock = MockApi::respond_with(
            200,
            format!(r#"{{"token":"{SESSION_TOKEN}","expires_at":"gho_reflected"}}"#),
        );
        let err = exchange_login_session(&mock.url, "gho_reflected").unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("without a usable expiry") && !message.contains("gho_reflected"),
            "{message}"
        );

        // The contract is UTC-only RFC 3339: a non-UTC offset - valid
        // RFC 3339, but not what the protocol mints - refuses rather
        // than being stored in a shape the client's expiry check
        // cannot read, while the `+00:00` UTC spelling stays accepted.
        let mock = MockApi::respond_with(
            200,
            format!(r#"{{"token":"{SESSION_TOKEN}","expires_at":"2999-01-01T08:00:00-04:00"}}"#),
        );
        let err = exchange_login_session(&mock.url, "gho_secret1").unwrap_err();
        assert!(err.to_string().contains("without a usable expiry"), "{err}");
        let mock = MockApi::respond_with(
            200,
            format!(r#"{{"token":"{SESSION_TOKEN}","expires_at":"2999-01-01T08:00:00+00:00"}}"#),
        );
        assert_eq!(
            exchange_login_session(&mock.url, "gho_secret1")
                .unwrap()
                .expires_at,
            "2999-01-01T08:00:00+00:00"
        );
    }

    /// Session self-revocation: `DELETE /api/v1/sessions/tokens`
    /// authorized by the session token itself; the uniform `401` a
    /// repeat DELETE answers maps to the session refusal the caller
    /// tolerates.
    #[test]
    fn revoke_session_deletes_with_the_bearer_and_tolerates_the_401() {
        let mock = MockApi::respond_with(204, "");
        let minted = Token::parse(SESSION_TOKEN).unwrap();
        mock.client(Some(minted.clone())).revoke_session().unwrap();
        let captured = mock.captured();
        assert_eq!(captured.method, "DELETE");
        assert_eq!(captured.path, "/api/v1/sessions/tokens");
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {SESSION_TOKEN}").as_str())
        );

        let mock = MockApi::respond_with(401, r#"{"errors":[{"detail":"unauthorized"}]}"#);
        let err = mock.client(Some(minted)).revoke_session().unwrap_err();
        assert!(
            matches!(err, RegistryApiError::SessionRefused { .. }),
            "{err:?}"
        );
    }
}
