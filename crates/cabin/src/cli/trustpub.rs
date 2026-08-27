//! Trusted publishing under GitHub Actions: the automatic
//! OIDC-for-registry-token exchange serving remote publishing
//! (`docs/remote-registry.md`, "Trusted publishing").
//!
//! Only `cabin publish` exchanges.  Reads never do: the hosted read
//! plane needs no token, and a `cabin build` inside an unrelated
//! workflow that granted `id-token: write` for some other service
//! must not spend a network round-trip (or a minted publish-capable
//! token) per invocation.  `cabin yank` never does either: the
//! registry mints exchanged tokens with only the `publish` scope, so
//! an auto-exchange there would trade a working stored yank
//! credential for a token the yank route must refuse.  The exchange
//! is further origin-confined, more strictly than the environment
//! override: the default hosted registry always, a loopback registry
//! only when the user named it with `--index-url` - the run's OIDC
//! token is a credential the hosted registry accepts from anyone, so
//! a project-steered index must never see it.

use anyhow::{Result, bail};

use cabin_credentials::Token;

use crate::cli::term_verbosity::Reporter;

/// The audience the registry's JWT verifier expects
/// (`registry/src/trustpub.rs`, `DEFAULT_AUDIENCE`).
const AUDIENCE: &str = "cabinpkg.com";

/// How the publish credential resolves before API discovery.
pub(crate) enum PublishCredential {
    /// Resolved without the network: the environment override or a
    /// stored credential.
    Token(Token),
    /// GitHub Actions ambient OIDC credentials are present and the
    /// origin is eligible: call [`exchange`] once the registry's
    /// `api` origin is discovered.
    NeedsExchange,
    /// No credential source applies.  `expired_at` carries the
    /// stored session's lapsed expiry when one existed, so the
    /// caller can name the cause (the registry's uniform 401
    /// cannot) instead of failing inexplicably.
    None { expired_at: Option<String> },
}

/// The one place publish-credential precedence is defined: the
/// explicit `CABIN_REGISTRY_TOKEN` override, then the GitHub Actions
/// auto-exchange, then the stored login session (`cabin login`'s
/// minted `cabin_ses_` token, from the platform keychain or the
/// fallback credentials file), then none.  Sessions sit *below* the
/// override and the exchange on purpose: CI's explicit or ambient
/// credential always wins over a personal session that happens to be
/// on the same machine.
/// The override leg keeps its own origin-trust gate
/// ([`env_token_eligible`](super::login::env_token_eligible)), which
/// asks whether the *user* chose the origin; the exchange leg is
/// stricter ([`exchange_origin_eligible`]): the run's OIDC JWT is a
/// credential the hosted registry accepts from anyone, so a loopback
/// registry only qualifies when the user themselves named it with
/// `--index-url` (`index_from_cli`) - not even their own config file
/// counts.
///
/// # Errors
/// Fails when GitHub Actions is detected without the OIDC endpoint -
/// the workflow lacks `permissions: id-token: write` - rather than
/// falling through to a `401` whose cause the user would have to
/// guess.
pub(crate) fn publish_credential(
    index_url: &str,
    origin: &str,
    index_user_chosen: bool,
    index_from_cli: bool,
    reporter: Reporter,
) -> Result<PublishCredential> {
    let env_eligible = super::login::env_token_eligible(origin, index_user_chosen)?;
    if let Some(token) = cabin_credentials::env_token(env_eligible)? {
        return Ok(PublishCredential::Token(token));
    }
    match exchange_decision(
        exchange_origin_eligible(origin, index_from_cli)?,
        env_value(cabin_env::GITHUB_ACTIONS).as_deref(),
        env_value(cabin_env::ACTIONS_ID_TOKEN_REQUEST_URL).as_deref(),
        env_value(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN).as_deref(),
    ) {
        Some(OidcDecision::Exchange) => return Ok(PublishCredential::NeedsExchange),
        Some(OidcDecision::MissingIdTokenPermission) => bail!(
            "this run is on GitHub Actions but has no OIDC token endpoint, so the \
             trusted-publishing exchange cannot run; grant the workflow (or job) \
             `permissions: id-token: write`, or set CABIN_REGISTRY_TOKEN explicitly"
        ),
        Some(OidcDecision::NotGithubActions) | None => {}
    }
    let lookup = cabin_credentials::stored_token(origin)?;
    if let Some(warning) = lookup.warning {
        reporter.warning(format_args!("{warning}"));
    }
    let Some(token) = lookup.token else {
        if lookup.expired_at.is_none() {
            super::login::warn_if_env_token_withheld(index_url, origin, env_eligible, reporter);
        }
        return Ok(PublishCredential::None {
            expired_at: lookup.expired_at,
        });
    };
    Ok(PublishCredential::Token(token))
}

/// What the GitHub Actions environment says about the OIDC exchange.
#[derive(Debug, PartialEq, Eq)]
enum OidcDecision {
    /// Both runner OIDC variables are present: exchange.
    Exchange,
    /// Running under Actions without the OIDC endpoint: the workflow
    /// lacks `permissions: id-token: write` - fail with the fix.
    MissingIdTokenPermission,
    /// Not a GitHub Actions run.
    NotGithubActions,
}

/// Whether the exchange may serve `origin` at all: the default
/// hosted registry always (a project cannot steer where its
/// authentically-served config.json lives), a loopback registry only
/// when the user named it on the command line - a checked-out
/// project's config or `[source-replacement]` can point at a loopback
/// port where its own earlier build/test code left a daemon, and that
/// daemon could proxy the fresh JWT (audience: the hosted registry)
/// to the real exchange and keep the minted token.
fn exchange_origin_eligible(origin: &str, index_from_cli: bool) -> Result<bool> {
    let default_origin =
        cabin_credentials::normalize_origin(cabin_core::registry::DEFAULT_INDEX_URL)?;
    Ok(origin == default_origin || (cabin_credentials::url_is_loopback(origin) && index_from_cli))
}

/// The exchange leg under the caller's origin-trust decision:
/// `None` for an ineligible origin, where the GitHub Actions
/// environment must not even be interpreted - neither the exchange
/// nor the missing-permission error may fire for an origin a
/// checked-out project could have steered.
fn exchange_decision(
    eligible: bool,
    github_actions: Option<&str>,
    request_url: Option<&str>,
    request_token: Option<&str>,
) -> Option<OidcDecision> {
    if !eligible {
        return None;
    }
    Some(oidc_decision(github_actions, request_url, request_token))
}

/// Decide from the three runner variables (empty values count as
/// unset, per environment-variable convention).  The endpoint pair
/// alone decides the exchange - `GITHUB_ACTIONS` only distinguishes
/// "not Actions" from "Actions without `id-token: write`" when the
/// pair is absent.
fn oidc_decision(
    github_actions: Option<&str>,
    request_url: Option<&str>,
    request_token: Option<&str>,
) -> OidcDecision {
    if request_url.is_some() && request_token.is_some() {
        return OidcDecision::Exchange;
    }
    if github_actions == Some("true") {
        return OidcDecision::MissingIdTokenPermission;
    }
    OidcDecision::NotGithubActions
}

/// A non-empty environment value, `None` otherwise.
fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// A minted trusted-publishing token, owned by exactly the publish
/// invocation that exchanged for it.  Dropping it best-effort revokes
/// the token - every exit path of the owning flow included, `?`
/// returns and unwinds alike - so there is deliberately no
/// process-global state: concurrent or repeated invocations in one
/// process each mint, use, and revoke their own token, and one
/// invocation can never reuse (or revoke) another's.  The pair also
/// pins the API origin the token was minted for; the owner must send
/// it there and nowhere else.
pub(crate) struct ExchangedToken {
    token: Token,
    api_url: String,
}

impl ExchangedToken {
    /// The minted bearer token.
    pub(crate) fn token(&self) -> &Token {
        &self.token
    }

    /// The API origin the token was minted for - the only origin it
    /// may be presented to.
    pub(crate) fn api_url(&self) -> &str {
        &self.api_url
    }
}

impl Drop for ExchangedToken {
    /// Best-effort self-revocation.  Failures are deliberately
    /// ignored: the token expires server-side within its 30-minute
    /// lifetime, and a revocation problem must never change the
    /// owning command's outcome.
    fn drop(&mut self) {
        let Ok(api) = cabin_registry_api::RegistryApi::new(&self.api_url, Some(self.token.clone()))
        else {
            return;
        };
        let _ = api.revoke_trusted_publishing();
    }
}

/// Run the exchange against `api_url` (the registry's discovered
/// `api` origin): fetch the run's OIDC JWT and exchange it for a
/// short-lived registry token, returned as the owning
/// [`ExchangedToken`] guard.  Called once per publish invocation (the
/// minted token is multi-use within its lifetime, and GitHub's OIDC
/// endpoint is rate-limited - a multi-package flow must hoist the
/// guard above its loop, never exchange per package).  Both secrets
/// are masked out of the runner's log before anything else can print
/// them; neither is ever written to disk.
///
/// # Errors
/// Refuses a loopback index whose config.json declares a non-loopback
/// `api` (see [`exchange_api_allowed`]) before any secret is fetched;
/// otherwise propagates the JWT fetch and exchange failures - the
/// exchange `401` is the registry's deliberately uniform refusal.
pub(crate) fn exchange(index_origin: &str, api_url: &str) -> Result<ExchangedToken> {
    if !exchange_api_allowed(index_origin, api_url) {
        bail!(
            "refusing the trusted-publishing exchange: loopback registry `{index_origin}` \
             declares the non-loopback API origin `{api_url}`, which must not receive the run's \
             OIDC token - a loopback index (which project config or `[source-replacement]` can \
             select) may only name a loopback API"
        );
    }
    let (Some(request_url), Some(request_token)) = (
        env_value(cabin_env::ACTIONS_ID_TOKEN_REQUEST_URL),
        env_value(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN),
    ) else {
        // Unreachable after a `NeedsExchange` decision; a defensive
        // error beats a panic in a credential path.
        bail!("the GitHub Actions OIDC endpoint disappeared from the environment");
    };
    let jwt = cabin_registry_api::fetch_github_actions_jwt(&request_url, &request_token, AUDIENCE)?;
    mask(&jwt);
    let token = cabin_registry_api::exchange_trusted_publishing(api_url, &jwt)?;
    mask(token.expose());
    Ok(ExchangedToken {
        token,
        api_url: api_url.to_owned(),
    })
}

/// Whether the discovered `api` origin may receive the run's OIDC
/// JWT.  The JWT is itself a credential - delivered unconsumed, it
/// can be exchanged against the real registry for a publish-capable
/// token - so even a user-selected loopback index (the only loopback
/// kind that reaches the exchange, see [`exchange_origin_eligible`])
/// must not fan the JWT out to a non-loopback `api` its config.json
/// happens to declare.  The default hosted registry's config.json
/// arrives over HTTPS from the registry itself, so its declared `api`
/// is taken at its word.
fn exchange_api_allowed(index_origin: &str, api_url: &str) -> bool {
    !cabin_credentials::url_is_loopback(index_origin) || cabin_credentials::url_is_loopback(api_url)
}

/// `::add-mask::` hides the value from the workflow log the moment it
/// exists.  Only emitted under GitHub Actions proper: the command is
/// noise anywhere else.  On stderr, deliberately: the runner
/// processes workflow commands on both output streams, and stdout
/// belongs to the command's own report - a mask line there would
/// corrupt `--format json`, and a step that pipes stdout (`| jq`)
/// would swallow the command before the runner ever saw it.  Written
/// to the raw stream rather than through `Reporter` on purpose: this
/// is machine protocol, not human output, and it must survive
/// `--quiet` undecorated or the secret goes unmasked.
fn mask(secret: &str) {
    if env_value(cabin_env::GITHUB_ACTIONS).as_deref() == Some("true") {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "::add-mask::{secret}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The endpoint pair alone triggers the exchange; the
    /// `GITHUB_ACTIONS` marker only refines the no-endpoint answer.
    #[test]
    fn oidc_decision_exchanges_when_the_endpoint_pair_is_present() {
        for marker in [Some("true"), Some("false"), None] {
            assert_eq!(
                oidc_decision(marker, Some("https://oidc.example/token?x=1"), Some("req")),
                OidcDecision::Exchange,
                "marker: {marker:?}"
            );
        }
    }

    /// Actions without the endpoint means the workflow lacks
    /// `permissions: id-token: write`: a hard, actionable error
    /// instead of an inexplicable 401 later.
    #[test]
    fn oidc_decision_flags_actions_without_the_endpoint() {
        assert_eq!(
            oidc_decision(Some("true"), None, None),
            OidcDecision::MissingIdTokenPermission
        );
        assert_eq!(
            oidc_decision(Some("true"), Some("https://oidc.example/token"), None),
            OidcDecision::MissingIdTokenPermission
        );
        assert_eq!(
            oidc_decision(Some("true"), None, Some("req")),
            OidcDecision::MissingIdTokenPermission
        );
    }

    /// The origin gate outranks everything: for an ineligible origin
    /// the Actions environment is never interpreted - full ambience
    /// neither exchanges nor raises the missing-permission error.
    #[test]
    fn exchange_decision_never_fires_for_an_ineligible_origin() {
        assert_eq!(
            exchange_decision(
                false,
                Some("true"),
                Some("https://oidc.example/token?x=1"),
                Some("req"),
            ),
            None
        );
        assert_eq!(exchange_decision(false, Some("true"), None, None), None);
        assert_eq!(
            exchange_decision(
                true,
                Some("true"),
                Some("https://oidc.example/token?x=1"),
                Some("req"),
            ),
            Some(OidcDecision::Exchange)
        );
        assert_eq!(
            exchange_decision(true, Some("true"), None, None),
            Some(OidcDecision::MissingIdTokenPermission)
        );
    }

    /// The exchange origin gate: the hosted default always; loopback
    /// only when the user typed it (`--index-url`), never when project
    /// config picked it; anything else never.
    #[test]
    fn exchange_origin_gate_requires_a_user_typed_loopback() {
        for (origin, from_cli, expected) in [
            ("https://registry.cabinpkg.com", false, true),
            ("https://registry.cabinpkg.com", true, true),
            ("http://127.0.0.1:4000", true, true),
            ("http://127.0.0.1:4000", false, false),
            ("http://localhost:4000", false, false),
            ("https://evil.example", true, false),
        ] {
            assert_eq!(
                exchange_origin_eligible(origin, from_cli).unwrap(),
                expected,
                "origin: {origin}, from_cli: {from_cli}"
            );
        }
    }

    /// The JWT-delivery policy: a loopback index only ever hands the
    /// run's OIDC token to a loopback `api`; the default hosted
    /// registry's authentically-served config.json is taken at its
    /// word.
    #[test]
    fn exchange_api_policy_confines_loopback_indexes() {
        // Loopback index: loopback api only.
        assert!(exchange_api_allowed(
            "http://127.0.0.1:4000",
            "http://127.0.0.1:4001"
        ));
        assert!(!exchange_api_allowed(
            "http://127.0.0.1:4000",
            "https://evil.example"
        ));
        assert!(!exchange_api_allowed(
            "http://localhost:4000",
            "https://cabinpkg.com"
        ));
        // Hosted index: its HTTPS config.json names the api.
        assert!(exchange_api_allowed(
            "https://registry.cabinpkg.com",
            "https://cabinpkg.com"
        ));
    }

    /// Anything else - including a half-present pair outside Actions -
    /// is not an Actions run and falls through to stored credentials.
    #[test]
    fn oidc_decision_ignores_non_actions_environments() {
        assert_eq!(
            oidc_decision(None, None, None),
            OidcDecision::NotGithubActions
        );
        assert_eq!(
            oidc_decision(Some("false"), None, None),
            OidcDecision::NotGithubActions
        );
        assert_eq!(
            oidc_decision(None, Some("https://oidc.example/token"), None),
            OidcDecision::NotGithubActions
        );
    }
}
