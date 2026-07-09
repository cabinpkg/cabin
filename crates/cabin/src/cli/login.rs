//! `cabin login` / `cabin logout`: store or remove a registry token
//! for the experimental remote-registry client (`-Z remote-registry`).
//!
//! Both commands resolve the registry the same way the fetch family
//! does (`--index-url`, else the `[registry] index-url` config
//! setting) and key the stored credential on the normalized index
//! origin.  The token itself only ever flows stdin →
//! `cabin-credentials`; it is never echoed, logged, or printed back.

use std::io::IsTerminal as _;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_core::{ExperimentalFeature, ExperimentalFeatures};
use cabin_credentials::{CredentialStore, Token};

use crate::cli::config::resolve_index_source;
use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct LoginArgs {
    /// Sparse HTTP index URL of the registry to log in to.  Falls
    /// back to the `[registry] index-url` config setting.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct LogoutArgs {
    /// Sparse HTTP index URL of the registry to log out from.  Falls
    /// back to the `[registry] index-url` config setting.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,
}

pub(crate) fn login(
    args: &LoginArgs,
    reporter: Reporter,
    features: &ExperimentalFeatures,
) -> Result<()> {
    let origin = effective_registry_origin(args.index_url.as_deref(), features, "cabin login")?;
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
    // The token-creation page is derived from the index origin by
    // the protocol's origin convention; `config.json` is deliberately
    // not consulted because on an auth-required registry it is itself
    // behind auth.
    reporter.note(format_args!("visit {origin}/me to create a token"));
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

pub(crate) fn logout(
    args: &LogoutArgs,
    reporter: Reporter,
    features: &ExperimentalFeatures,
) -> Result<()> {
    let origin = effective_registry_origin(args.index_url.as_deref(), features, "cabin logout")?;
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
/// `index_url`: the `CABIN_REGISTRY_TOKEN` env override first, then
/// the `credentials.toml` entry for the URL's origin.  Only consulted
/// under `-Z remote-registry`; without the feature the client stays
/// tokenless and the read path is byte-identical to the
/// unauthenticated flow.
pub(crate) fn registry_auth_for_index_url(
    index_url: &str,
    features: &ExperimentalFeatures,
    reporter: Reporter,
) -> Result<Option<cabin_index_http::RegistryAuth>> {
    if !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
        return Ok(None);
    }
    let origin = cabin_credentials::normalize_origin(index_url)?;
    let lookup = cabin_credentials::lookup_token(&origin)?;
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

/// Resolve the registry origin `cabin login` / `cabin logout`
/// operate on: gate on `-Z remote-registry`, apply the documented
/// index-source precedence (`--index-url`, else config), and reject
/// index sources that cannot carry a token (none, or a local path).
fn effective_registry_origin(
    cli_index_url: Option<&str>,
    features: &ExperimentalFeatures,
    command: &str,
) -> Result<String> {
    let url = effective_registry_index_url(
        cli_index_url,
        features,
        command,
        "tokens only apply to `--index-url` registries",
    )?;
    Ok(cabin_credentials::normalize_origin(&url)?)
}

/// Resolve the HTTP index URL a remote-registry command targets:
/// gate on `-Z remote-registry`, apply the documented index-source
/// precedence (`--index-url`, else config, with
/// `[source-replacement]`), and reject an absent index or a local
/// path - `local_path_reason` finishes the local-path error with the
/// command's own justification.
pub(crate) fn effective_registry_index_url(
    cli_index_url: Option<&str>,
    features: &ExperimentalFeatures,
    command: &str,
    local_path_reason: &str,
) -> Result<String> {
    if !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
        bail!(cabin_core::registry::remote_registry_field_error(command));
    }
    // An explicit `--index-url` needs no config fallback: skip
    // discovery entirely so an unrelated broken config file or
    // manifest cannot fail the command, and key the token on exactly
    // the origin the user named.
    let config = if cli_index_url.is_some() {
        cabin_config::EffectiveConfig::default()
    } else {
        effective_config_for_cwd()?
    };
    let Some(source) = resolve_index_source(None, cli_index_url, &config)? else {
        bail!("`{command}` requires --index-url or a `[registry] index-url` config setting")
    };
    // Mirror the fetch pipeline: a config-supplied registry source is
    // subject to `[source-replacement]`, so the token must be keyed
    // on the origin the later fetch will actually contact.
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

    /// Without `-Z remote-registry` both commands fail with the
    /// standard experimental-feature wording before touching config
    /// or the credential store.
    #[test]
    fn origin_resolution_gates_on_the_feature() {
        for command in ["cabin login", "cabin logout"] {
            let err = effective_registry_origin(
                Some("https://registry.example.com"),
                &ExperimentalFeatures::default(),
                command,
            )
            .unwrap_err();
            let message = err.to_string();
            assert!(message.contains(command), "{message}");
            assert!(message.contains("-Z remote-registry"), "{message}");
        }
    }
}
