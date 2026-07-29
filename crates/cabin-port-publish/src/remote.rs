//! Remote publication through the existing `cabin publish` flow.
//!
//! Each preflighted scratch package uploads via
//! `cabin -Z remote-registry publish --manifest-path ... --index-url ...`,
//! so staging, scoped-name gates, credential lookup, the registry's
//! `config.json` API discovery, publish lints, and the framed upload
//! all run exactly the code path an ordinary publish runs - this
//! tool adds only the ordering and the rate-limit pacing.
//!
//! Versions are never skipped based on the public index: pending
//! (not yet verified) versions are invisible there, so the only
//! correct dedupe is the registry's own byte-identical idempotency -
//! `cabin publish` reports a no-op for identical bytes and fails on
//! divergent ones (published versions are immutable).
//!
//! A serial bulk publish can outrun the registry's per-token publish
//! bucket, and every attempt - byte-identical no-ops included -
//! charges it, so a rate-limited package is retried here after the
//! server-advertised delay instead of failing the whole run.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::plan::PortConversion;

/// Attempts per package. The default publish bucket refills one token
/// per minute, so a drained bucket needs at most one advertised wait
/// per token; a handful of attempts rides that out without masking a
/// persistently failing package.
const MAX_PUBLISH_ATTEMPTS: u32 = 5;

/// Fallback delay when the diagnostic carries no seconds ("try again
/// later"), matching the default class's one-token refill time.
const DEFAULT_RETRY_DELAY_SECS: u64 = 60;

/// Ceiling on any advertised delay, so a corrupt or hostile value can
/// never stall CI for hours.
const MAX_RETRY_DELAY_SECS: u64 = 300;

/// Upload every conversion, in publication order.
///
/// Every upload passes `--new-revision`: the committed recipes are
/// the source of truth, so a recipe change that reaches this tool is
/// the deliberate intent to respin the published version - identical
/// bytes still no-op through the registry's idempotency, and changed
/// bytes become a new packaging revision of the same upstream
/// version.
///
/// # Errors
/// Returns an error when a `cabin publish` invocation cannot start
/// or exits non-zero with every retry spent.
pub fn publish_all(
    conversions: &[PortConversion],
    package_dirs: &[&Path],
    index_url: &str,
    cabin: &Path,
) -> Result<()> {
    for (conversion, package_dir) in conversions.iter().zip(package_dirs) {
        publish_one(conversion, package_dir, index_url, cabin)?;
    }
    Ok(())
}

fn publish_one(
    conversion: &PortConversion,
    package_dir: &Path,
    index_url: &str,
    cabin: &Path,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        // Captured rather than inherited so the rate-limit diagnostic
        // is inspectable; the transcript is relayed below either way.
        // `--color never` (the highest-precedence color control) keeps
        // an inherited CABIN_TERM_COLOR or a user config's
        // `term.color = "always"` from ANSI-wrapping the diagnostic
        // the parser anchors on.
        let output = Command::new(cabin)
            .arg("--color")
            .arg("never")
            .arg("-Z")
            .arg("remote-registry")
            .arg("publish")
            .arg("--new-revision")
            .arg("--manifest-path")
            .arg(package_dir.join("cabin.toml"))
            .arg("--index-url")
            .arg(index_url)
            .output()
            .with_context(|| format!("running {} publish", cabin.display()))?;
        let _ = std::io::stdout().write_all(&output.stdout);
        let _ = std::io::stderr().write_all(&output.stderr);
        if output.status.success() {
            return Ok(());
        }
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        if attempt < MAX_PUBLISH_ATTEMPTS
            && let Some(delay_secs) = rate_limited_retry_delay(&stderr_text)
        {
            // One extra second over the advertised wait: the server
            // rounds its own estimate up, but the bucket timestamps
            // and this clock are not the same clock.
            let delay_secs = delay_secs + 1;
            eprintln!(
                "note: the registry rate limited {} {}; retrying in {delay_secs}s (attempt \
                 {attempt} of {MAX_PUBLISH_ATTEMPTS})",
                conversion.scoped_name.as_str(),
                conversion.published_version,
            );
            std::thread::sleep(Duration::from_secs(delay_secs));
            continue;
        }
        bail!(
            "publishing {} {} failed ({})",
            conversion.scoped_name.as_str(),
            conversion.published_version,
            output.status,
        );
    }
}

/// The retry delay a rate-limited `cabin publish` advertised, when its
/// stderr carries the registry client's 429 diagnostic; `None` for
/// every other failure.  The subprocess boundary erases the typed
/// error, so the stable diagnostic text is the only signal - the
/// dev-dependency test below pins this parser to the actual
/// `cabin-registry-api` rendering so the two cannot drift silently.
/// Anchored to the complete line shape, never a substring scan:
/// other variants render server-controlled detail text verbatim, and
/// a detail merely *containing* the phrase must not turn an auth or
/// server error into a retry loop.  A server whose detail *equals*
/// the phrase renders a byte-identical line and cannot be told apart
/// over text - accepted: the registry is the operator's own service,
/// and [`MAX_PUBLISH_ATTEMPTS`] with the [`MAX_RETRY_DELAY_SECS`] cap
/// bounds what such a lie can cost to minutes.
fn rate_limited_retry_delay(stderr: &str) -> Option<u64> {
    let suffix = stderr.lines().find_map(|line| {
        line.trim_ascii()
            .strip_prefix("error: ")?
            .strip_prefix("the registry rate limited this request")
    })?;
    // Only the client's two complete renderings qualify; any other
    // suffix is some different diagnostic that happens to share the
    // prefix and must fail fast, never inherit the fallback delay.
    let advertised = if suffix == "; try again later" {
        DEFAULT_RETRY_DELAY_SECS
    } else {
        let rest = suffix.strip_prefix("; try again in ")?;
        let (digits, unit) = rest.split_once(' ')?;
        if unit != "seconds" && unit != "second" {
            return None;
        }
        // Plain digits only: `u64::parse` alone would also accept a
        // leading `+`, which the client's u64 rendering never emits.
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()?
    };
    Some(advertised.min(MAX_RETRY_DELAY_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_advertised_delay() {
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in 50 seconds\n"
            ),
            Some(50)
        );
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in 1 second\n"
            ),
            Some(1)
        );
    }

    #[test]
    fn falls_back_when_no_seconds_are_advertised() {
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again later\n"
            ),
            Some(DEFAULT_RETRY_DELAY_SECS)
        );
    }

    #[test]
    fn caps_an_absurd_advertised_delay() {
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in 99999 seconds\n"
            ),
            Some(MAX_RETRY_DELAY_SECS)
        );
    }

    #[test]
    fn other_failures_never_retry() {
        assert_eq!(rate_limited_retry_delay(""), None);
        assert_eq!(
            rate_limited_retry_delay(
                "error: `cabin-ports/zlib 1.3.1` is already published with different bytes; \
                 published versions are immutable - bump the version and publish again\n"
            ),
            None
        );
        // A server-controlled detail embedding the phrase renders
        // behind other text; the anchored match must not turn such a
        // non-429 failure into a retry.
        assert_eq!(
            rate_limited_retry_delay(
                "error: publish forbidden: the registry rate limited this request; try again \
                 in 1 second\n"
            ),
            None
        );
        // Sharing the prefix is not enough either: a suffix that is
        // not one of the client's two complete renderings fails fast
        // instead of inheriting the fallback delay.
        assert_eq!(
            rate_limited_retry_delay("error: the registry rate limited this request permanently\n"),
            None
        );
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in 5 minutes\n"
            ),
            None
        );
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in soon seconds\n"
            ),
            None
        );
        assert_eq!(
            rate_limited_retry_delay(
                "error: the registry rate limited this request; try again in +50 seconds\n"
            ),
            None
        );
    }

    #[test]
    fn the_parser_matches_the_registry_client_diagnostic() {
        // Pin against the real rendering: if cabin-registry-api ever
        // rewords its 429 diagnostic, this fails here instead of the
        // retry silently never firing in CI.
        let rendered = cabin_registry_api::RegistryApiError::RateLimited {
            retry_after_secs: Some(50),
        }
        .to_string();
        assert_eq!(
            rate_limited_retry_delay(&format!("error: {rendered}\n")),
            Some(50)
        );
        let vague = cabin_registry_api::RegistryApiError::RateLimited {
            retry_after_secs: None,
        }
        .to_string();
        assert_eq!(
            rate_limited_retry_delay(&format!("error: {vague}\n")),
            Some(DEFAULT_RETRY_DELAY_SECS)
        );
    }
}
