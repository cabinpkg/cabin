//! The OIDC admission control, post-migration coverage with no shell
//! ancestor: the global cache-bypass budget on unknown-kid JWKS
//! refetches, then the per-client-IP gate on the two public OIDC
//! endpoints (`registry/wrangler.jsonc`, the ratelimit bindings).
//! Deliberately the run's LAST leg: every local request shares one
//! admission bucket (no `CF-Connecting-IP` off the edge), so the
//! exhaustion burst would answer any later OIDC request with the 429
//! for up to a minute.  The budget arithmetic below counts against a
//! full bucket, which holds twice over: the concurrency leg's dev
//! restart starts fresh in-process limiter state, and the legs since
//! the run's last OIDC request (reconciles, download waves) outlast
//! the limiter's minute anyway.

use anyhow::{Result, bail};

use crate::context::{Base, Smoke};
use crate::legs::anonymous::{TRUSTPUB_TOKENS_PATH, uniform_401_with};
use crate::step;
use crate::text::{capture, grep_lines, text};

/// A structurally valid JWT under a kid no JWKS ever named - the
/// header decodes to `{"alg":"RS256","kid":"admission-unknown"}` -
/// with an empty-object payload and a garbage signature: enough to
/// reach the unknown-kid refetch, the surface under test.  The kid is
/// deliberately FIXED across requests: the verifier never
/// negative-caches a miss (the one-refetch contract), so a repeated
/// kid buys the same bypass attempt a fresh one would.
const UNKNOWN_KID_JWT: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6ImFkbWlzc2lvbi11bmtub3duIn0.e30.AAAA";

/// The verdict route the leg drives; every refusal here is pre-authn,
/// so the version under it never matters.
const VERDICT_PATH: &str = "/api/v1/admin/versions/smoke/withdep/0.1.0";

/// The fixed refusal both endpoints must answer once admission
/// refuses: `quota::OIDC_RATE_LIMITED` through the error envelope,
/// decided before any credential is read.
const EXPECTED_429: &str =
    r#"{"errors":[{"detail":"request rate limit exceeded; retry later","code":"rate_limited"}]}"#;

/// The wrangler.jsonc budgets the assertions below count against.
const JWKS_BUDGET: u64 = 6;
const ADMISSION_BURST: u32 = 70;

/// Both admission layers, in cost order: the flood that must stay
/// bounded upstream first (it needs admitted requests), the burst that
/// empties the admission bucket last.
///
/// # Errors
///
/// The first failed check, worded like the other legs.
pub fn run(smoke: &mut Smoke) -> Result<()> {
    // The local limiter's fixed windows align to the wall clock:
    // started near a minute boundary, the flood or the burst would
    // straddle two windows and the budget arithmetic below would
    // count against a mid-run reset.  Both checks finish in well
    // under the half minute this guarantees.
    let into = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |now| now.as_secs() % 60);
    if into > 30 {
        std::thread::sleep(std::time::Duration::from_secs(61 - into));
    }
    jwks_budget(smoke)?;
    exhaustion(smoke)
}

/// Repeated unknown-kid requests on BOTH endpoints buy at most the
/// global budget's worth of upstream JWKS fetches, and every refusal
/// stays the uniform 401 - no budget oracle.
fn jwks_budget(smoke: &mut Smoke) -> Result<()> {
    step("an unknown-kid flood buys at most the jwks bypass budget of upstream fetches");
    // One authenticated verdict first: the legitimate verifier path
    // still works right before the flood, and the JWKS cache is
    // re-warmed in case the run outlived its 10-minute TTL - the
    // flood's arithmetic counts on the non-bypass lookup hitting cache.
    smoke.verdict_patch(VERDICT_PATH, b"{}", &[400])?;
    let before = smoke.jwks_hits()?;
    let body = format!(r#"{{"jwt":"{UNKNOWN_KID_JWT}"}}"#);
    let bearer = vec![(
        "Authorization".to_owned(),
        format!("Bearer {UNKNOWN_KID_JWT}"),
    )];
    for index in 0..10 {
        if index % 2 == 0 {
            uniform_401_with(
                smoke,
                Base::Web,
                "PUT",
                TRUSTPUB_TOKENS_PATH,
                &[],
                Some(body.as_bytes()),
            )?;
        } else {
            uniform_401_with(
                smoke,
                Base::Web,
                "PATCH",
                VERDICT_PATH,
                &bearer,
                Some(b"{}"),
            )?;
        }
    }
    let bought = smoke.jwks_hits()? - before;
    if bought != JWKS_BUDGET {
        bail!(
            "10 unknown-kid requests bought {bought} upstream jwks fetches, \
             expected the budget's {JWKS_BUDGET}"
        );
    }
    Ok(())
}

/// The per-IP admission bucket: burst the exchange endpoint until the
/// 429 appears, then hold that the verdict endpoint answers the
/// byte-identical refusal out of the same bucket - one fixed shape
/// with a `Retry-After` on both endpoints, before any credential is
/// read.
fn exhaustion(smoke: &mut Smoke) -> Result<()> {
    step("an admission-exhausting burst answers one fixed 429 on both oidc endpoints");
    let url = smoke.url(Base::Web, TRUSTPUB_TOKENS_PATH);
    let mut refused_at = None;
    // The bucket is 60/min shared by every local request; earlier legs
    // spent some of the minute's budget, so the burst needs at most 61
    // of its own.
    for attempt in 0..ADMISSION_BURST {
        let status = smoke.http("PUT", &url, &[], Some(b"not json"))?;
        match status {
            401 => {}
            429 => {
                refused_at = Some(attempt);
                break;
            }
            other => bail!("burst request {attempt} answered {other}, expected 401 or 429"),
        }
    }
    let Some(refused_at) = refused_at else {
        bail!("{ADMISSION_BURST} burst requests never saw the admission 429");
    };
    println!("    admission refused after {refused_at} burst requests");
    if capture(&smoke.body) != EXPECTED_429 {
        bail!("exchange 429 body mismatch: {}", capture(&smoke.body));
    }
    if grep_lines(&text(&smoke.headers), "retry-after:").is_empty() {
        bail!("the exchange 429 carries no Retry-After");
    }
    // The other endpoint, same bucket, same bytes: a garbage bearer
    // that is never read.
    let verdict = smoke.url(Base::Web, VERDICT_PATH);
    let bearer = vec![("Authorization".to_owned(), "Bearer not-a-jwt".to_owned())];
    let status = smoke.http("PATCH", &verdict, &bearer, Some(b"{}"))?;
    if status != 429 {
        bail!("the verdict PATCH answered {status} out of the exhausted bucket, expected 429");
    }
    if capture(&smoke.body) != EXPECTED_429 {
        bail!("verdict 429 body mismatch: {}", capture(&smoke.body));
    }
    if grep_lines(&text(&smoke.headers), "retry-after:").is_empty() {
        bail!("the verdict 429 carries no Retry-After");
    }
    Ok(())
}
