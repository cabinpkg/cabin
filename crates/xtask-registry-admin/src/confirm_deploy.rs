//! The deploy job's "Confirm the deployed version" step
//! (`registry/docs/runbook.md`, "Deploy skew"). `wrangler deploy`
//! exiting 0 says its API calls succeeded, not which version takes the
//! traffic, so the Deploy step tags its version with the run attempt
//! (`--tag`, the `workers/tag` annotation) and this passes only once
//! Cloudflare's latest deployment routes 100% of the traffic to one
//! version carrying that tag. The reads are retried, six in all and ten
//! seconds apart, so a transient API failure does not fail a good
//! deploy.
//!
//! Ceiling: this reads the control plane's deployment record, not an
//! edge response. The Worker answers no request with its version, and
//! `/healthz` and the generation header read the same from every
//! version, so no request can tell the new code from the old.
//!
//! Wrangler authenticates from `CLOUDFLARE_API_TOKEN` and
//! `CLOUDFLARE_ACCOUNT_ID`; both reads need only the deploy token's
//! Workers Scripts scope.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::{output, wrangler};

const ATTEMPTS: u32 = 6;
const PAUSE: Duration = Duration::from_secs(10);

/// Confirms the version tagged `tag` serves all traffic.
///
/// # Errors
///
/// If no read within the retry budget shows it serving: the error is
/// also printed as a `Registry deploy unconfirmed` error annotation.
pub fn run(tag: &str) -> Result<()> {
    let mut attempt = 1;
    loop {
        match confirm(tag) {
            Ok(version) => {
                println!("version {version} carries this run's tag {tag} and takes all traffic");
                return Ok(());
            }
            Err(error) if attempt < ATTEMPTS => {
                eprintln!(
                    "attempt {attempt}/{ATTEMPTS}: {error:#}; reading again in {}s",
                    PAUSE.as_secs()
                );
                std::thread::sleep(PAUSE);
                attempt += 1;
            }
            Err(error) => {
                println!(
                    "::error title=Registry deploy unconfirmed::{error:#}. The Deploy step \
                     exited 0, but Cloudflare does not show this run's version serving all \
                     traffic. Rerun this job, which redeploys under a fresh tag (or stays green \
                     when superseded); if it fails again, read `node_modules/.bin/wrangler \
                     deployments list` from registry/ and run the smoke checks \
                     (registry/docs/runbook.md, \"Deploy skew\")."
                );
                return Err(error);
            }
        }
    }
}

/// The serving version's id, when its tag is `tag`.
fn confirm(tag: &str) -> Result<String> {
    let version = serving(&json(&["deployments", "status", "--json"])?)?;
    let detail = json(&["versions", "view", &version, "--json"])?;
    match tag_of(&detail) {
        Some(found) if found == tag => Ok(version),
        found => bail!(
            "the serving version {version} is tagged {}, not this run's {tag}",
            found.unwrap_or("<none>")
        ),
    }
}

fn json(arguments: &[&str]) -> Result<Value> {
    let text = output(&mut wrangler(arguments))?;
    serde_json::from_str(&text).with_context(|| format!("parse `wrangler {}`", arguments.join(" ")))
}

/// The one version a deployment routes all traffic to.
fn serving(deployment: &Value) -> Result<String> {
    let Some(versions) = deployment.get("versions").and_then(Value::as_array) else {
        bail!("the latest deployment lists no versions");
    };
    let [only] = versions.as_slice() else {
        bail!(
            "the latest deployment splits traffic across {} versions",
            versions.len()
        );
    };
    let Some(id) = only.get("version_id").and_then(Value::as_str) else {
        bail!("the latest deployment's version has no id");
    };
    if only.get("percentage").and_then(Value::as_u64) != Some(100) {
        bail!("the latest deployment routes less than all traffic to {id}");
    }
    Ok(id.to_owned())
}

fn tag_of(version: &Value) -> Option<&str> {
    version
        .pointer("/annotations/workers~1tag")
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_single_version_at_full_traffic_is_the_serving_one() {
        let deployment = json!({
            "strategy": "percentage",
            "versions": [{ "version_id": "v-new", "percentage": 100 }],
        });
        assert_eq!(serving(&deployment).unwrap(), "v-new");
    }

    #[test]
    fn a_split_or_partial_deployment_names_no_serving_version() {
        for versions in [
            json!([
                { "version_id": "v-new", "percentage": 50 },
                { "version_id": "v-old", "percentage": 50 },
            ]),
            json!([{ "version_id": "v-new", "percentage": 50 }]),
            json!([{ "version_id": "v-new" }]),
            json!([{ "percentage": 100 }]),
            json!([]),
        ] {
            assert!(
                serving(&json!({ "versions": versions })).is_err(),
                "{versions}"
            );
        }
        assert!(serving(&json!({})).is_err());
    }

    #[test]
    fn the_tag_is_read_from_the_version_annotation() {
        let version = json!({
            "id": "v-new",
            "annotations": { "workers/tag": "123.1", "workers/triggered_by": "upload" },
        });
        assert_eq!(tag_of(&version), Some("123.1"));
        assert_eq!(tag_of(&json!({ "annotations": {} })), None);
    }
}
