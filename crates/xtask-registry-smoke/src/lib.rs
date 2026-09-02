//! The registry smoke test against local `wrangler dev`, ported
//! one-to-one from `registry/scripts/smoke.sh`: two dev instances
//! (the registry role on the default host, the website role via
//! `--host cabinpkg.com`) over one local D1/R2 state, plus a local
//! export-API mock and a local GitHub mock, driven through the same
//! step sequence, diagnostics and timing as the shell.
//!
//!   `cargo registry-smoke`                                   healthz + 401 only
//!   `CABIN_REGISTRY_SMOKE_TOKEN=cabin_smoke cargo registry-smoke`   full run
//!
//! The token is seeded into the local D1 state before the checks, so
//! any `cabin_...` value works.  Local-only: state lives in
//! `.wrangler/`, never a deployed environment.

pub mod bytes;
pub mod context;
pub mod legs;
pub mod servers;
mod text;

use std::path::Path;

use anyhow::{Context as _, Result};
use tempfile::TempDir;
use xtask_registry_admin::registry_dir;

use crate::bytes::replace_all;
use crate::context::Smoke;
use crate::legs::{anonymous, blobs, claims, finale, publish, revisions, session, signin};
use crate::servers::{DevServers, Ports};
use crate::text::read;

pub use xtask_registry_admin::step;

/// The whole run, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run() -> Result<()> {
    let registry = registry_dir();
    let ports = Ports::from_env();
    // L72: unset and empty are the same thing, and the empty token is
    // what selects the unauthenticated run.
    let token = std::env::var("CABIN_REGISTRY_SMOKE_TOKEN").unwrap_or_default();

    servers::apply_migrations()?;
    if !token.is_empty() {
        // L202: the verifier's and no-verify credentials are the
        // publisher's by suffix, which is also what
        // `Smoke::as_verifier` / `Smoke::as_ci_publisher` present.
        servers::seed_tokens(
            &token,
            &format!("{token}-verify"),
            &format!("{token}-noverify"),
        )?;
    }
    // Declared before the servers so it outlives them: the mocks are
    // reading files from it right up until they are killed.
    let mock_dir = TempDir::new().context("creating the mock directory")?;
    servers::export_dump(mock_dir.path())?;
    let mut servers = DevServers::start(&ports, mock_dir.path())?;

    let github_port = port("SMOKE_GITHUB_PORT", &ports.github)?;
    let mut smoke = Smoke::new(
        port("SMOKE_PORT", &ports.registry)?,
        port("SMOKE_WEB_PORT", &ports.web)?,
        github_port,
        token.clone(),
    );
    legs(
        &mut smoke,
        &mut servers,
        &registry,
        mock_dir.path(),
        &token,
        github_port,
    )
    .inspect_err(|_| servers.dump_log_tails())
}

/// Every leg, so that `run` still holds the servers when one fails: a
/// leg's error names the request, but why the Worker answered it that
/// way is only in the dev-server logs, which `DevServers` takes with it.
fn legs(
    smoke: &mut Smoke,
    servers: &mut DevServers,
    registry: &Path,
    mock_dir: &Path,
    token: &str,
    github_port: u16,
) -> Result<()> {
    anonymous::run(smoke, registry, github_port)?;

    if token.is_empty() {
        // L655-659: the only success path that is not the tail.
        step("CABIN_REGISTRY_SMOKE_TOKEN not set; skipping authenticated checks");
        println!("smoke OK");
        return Ok(());
    }
    tokened(smoke, servers, registry, mock_dir, token, github_port)
}

/// Everything from L663 on, which only a seeded token reaches.
fn tokened(
    smoke: &mut Smoke,
    servers: &mut DevServers,
    registry: &Path,
    mock_dir: &Path,
    token: &str,
    github_port: u16,
) -> Result<()> {
    anonymous::verifier_exchange_surface(smoke)?;
    anonymous::login_session_surface(smoke, &format!("{token}-verify"))?;
    let setup = session::setup(registry)?;
    session::read_plane(smoke, &setup)?;
    let cookie = session::session_plane(smoke)?;
    claims::run(smoke, &cookie, github_port)?;
    signin::run(smoke, github_port)?;

    let work = setup.work.path();
    publish::run(
        smoke,
        &publish::PublishInputs {
            work,
            fixture_archive: &setup.fixture_archive,
            fixture_metadata: &setup.fixture_metadata,
            verifier_bin: &setup.verifier_bin,
            session_cookie: &cookie,
            scope: session::SCOPE,
            name: session::NAME,
            version: session::VERSION,
            rev: &setup.rev,
            blob_hash: &setup.blob_hash,
            publish_path: &setup.publish_path,
            package_path: &setup.package_path,
            artifact_path: &setup.artifact_path,
        },
    )?;

    // The two files the publish span left in `$work`, which the shell's
    // later phases re-read by path (L1307, L1373).
    let publish_bin = read(&work.join("publish.bin"))?;
    let verdict_verified = read(&work.join("verdict-verified.json"))?;
    let outputs = revisions::run(
        smoke,
        &revisions::RevisionInputs {
            scope: session::SCOPE,
            name: session::NAME,
            version: session::VERSION,
            fixture_archive: &setup.fixture_zip,
            fixture_metadata: &setup.fixture_meta,
            publish_path: &setup.publish_path,
            package_path: &setup.package_path,
            artifact_path: &setup.artifact_path,
            publish_bin: &publish_bin,
            verdict_verified: &verdict_verified,
            session_cookie: &cookie,
            token,
        },
    )?;

    blobs::run(
        smoke,
        &blobs::BlobInputs {
            scope: session::SCOPE,
            name: session::NAME,
            version: session::VERSION,
            version2: &outputs.version2,
            rev: &setup.rev,
            blob_hash: &setup.blob_hash,
            artifact_path: &setup.artifact_path,
            package_path: &setup.package_path,
            publish2_path: &outputs.publish2_path,
            artifact2_path: &outputs.artifact2_path,
            verdict2_path: &outputs.verdict2_path,
            // `$work/withdep-0.2.1.json` (L1590): the same textual
            // replace the second-version fixtures were framed from.
            metadata2: &replace_all(&setup.fixture_meta, b"0.2.0", b"0.2.1"),
            publish2: &outputs.publish2_bin,
            archive: &setup.fixture_zip,
            publish: &publish_bin,
            yank: revisions::YANKED,
            session_cookie: &cookie,
            work,
        },
    )?;

    finale::run(
        smoke,
        &mut finale::FinaleInputs {
            servers,
            work,
            mock_dir,
            verifier_bin: &setup.verifier_bin,
            scope: session::SCOPE,
            name: session::NAME,
            fixture_metadata: &setup.fixture_meta,
            blob_hash: &setup.blob_hash,
            artifact_path: &setup.artifact_path,
            publish_path: &setup.publish_path,
            publish_body: &publish_bin,
            session_cookie: &cookie,
            token,
        },
    )
}

/// `Ports` keeps the shell's raw strings, where [`Smoke`] builds its
/// bases from numbers.  Parsed after the servers are up, so a junk port
/// still fails in wrangler first, exactly as it did in the shell.
fn port(name: &str, value: &str) -> Result<u16> {
    value
        .parse()
        .with_context(|| format!("{name}={value} is not a port number"))
}
