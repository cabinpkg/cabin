//! Operator surface for the cost governor
//! (`registry/docs/runbook.md`, "The cost governor"): the usage
//! snapshot, read through the admin endpoint so it never starts from a
//! hand-typed `curl`.
//!
//! It needs `CABIN_REGISTRY_TOKEN` (a login-session token whose
//! `verify` scope authenticates the admin endpoint).
//!
//! `CABIN_API_ORIGIN` overrides the API origin for scratch rehearsal
//! deployments (default `https://cabinpkg.com`; https only).  The
//! bearer token goes to whichever origin the operator names here, so
//! a rehearsal uses a session minted on that scratch deployment,
//! never the production one.
//!
//! Ceiling: a proxy-only network is not reachable, the same ceiling
//! the crate's other HTTP clients carry.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::step;

/// The governor's usage snapshot: the operation windows.
#[derive(Deserialize)]
struct Snapshot {
    ops: Vec<OpRow>,
}

#[derive(Deserialize)]
struct OpRow {
    pool: String,
    window: String,
    used: u64,
}

fn origin() -> Result<String> {
    let origin = std::env::var(cabin_env::CABIN_API_ORIGIN)
        .ok()
        .filter(|origin| !origin.is_empty())
        .unwrap_or_else(|| "https://cabinpkg.com".to_owned());
    if !origin.starts_with("https://") {
        bail!("{} must be https", cabin_env::CABIN_API_ORIGIN);
    }
    Ok(origin)
}

/// The admin governor endpoint.
struct Api {
    endpoint: String,
    token: String,
}

impl Api {
    /// # Errors
    ///
    /// If `CABIN_API_ORIGIN` is not https, or the registry token is
    /// missing.
    fn new() -> Result<Self> {
        let token = std::env::var(cabin_env::CABIN_REGISTRY_TOKEN).unwrap_or_default();
        if token.is_empty() {
            bail!(
                "{} is required: mint a login-session token and export it \
                 (registry/docs/runbook.md)",
                cabin_env::CABIN_REGISTRY_TOKEN
            );
        }
        Ok(Self {
            endpoint: format!("{}/api/v1/admin/governor", origin()?),
            token,
        })
    }

    /// The snapshot's body.  Anything but 200 ends the run, naming
    /// what came back - a redirect included, which the agent hands
    /// over unfollowed.
    fn snapshot(&self) -> Result<String> {
        let (status, body) = match crate::agent()
            .get(&self.endpoint)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
        {
            Ok(response) => (response.status(), response.into_string()),
            Err(ureq::Error::Status(status, response)) => (status, response.into_string()),
            Err(error) => return Err(error).context("the governor endpoint request failed"),
        };
        if status != 200 {
            bail!(
                "the usage snapshot answered {status}: {}",
                body.unwrap_or_default()
            );
        }
        body.context("read the usage snapshot")
    }
}

/// Prints the usage snapshot (`cargo registry-governor usage`).
///
/// # Errors
///
/// If the endpoint refuses or answers something that is not a
/// snapshot.
pub fn run() -> Result<()> {
    let api = Api::new()?;
    step(&format!("governor usage snapshot ({})", api.endpoint));
    for line in snapshot_lines(&api)? {
        println!("{line}");
    }
    Ok(())
}

/// The snapshot as the lines [`run`] prints, without its heading.
///
/// The diagnostics bundle prints this under a heading of its own and
/// indents it, which is why the rendering is separate from the
/// printing.
///
/// # Errors
///
/// If the endpoint refuses or answers something that is not a
/// snapshot.
pub fn usage_lines() -> Result<Vec<String>> {
    snapshot_lines(&Api::new()?)
}

fn snapshot_lines(api: &Api) -> Result<Vec<String>> {
    let snapshot: Snapshot =
        serde_json::from_str(&api.snapshot()?).context("parse the usage snapshot")?;
    let mut lines = vec!["ops (used of the UTC-month window):".to_owned()];
    lines.extend(
        snapshot
            .ops
            .iter()
            .map(|row| format!("    {}[{}]: {}", row.pool, row.window, row.used)),
    );
    if snapshot.ops.is_empty() {
        lines.push("    (no window opened yet)".to_owned());
    }
    Ok(lines)
}
