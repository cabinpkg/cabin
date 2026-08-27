//! `cabin login` / `cabin logout`: mint or revoke a registry login
//! session.
//!
//! Both commands resolve the registry the same way the fetch family
//! does (`--index-url`, else the `[registry] index-url` config
//! setting, else the default hosted registry) and key the stored
//! credential on the normalized index origin.  `cabin login` runs
//! GitHub's OAuth device flow and trades the resulting GitHub access
//! token for a short-lived `cabin_ses_` session at the registry's
//! `api` origin (`docs/remote-registry.md`, "Login sessions"); the
//! GitHub token lives only on this invocation's stack - used for
//! exactly the one exchange call, never written to disk, never
//! logged.  `cabin logout` best-effort revokes the stored session and
//! removes it from storage.

use std::io::{IsTerminal as _, Write as _};

use anyhow::{Context as _, Result, bail};
use clap::Args;

use cabin_credentials::{Session, SessionStorage, StoredIn, Token};

use crate::cli::config::resolve_index_source;
use crate::cli::term_verbosity::Reporter;

/// Public client id of the hosted registry's GitHub OAuth App (the
/// same app the registry's own web sign-in uses; `registry/wrangler.jsonc`
/// pins the identical value).  Public by design - the device flow
/// authenticates the *user*, never the client - so it ships in the
/// binary like Cargo ships crates.io's.
///
/// This also bounds interactive login: a registry whose mint pins to a
/// different OAuth app cannot accept grants issued to this one
/// (`docs/remote-registry.md`, "Minting a session token").
const OAUTH_CLIENT_ID: &str = "Ov23liAgmw27EQavKC8H";

/// The GitHub OAuth host the device flow contacts.  Tests override it
/// with `CABIN_GITHUB_OAUTH_URL` to point at a loopback mock.
const GITHUB_OAUTH_URL: &str = "https://github.com";

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
        "sessions only apply to `--index-url` registries",
        true,
    )?
    .url;
    let origin = cabin_credentials::normalize_origin(&index_url)?;
    let session = run_login_flow(&index_url, &origin, reporter)?;
    reporter.status(
        "Login",
        format_args!(
            "session for `{origin}` saved (expires {})",
            session.expires_at
        ),
    );
    Ok(())
}

pub(crate) fn logout(args: &LogoutArgs, reporter: Reporter) -> Result<()> {
    let origin = effective_registry_origin(args.index_url.as_deref(), "cabin logout")?;
    let storage = SessionStorage::from_env();
    let load = storage.load(&origin)?;
    surface_warning(reporter, load.warning);
    if load.session.is_none() {
        // Still ask every backend to remove: an unavailable keychain
        // answers loads with "nothing" while an entry may sit inside,
        // and skipping the removal would let it resurface silently
        // once the keychain recovers.
        let removal = storage.remove(&origin)?;
        warn_keychain_unreachable(removal, reporter);
        reporter.status(
            "Logout",
            format_args!("no session was stored for `{origin}`"),
        );
        return Ok(());
    }
    // Best-effort server-side revocation at the API origin each
    // session was minted for; every failure is tolerated - a 401
    // means the session already expired or was revoked, and logout
    // must always succeed locally.  Skipped outright under offline
    // mode.  Every backend's session is revoked, not just the one
    // `load` prefers: a keychain outage during a past login can
    // leave a second live session behind, and removing it without
    // revocation would orphan it server-side until expiry.
    if !crate::cli::config::effective_offline(false)? {
        for session in storage.load_each(&origin)? {
            if let Ok(api) =
                cabin_registry_api::RegistryApi::new(&session.api_url, Some(session.token))
            {
                let _ = api.revoke_session();
            }
        }
    }
    let removal = storage.remove(&origin)?;
    warn_keychain_unreachable(removal, reporter);
    reporter.status("Logout", format_args!("session for `{origin}` removed"));
    Ok(())
}

/// `cabin logout` could not even ask the keychain: a session stored
/// there survives the removal and would be used again on recovery, so
/// the removal report must not stand unqualified.
fn warn_keychain_unreachable(removal: cabin_credentials::Removal, reporter: Reporter) {
    if removal.keychain_unreachable {
        reporter.warning(format_args!(
            "the platform keychain is unavailable; a session stored there could not be removed \
             and would be used again once the keychain is back - re-run `cabin logout` then"
        ));
    }
}

/// The whole interactive login: discover the registry's `api` origin,
/// run GitHub's device flow, exchange the GitHub token for a session,
/// and store it.  Shared by `cabin login` and the inline offer the
/// authenticated commands make on an interactive terminal.
pub(crate) fn run_login_flow(index_url: &str, origin: &str, reporter: Reporter) -> Result<Session> {
    // Name the destination before anything else, so a user can abort
    // if configuration steered the login somewhere unexpected - and
    // so even the offline refusal names its target.
    reporter.note(format_args!("logging in to `{origin}`"));
    // A session stored for a plain-http non-loopback origin could
    // never be attached (the client refuses cleartext beyond
    // loopback), so refuse the login instead of confusing the next
    // fetch.  Here in the shared flow, so the inline offer cannot
    // mint an unusable credential either.
    if origin.starts_with("http://") && !cabin_credentials::url_is_loopback(origin) {
        bail!(
            "refusing to log in to `{origin}`: tokens are never sent over plain `http` except to \
             loopback hosts; use an `https` registry URL"
        );
    }
    // Confine the device flow's GitHub grant to destinations that may
    // see it.  The grant is minted under the hosted registry's OAuth
    // app and is itself a credential: sent to an arbitrary registry's
    // self-declared `api`, it could be relayed to the hosted mint and
    // traded for a full hosted session behind the user's back.  So
    // interactive login serves exactly the default hosted registry -
    // whose HTTPS `config.json` is taken at its word - and a loopback
    // registry (development and tests), which in turn may only
    // declare a loopback `api` (checked after discovery below).
    let default_origin =
        cabin_credentials::normalize_origin(cabin_core::registry::DEFAULT_INDEX_URL)?;
    if origin != default_origin && !cabin_credentials::url_is_loopback(origin) {
        bail!(
            "refusing to log in to `{origin}`: the login's GitHub grant is issued to the hosted \
             registry's OAuth app and is only ever sent to the hosted registry's API or a \
             loopback API, so interactive login serves only those registries \
             (`docs/remote-registry.md`, \"Minting a session token\")"
        );
    }
    if crate::cli::config::effective_offline(false)? {
        bail!("`cabin login` needs the network to reach github.com and the registry");
    }
    // Discover the registry's `api` origin first, before any user
    // interaction: a registry that cannot accept the session should
    // fail the login up front, not after the browser round-trip.
    // This tokenless open can always bootstrap - `config.json` is
    // public even on an `auth-required` registry, by the protocol's
    // login-bootstrap rule (`docs/remote-registry.md`, "Registry
    // configuration").
    let index = cabin_index_http::HttpIndex::open(index_url, cabin_index_http::HttpClient::new())?;
    let Some(api_url) = index.api() else {
        bail!(
            "registry `{origin}` does not declare an `api` URL in its config.json; logging in \
             needs one to locate the registry API origin"
        );
    };
    let api_url = api_url.to_owned();
    if !login_api_allowed(origin, &api_url) {
        bail!(
            "refusing to log in: loopback registry `{origin}` declares the non-loopback API \
             origin `{api_url}`, which must not receive the login's GitHub token - a loopback \
             index may only name a loopback API"
        );
    }

    let github_url = github_oauth_url();
    let granted = cabin_registry_api::request_device_authorization(&github_url, OAUTH_CLIENT_ID)?;
    // Interaction protocol, not progress: printed to stderr,
    // un-gated, so the code shows under `--quiet` and stdout stays
    // clean.
    eprintln!();
    eprintln!(
        "To authorize this login, open {} and enter the code:",
        granted.verification_uri
    );
    eprintln!();
    eprintln!("    {}", granted.user_code);
    eprintln!();
    if interactive() {
        eprint!("press Enter to open that page in your browser (or open it yourself) ");
        let _ = std::io::stderr().flush();
        if prompt_answer(&mut std::io::stdin().lock()).is_some() {
            open_browser(&granted.verification_uri);
        } else {
            // EOF leaves the prompt line unterminated (no Enter echo).
            eprintln!();
        }
    }
    eprintln!("waiting for the login to be approved on github.com ...");
    // The GitHub access token is a secret used for exactly the one
    // exchange call below and then dropped - never stored, never
    // logged.  GitHub's refresh-token fields are discarded unread
    // inside the poll (Cabin re-runs the device flow instead of
    // refreshing).
    let github_token = cabin_registry_api::poll_device_token(
        &github_url,
        OAUTH_CLIENT_ID,
        &granted,
        std::thread::sleep,
    )?;
    let minted = cabin_registry_api::exchange_login_session(&api_url, &github_token)?;
    drop(github_token);

    let session = Session {
        token: minted.token,
        expires_at: minted.expires_at,
        api_url,
    };
    let storage = SessionStorage::from_env();
    // The sessions this store displaces: the registry keeps one row
    // per mint and revocation deletes only the presented token, so a
    // displaced session left un-revoked would stay live server-side
    // until it expires - and no later `cabin logout` could reach it.
    // Collected before the store, revoked best-effort (like logout)
    // only after the new session is safely stored.
    let displaced = storage.load_each(origin).unwrap_or_default();
    let stored_in = storage.store(origin, &session)?;
    for old in displaced {
        if old.token != session.token
            && let Ok(api) = cabin_registry_api::RegistryApi::new(&old.api_url, Some(old.token))
        {
            let _ = api.revoke_session();
        }
    }
    if stored_in == StoredIn::FileFallback {
        reporter.warning(format_args!(
            "the platform keychain is unavailable; the session was stored in the 0600 \
             credentials file instead"
        ));
    }
    Ok(session)
}

/// When an authenticated command (publish, yank) resolved no usable
/// stored credential, say why and - on an interactive terminal -
/// offer to run the login flow inline, returning the fresh session
/// token so the command proceeds without a restart.  Declining, or a
/// non-interactive run, keeps the "say so and instruct" contract: an
/// *expired* session fails here with the actionable message (the
/// registry's uniform 401 could not name the cause), while no stored
/// session at all proceeds tokenless so the server's own
/// `authentication required` answer stands, exactly as before.
///
/// `user_chosen` carries the same origin-trust contract as
/// [`env_token_eligible`]: the offer is made only for a registry the
/// user named (`--index-url` or user-level config).  A checked-out
/// project's config must not be able to steer where the login flow
/// sends a live GitHub access token - the same rule `cabin login`
/// itself enforces by resolving user-level config only, which this
/// inline entry point would otherwise bypass.
pub(crate) fn offer_interactive_login(
    index_url: &str,
    origin: &str,
    expired_at: Option<&str>,
    user_chosen: bool,
    reporter: Reporter,
) -> Result<Option<Token>> {
    // The advice quotes the full `index_url`, not the normalized
    // origin: for a registry hosted below a path, the origin alone
    // would point `cabin login` at the wrong `config.json`.
    let expired_message = |at: &str| {
        format!(
            "the stored session for `{origin}` has expired (at {at}); run `cabin login \
             --index-url {index_url}` to start a new one"
        )
    };
    if !user_chosen || !interactive() {
        if let Some(at) = expired_at {
            bail!(expired_message(at));
        }
        return Ok(None);
    }
    if let Some(at) = expired_at {
        eprintln!("the stored session for `{origin}` has expired (at {at})");
    } else {
        eprintln!("no credential is stored for `{origin}`");
    }
    eprint!("run `cabin login` for `{origin}` now? [Y/n] ");
    let _ = std::io::stderr().flush();
    let answer = prompt_answer(&mut std::io::stdin().lock());
    if answer.is_none() {
        // EOF (Ctrl-D) or a failed read declines the default yes; it
        // also leaves the prompt line unterminated (no Enter echo).
        eprintln!();
    }
    if !answer.is_some_and(|line| matches!(line.trim(), "" | "y" | "Y" | "yes")) {
        if let Some(at) = expired_at {
            bail!(expired_message(at));
        }
        return Ok(None);
    }
    let session = run_login_flow(index_url, origin, reporter)?;
    Ok(Some(session.token))
}

/// Whether the discovered `api` origin may receive the login's
/// GitHub token.  Mirrors
/// [`crate::cli::trustpub::exchange_api_allowed`]'s rule - the grant
/// is itself a credential, and a loopback index must not fan it out
/// to a non-loopback `api` its config.json happens to declare - but
/// guards a different credential, so loosening one must never
/// silently loosen the other.  A non-loopback origin only reaches
/// this check as the default hosted registry, whose declared `api`
/// is taken at its word.
fn login_api_allowed(origin: &str, api_url: &str) -> bool {
    !cabin_credentials::url_is_loopback(origin) || cabin_credentials::url_is_loopback(api_url)
}

/// Whether this invocation can converse with the user: both stdin
/// (answers) and stderr (prompts) are terminals.
fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// One prompt answer: the line read from `input`, only when the read
/// succeeded and ended in a newline (Enter).  `None` - EOF (Ctrl-D)
/// or a read failure - is an attempted cancellation, never an
/// answer.
fn prompt_answer(input: &mut impl std::io::BufRead) -> Option<String> {
    let mut line = String::new();
    (input.read_line(&mut line).is_ok() && line.ends_with('\n')).then_some(line)
}

/// The GitHub OAuth host: the `CABIN_GITHUB_OAUTH_URL` test override
/// when set and non-empty, else github.com.
fn github_oauth_url() -> String {
    std::env::var(cabin_env::CABIN_GITHUB_OAUTH_URL)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| GITHUB_OAUTH_URL.to_owned())
}

/// Open `url` in the user's browser, best-effort: only ever called
/// after explicit consent (Enter at the prompt), and a failure is
/// silent - the URL is already printed for opening by hand.  The
/// value is server-supplied, so only an `https` URL reaches process
/// execution; anything else stays print-only.
fn open_browser(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/c", "start", "", url]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    // Drop the credential env vars every other Cabin subprocess
    // drops: a browser helper has no business inheriting them.
    let _ = command
        .env_remove(cabin_env::CABIN_REGISTRY_TOKEN)
        .env_remove(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Resolve the credential to attach to sparse-HTTP requests for
/// `index_url`: the `CABIN_REGISTRY_TOKEN` env override first (for
/// eligible origins, see [`env_token_eligible`]), then the stored
/// session for the URL's origin.  An expired stored session is
/// surfaced as a warning and the client stays tokenless - reads on
/// the hosted registry work anonymously, and an `auth-required`
/// registry's 401 then carries the login advice.  Without any
/// credential the read path is byte-identical to the
/// unauthenticated flow.
pub(crate) fn registry_auth_for_index_url(
    index_url: &str,
    user_chosen: bool,
    reporter: Reporter,
) -> Result<Option<cabin_index_http::RegistryAuth>> {
    let origin = cabin_credentials::normalize_origin(index_url)?;
    let eligible = env_token_eligible(&origin, user_chosen)?;
    let lookup = cabin_credentials::lookup_token(&origin, eligible)?;
    surface_warning(reporter, lookup.warning);
    let Some(token) = lookup.token else {
        if let Some(at) = lookup.expired_at {
            warn_session_expired(index_url, &origin, &at, reporter);
        } else {
            warn_if_env_token_withheld(index_url, &origin, eligible, reporter);
        }
        return Ok(None);
    };
    Ok(Some(cabin_index_http::RegistryAuth::for_index_url(
        index_url, token,
    )?))
}

fn surface_warning(reporter: Reporter, warning: Option<String>) {
    if let Some(warning) = warning {
        reporter.warning(format_args!("{warning}"));
    }
}

/// The stored session exists but its expiry has passed: name the
/// cause and the fix, since the registry's uniform 401 cannot.  The
/// advice quotes the full `index_url` (see `offer_interactive_login`
/// on why the origin alone would misdirect a path-hosted registry).
pub(crate) fn warn_session_expired(
    index_url: &str,
    origin: &str,
    expired_at: &str,
    reporter: Reporter,
) {
    reporter.warning(format_args!(
        "the stored session for `{origin}` has expired (at {expired_at}); run `cabin login \
         --index-url {index_url}` to start a new one"
    ));
}

/// Say so when `CABIN_REGISTRY_TOKEN` is set, this origin was not
/// allowed to use it, and nothing else supplied a credential either.
/// The run is about to fail with the registry's generic
/// "authentication required", which advises `cabin login` but never
/// reveals that the token the user *did* export was deliberately
/// withheld - the one question they would otherwise be left with.
///
/// Deliberately silent once a stored credential answered: nothing was
/// lost there, and a shell that exports the variable globally would
/// otherwise warn on every command against every other registry.
pub(crate) fn warn_if_env_token_withheld(
    index_url: &str,
    origin: &str,
    eligible: bool,
    reporter: Reporter,
) {
    if eligible || !env_token_is_set() {
        return;
    }
    reporter.warning(format_args!(
        "CABIN_REGISTRY_TOKEN is set but was not used for `{origin}`: the variable carries no \
         origin key, so it serves only the default registry and a loopback registry you named \
         yourself - run `cabin login --index-url {index_url}` to store a session for this origin"
    ));
}

/// Whether the override is present at all, by the same empty-is-unset
/// rule `cabin-credentials` applies when it reads the value.
fn env_token_is_set() -> bool {
    std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).is_some_and(|value| !value.is_empty())
}

/// Whether the `CABIN_REGISTRY_TOKEN` environment override may serve
/// `origin` (a normalized origin string).  The override carries no
/// origin key of its own, and an invocation's index origin can come
/// from project-level config or `[source-replacement]` - inputs a
/// checked-out project controls - so an unrestricted override would
/// let any built project route the credential to an origin of its
/// choosing.
///
/// It therefore serves the default hosted registry (its intended CI
/// consumer) unconditionally, and a loopback origin (local testing)
/// only when `user_chosen` says the *user* named it rather than the
/// tree being built - see
/// [`index_origin_user_chosen`](crate::cli::config::index_origin_user_chosen).
/// Every other registry uses stored sessions, which are origin-keyed
/// by construction.
///
/// Kept separate from [`crate::cli::trustpub::exchange_origin_eligible`]
/// although the two predicates now read alike: they guard different
/// credentials (this stored login token vs. the run's OIDC JWT), and
/// loosening one must never silently loosen the other.
pub(crate) fn env_token_eligible(origin: &str, user_chosen: bool) -> Result<bool> {
    let default_origin =
        cabin_credentials::normalize_origin(cabin_core::registry::DEFAULT_INDEX_URL)?;
    Ok(origin == default_origin || (cabin_credentials::url_is_loopback(origin) && user_chosen))
}

/// Resolve the registry origin `cabin login` / `cabin logout`
/// operate on: apply the documented index-source precedence
/// (`--index-url`, else config, else the default registry), and
/// reject index sources that cannot carry a token (a local path).
fn effective_registry_origin(cli_index_url: Option<&str>, command: &str) -> Result<String> {
    let url = effective_registry_index_url(
        cli_index_url,
        command,
        "sessions only apply to `--index-url` registries",
        true,
    )?
    .url;
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
/// `[source-replacement]`) must not be able to steer where a minted
/// credential is stored - the session would go to whatever origin the
/// project picked.  Reads key stored sessions on the effective origin
/// too, so a project-steered read simply finds no credential.
pub(crate) fn effective_registry_index_url(
    cli_index_url: Option<&str>,
    command: &str,
    local_path_reason: &str,
    credential_command: bool,
) -> Result<EffectiveRegistryIndex> {
    // An explicit `--index-url` needs no config fallback: skip
    // discovery entirely so an unrelated broken config file or
    // manifest cannot fail the command, and key the session on
    // exactly the origin the user named.
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
    // session must be keyed on the origin the later fetch will
    // actually contact.
    let locator = crate::cli::config::index_source_kind_to_locator(&source.kind);
    let resolution = crate::cli::patch::apply_source_replacement(locator, &config, false)?;
    let user_chosen = crate::cli::config::index_origin_user_chosen(&source, &resolution);
    match resolution.resolved {
        cabin_core::SourceLocator::IndexPath { path } => bail!(
            "`{command}` requires an HTTP registry, but the effective index source is the local \
             path `{path}`; {local_path_reason}"
        ),
        cabin_core::SourceLocator::IndexUrl { url } => Ok(EffectiveRegistryIndex {
            url,
            user_chosen,
            from_cli: cli_index_url.is_some(),
        }),
    }
}

/// The HTTP registry a command targets, plus how its origin was
/// chosen.  The two credential gates ask different questions of the
/// same origin, so both answers travel with it.
pub(crate) struct EffectiveRegistryIndex {
    pub url: String,
    /// The *user* chose this origin - see
    /// [`index_origin_user_chosen`](crate::cli::config::index_origin_user_chosen).
    /// Gates the `CABIN_REGISTRY_TOKEN` override.
    pub user_chosen: bool,
    /// Stricter: this invocation's own `--index-url` named it, so not
    /// even the user's config file counts.  Gates the
    /// trusted-publishing exchange
    /// (`crate::cli::trustpub::publish_credential`).
    pub from_cli: bool,
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
/// hostile checkout cannot steer where a session is stored.
fn user_level_config() -> Result<cabin_config::EffectiveConfig> {
    let inputs = cabin_config::ConfigDiscoveryInputs::from_process(None);
    let discovery =
        cabin_config::discover_config_files(&inputs).context("failed to load Cabin config")?;
    Ok(cabin_config::merge_loaded_files(discovery.loaded_files))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The api-side confinement: a loopback index may only fan the
    /// GitHub grant out to a loopback `api`; the default hosted
    /// registry's declared `api` is taken at its word.
    #[test]
    fn login_api_gate_confines_a_loopback_index_to_a_loopback_api() {
        assert!(login_api_allowed(
            "http://127.0.0.1:8080",
            "http://127.0.0.1:9090"
        ));
        assert!(!login_api_allowed(
            "http://127.0.0.1:8080",
            "https://evil.example"
        ));
        assert!(login_api_allowed(
            "https://registry.cabinpkg.com",
            "https://cabinpkg.com"
        ));
    }

    /// Prompt reads only count when Enter terminated them: EOF is an
    /// attempted cancellation, not an answer - at the login offer it
    /// would otherwise read as the default yes, and at the browser
    /// prompt as consent to open it.
    #[test]
    fn prompt_answers_require_a_newline() {
        assert_eq!(prompt_answer(&mut &b"\n"[..]).as_deref(), Some("\n"));
        assert_eq!(prompt_answer(&mut &b"n\n"[..]).as_deref(), Some("n\n"));
        assert!(
            prompt_answer(&mut &b""[..]).is_none(),
            "EOF is not an answer"
        );
        assert!(
            prompt_answer(&mut &b"y"[..]).is_none(),
            "EOF mid-line is not an answer"
        );
    }

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
    /// registry and loopback origins; any other origin is ineligible
    /// however it was chosen.
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
                env_token_eligible(origin, true).unwrap(),
                expected,
                "origin: {origin}"
            );
        }
    }

    /// A loopback origin the user did not choose - project-level
    /// config or a `[source-replacement]` hop picked it - never
    /// receives the origin-key-less `CABIN_REGISTRY_TOKEN`.  The
    /// default hosted registry stays eligible either way: a project
    /// cannot steer where that origin's traffic goes.
    #[test]
    fn a_loopback_origin_the_user_did_not_choose_is_ineligible() {
        for origin in [
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
            "http://localhost:8080",
        ] {
            assert!(
                !env_token_eligible(origin, false).unwrap(),
                "origin: {origin}"
            );
        }
        assert!(env_token_eligible("https://registry.cabinpkg.com", false).unwrap());
    }
}
