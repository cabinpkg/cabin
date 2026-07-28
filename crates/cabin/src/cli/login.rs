//! `cabin login` / `cabin logout`: store or remove a registry token.
//!
//! Both commands resolve the registry the same way the fetch family
//! does (`--index-url`, else the `[registry] index-url` config
//! setting, else the default hosted registry) and key the stored
//! credential on the normalized index origin.  The token itself only
//! ever flows stdin → `cabin-credentials`; it is never echoed,
//! logged, or printed back.

use std::io::IsTerminal as _;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_credentials::{CredentialStore, Token};

use crate::cli::config::resolve_index_source;
use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct LoginArgs {
    /// Sparse HTTP index URL of the registry to log in to.  Falls
    /// back to the `[registry] index-url` config setting, then the
    /// default registry.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct LogoutArgs {
    /// Sparse HTTP index URL of the registry to log out from.  Falls
    /// back to the `[registry] index-url` config setting, then the
    /// default registry.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,
}

pub(crate) fn login(args: &LoginArgs, reporter: Reporter) -> Result<()> {
    let index_url = effective_registry_index_url(
        args.index_url.as_deref(),
        "cabin login",
        "tokens only apply to `--index-url` registries",
        true,
    )?;
    let origin = cabin_credentials::normalize_origin(&index_url)?;
    // A token stored for a plain-http non-loopback origin could
    // never be attached (the client refuses cleartext beyond
    // loopback), so refuse to store it instead of confusing the
    // next fetch.
    if origin.starts_with("http://") && !cabin_credentials::url_is_loopback(&origin) {
        bail!(
            "refusing to store a token for `{origin}`: tokens are never sent over plain `http` \
             except to loopback hosts; use an `https` registry URL"
        );
    }
    // Name the destination before the token is read, so a user can
    // abort if configuration steered the login somewhere unexpected.
    reporter.note(format_args!("logging in to `{origin}`"));
    // Login-URL discovery mirrors Cargo's `login_url` challenge
    // (docs/remote-registry.md, "Authentication"): one advisory
    // unauthenticated probe of the index's config.json. A registry
    // without the challenge, an implausible URL, or a failed probe
    // (offline) degrades to the generic wording - the probe never
    // blocks login, and the pasted token is accepted either way.
    // Under offline mode the probe is skipped outright: advisory or
    // not, `CABIN_NET_OFFLINE` promises no network traffic.
    let login_url = if crate::cli::config::effective_offline(false)? {
        None
    } else {
        cabin_index_http::fetch_login_url(&index_url)
    };
    match login_url {
        Some(login_url) => {
            reporter.note(format_args!("visit {login_url} to create a token"));
        }
        None => reporter.note(format_args!(
            "create a token in the registry's web interface"
        )),
    }
    let token = read_token()?;
    let store = CredentialStore::from_env()?;
    let loaded = store.load()?;
    surface_permissions_warning(reporter, loaded.permissions_warning);
    let mut credentials = loaded.credentials;
    credentials.set_token(origin.clone(), token);
    store.save(&credentials)?;
    reporter.status("Login", format_args!("token for `{origin}` saved"));
    Ok(())
}

pub(crate) fn logout(args: &LogoutArgs, reporter: Reporter) -> Result<()> {
    let origin = effective_registry_origin(args.index_url.as_deref(), "cabin logout")?;
    let store = CredentialStore::from_env()?;
    let loaded = store.load()?;
    surface_permissions_warning(reporter, loaded.permissions_warning);
    let mut credentials = loaded.credentials;
    if credentials.remove_token(&origin) {
        store.save(&credentials)?;
        reporter.status("Logout", format_args!("token for `{origin}` removed"));
    } else {
        reporter.status("Logout", format_args!("no token was stored for `{origin}`"));
    }
    Ok(())
}

/// Resolve the credential to attach to sparse-HTTP requests for
/// `index_url`: the `CABIN_REGISTRY_TOKEN` env override first (for
/// eligible origins, see [`env_token_eligible`]), then the
/// `credentials.toml` entry for the URL's origin.  Without a stored
/// credential the client stays tokenless and the read path is
/// byte-identical to the unauthenticated flow.
pub(crate) fn registry_auth_for_index_url(
    index_url: &str,
    reporter: Reporter,
) -> Result<Option<cabin_index_http::RegistryAuth>> {
    let origin = cabin_credentials::normalize_origin(index_url)?;
    let lookup = cabin_credentials::lookup_token(&origin, env_token_eligible(&origin)?)?;
    surface_permissions_warning(reporter, lookup.permissions_warning);
    match lookup.token {
        Some(token) => Ok(Some(cabin_index_http::RegistryAuth::for_index_url(
            index_url, token,
        )?)),
        None => Ok(None),
    }
}

fn surface_permissions_warning(reporter: Reporter, warning: Option<String>) {
    if let Some(warning) = warning {
        reporter.warning(format_args!("{warning}"));
    }
}

/// Whether the `CABIN_REGISTRY_TOKEN` environment override may serve
/// `origin` (a normalized origin string).  The override carries no
/// origin key of its own, and an invocation's index origin can come
/// from project-level config or `[source-replacement]` - inputs a
/// checked-out project controls - so an unrestricted override would
/// let any built project route the credential to an origin of its
/// choosing.  It therefore serves exactly the default hosted
/// registry (its intended CI consumer) and loopback origins (local
/// testing); every other registry uses `credentials.toml`, which is
/// origin-keyed by construction.
pub(crate) fn env_token_eligible(origin: &str) -> Result<bool> {
    let default_origin =
        cabin_credentials::normalize_origin(cabin_core::registry::DEFAULT_INDEX_URL)?;
    Ok(origin == default_origin || cabin_credentials::url_is_loopback(origin))
}

/// Resolve the registry origin `cabin login` / `cabin logout`
/// operate on: apply the documented index-source precedence
/// (`--index-url`, else config, else the default registry), and
/// reject index sources that cannot carry a token (a local path).
fn effective_registry_origin(cli_index_url: Option<&str>, command: &str) -> Result<String> {
    let url = effective_registry_index_url(
        cli_index_url,
        command,
        "tokens only apply to `--index-url` registries",
        true,
    )?;
    Ok(cabin_credentials::normalize_origin(&url)?)
}

/// Resolve the HTTP index URL a registry command targets: apply the
/// documented index-source precedence (`--index-url`, else config,
/// with `[source-replacement]`), and reject a local path -
/// `local_path_reason` finishes the local-path error with the
/// command's own justification.
///
/// `credential_command` marks the `cabin login` / `cabin logout`
/// mode and decides two things at once.  An absent source falls back
/// to the default hosted registry (like the read pipeline), while
/// the mutation commands (`cabin yank`) keep requiring an explicit
/// source - a mutation must never target a registry the user did not
/// name.  And config discovery is *user-level only*: a checked-out
/// project's `.cabin/config.toml` (registry selection or
/// `[source-replacement]`) must not be able to steer where a pasted
/// credential is stored - the token would go to whatever origin the
/// project picked.  Reads key stored tokens on the effective origin
/// too, so a project-steered read simply finds no credential.
pub(crate) fn effective_registry_index_url(
    cli_index_url: Option<&str>,
    command: &str,
    local_path_reason: &str,
    credential_command: bool,
) -> Result<String> {
    // An explicit `--index-url` needs no config fallback: skip
    // discovery entirely so an unrelated broken config file or
    // manifest cannot fail the command, and key the token on exactly
    // the origin the user named.
    let config = if cli_index_url.is_some() {
        cabin_config::EffectiveConfig::default()
    } else if credential_command {
        user_level_config()?
    } else {
        effective_config_for_cwd()?
    };
    let source = match resolve_index_source(None, cli_index_url, &config)? {
        Some(source) => source,
        None if credential_command => crate::cli::config::default_index_source(),
        None => {
            bail!("`{command}` requires --index-url or a `[registry] index-url` config setting")
        }
    };
    // Mirror the fetch pipeline: a config-supplied (or defaulted)
    // registry source is subject to `[source-replacement]`, so the
    // token must be keyed on the origin the later fetch will
    // actually contact.
    let locator = crate::cli::config::index_source_kind_to_locator(&source.kind);
    let resolution = crate::cli::patch::apply_source_replacement(locator, &config, false)?;
    match resolution.resolved {
        cabin_core::SourceLocator::IndexPath { path } => bail!(
            "`{command}` requires an HTTP registry, but the effective index source is the local \
             path `{path}`; {local_path_reason}"
        ),
        cabin_core::SourceLocator::IndexUrl { url } => Ok(url),
    }
}

/// Config discovery for a command that may run outside any project:
/// the workspace/package config applies when the current directory
/// is inside one, and the user-level config always applies.
fn effective_config_for_cwd() -> Result<cabin_config::EffectiveConfig> {
    let manifest_path = crate::cli::resolve_invocation_manifest(None)?;
    if manifest_path.is_file() {
        return crate::cli::config::load_effective_config_for_manifest(&manifest_path);
    }
    user_level_config()
}

/// User-level config only - no workspace or package layers.  The
/// credential commands resolve their registry through this so a
/// hostile checkout cannot steer where a token is stored.
fn user_level_config() -> Result<cabin_config::EffectiveConfig> {
    let inputs = cabin_config::ConfigDiscoveryInputs::from_process(None);
    let discovery =
        cabin_config::discover_config_files(&inputs).context("failed to load Cabin config")?;
    Ok(cabin_config::merge_loaded_files(discovery.loaded_files))
}

/// Read the token from stdin: without echo when stdin is a terminal
/// (so the secret never lands in scrollback), a plain line read
/// otherwise so piping (`echo $TOKEN | cabin login ...`) works.
fn read_token() -> Result<Token> {
    let raw = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("token: ").context("failed to read token")?
    } else {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("failed to read token from stdin")?;
        buf
    };
    Ok(Token::parse(raw.trim())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An explicit `--index-url` is used verbatim: no config
    /// discovery, no default substitution.
    #[test]
    fn explicit_index_url_resolves_to_its_own_origin() {
        let origin =
            effective_registry_origin(Some("https://registry.example.com/idx"), "cabin login")
                .unwrap();
        assert_eq!(origin, "https://registry.example.com");
    }

    /// The env-token override serves exactly the default hosted
    /// registry and loopback origins; any other origin - which
    /// project-level config or `[source-replacement]` could have
    /// picked - is ineligible.
    #[test]
    fn env_token_eligibility_is_origin_bound() {
        for (origin, expected) in [
            ("https://registry.cabinpkg.com", true),
            ("http://127.0.0.1:8080", true),
            ("http://[::1]:8080", true),
            ("http://localhost:8080", true),
            ("https://evil.example", false),
            ("https://cabinpkg.com", false),
            ("http://registry.cabinpkg.com", false),
        ] {
            assert_eq!(
                env_token_eligible(origin).unwrap(),
                expected,
                "origin: {origin}"
            );
        }
    }
}
