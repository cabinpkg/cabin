//! The packaging-revision respin, the yank cycle, the source viewer's
//! ranged session reads and the two service-mode gates.
//!
//! Its last statements build the `0.2.1` fixtures the shared-blob leg
//! consumes, which [`run`] returns.
//!
//! The two `sleep`s here precede assertions that a counter did **not**
//! move; trimming them turns those into races that pass for the wrong
//! reason.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use xtask_registry_admin::{display, results};

use crate::bytes::{frame, replace_all, retarget_hash, revision_of, sha256_hex, tamper_zip};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::{SESSION_TOKENS_PATH, TRUSTPUB_TOKENS_PATH, uniform_401_with};
use crate::servers::{d1, d1_json, d1_quiet};
use crate::step;
use crate::text::capture;

/// What earlier legs leave behind that this span reads.  The paths are
/// passed in rather than rebuilt here; `rev` is not among them because
/// it is recomputed from the archive.
pub struct RevisionInputs<'a> {
    pub scope: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    /// The frozen `smoke-withdep-0.2.0.zip` bytes.
    pub fixture_archive: &'a [u8],
    /// The frozen canonical metadata document.
    pub fixture_metadata: &'a [u8],
    pub publish_path: &'a str,
    pub package_path: &'a str,
    pub artifact_path: &'a str,
    /// The framed first publish.
    pub publish_bin: &'a [u8],
    /// The verdict the publish leg built.
    pub verdict_verified: &'a [u8],
    /// The minted session.
    pub session_cookie: &'a str,
    /// The publisher's bearer credential - presented here only to prove
    /// the source route refuses it.
    pub token: &'a str,
}

/// The `0.2.1` fixtures built for the shared-blob leg.
pub struct RevisionOutputs {
    pub version2: String,
    pub publish2_path: String,
    /// Identical bytes, so the same revision id.
    pub artifact2_path: String,
    pub verdict2_path: String,
    pub publish2_bin: Vec<u8>,
}

/// The shared-blob leg replays these exact bytes.
pub const YANKED: &[u8] = br#"{"yanked":true}"#;

pub const UNYANKED: &[u8] = br#"{"yanked":false}"#;

/// The whole span.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<RevisionOutputs> {
    let source_path = format!(
        "/api/v1/user/source/{}/{}/{}",
        inputs.scope, inputs.name, inputs.version
    );
    respin(smoke, inputs)?;
    yank_cycle(smoke, inputs, &source_path)?;
    artifact_checksum(smoke, inputs)?;
    source_viewer(smoke, inputs, &source_path)?;
    writes_blocked(smoke, inputs, &source_path)?;
    reads_blocked(smoke, inputs, &source_path)?;
    restore_normal(smoke, inputs)?;
    shared_bucket(smoke, inputs)?;
    second_version(inputs)
}

fn respin(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<()> {
    step("tampered re-publish needs the new-revision opt-in");
    let tampered = tamper_zip(inputs.fixture_archive, 1);
    let old_hash = sha256_hex(inputs.fixture_archive);
    let new_hash = sha256_hex(&tampered);
    let new_rev = revision_of(&tampered);
    let tampered_artifact_path = format!(
        "/artifacts/{scope}/{name}/{scope}-{name}-{version}-{new_rev}.zip",
        scope = inputs.scope,
        name = inputs.name,
        version = inputs.version,
    );
    let metadata = retarget_hash(inputs.fixture_metadata, &old_hash, &new_hash);
    let body = frame(&metadata, &tampered);
    smoke.wrequest("PUT", inputs.publish_path, &body, &[409])?;
    smoke.expect_body("new-revision")?;

    step("the opt-in publishes the changed bytes as a pending respin");
    // The same refused pair, opted in: a new packaging revision of the
    // same version, pending like any other first publish - and
    // invisible to reads until it is verified, so what the registry
    // serves cannot move.
    d1_quiet(RESET_PUBLISH_BUCKET)?;
    let opt_in = format!("{}?new-revision=true", inputs.publish_path);
    smoke.wrequest("PUT", &opt_in, &body, &[201])?;
    smoke.expect_body(&format!(r#""revision":"{new_rev}""#))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(&format!(r#""revision":"{}""#, &old_hash[..16]))?;
    if capture(&smoke.body).contains(&new_rev) {
        bail!(
            "a pending respin leaked into the package document: {}",
            capture(&smoke.body)
        );
    }
    // Invisible to a no-verify credential (the session-shaped
    // publisher token carries verify and would read it).
    smoke.as_ci_publisher();
    smoke.check(&tampered_artifact_path, &[404])?;
    smoke.as_verifier();
    smoke.check(&tampered_artifact_path, &[200])?;
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    smoke.expect_body(&format!(r#""revision":"{new_rev}""#))?;
    // Rejected right here: the respin has served its purpose, and
    // leaving it pending would put a second candidate in front of every
    // later listing and stuck-pending alert.
    let package = format!("{}/{}", inputs.scope, inputs.name);
    let entry = listing_entry(&smoke.body, &package, inputs.version)?;
    let verdict = verdict_respin(&entry);
    smoke.verdict_patch(
        &verdict_path(inputs, inputs.version),
        verdict.as_bytes(),
        &[200],
    )?;
    smoke.expect_body(&format!(r#""revision":"{new_rev}""#))?;
    smoke.expect_body(r#""verification":"rejected""#)?;
    smoke.as_publisher();
    Ok(())
}

fn yank_cycle(smoke: &mut Smoke, inputs: &RevisionInputs<'_>, source_path: &str) -> Result<()> {
    step("yank and un-yank walk the state transitions");
    let yank_path = format!("{}/yank", inputs.publish_path);
    smoke.wrequest("PATCH", &yank_path, YANKED, &[200])?;
    smoke.expect_body(r#""yanked":true"#)?;
    smoke.expect_body(r#""changed":true"#)?;
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(r#""yanked":true"#)?;
    // The session packages listing mirrors the row: the seeded user
    // created the package, its version is verified by now, and
    // currently yanked.
    session_request(smoke, inputs, "GET", "/api/v1/user/packages", 200, &[])?;
    smoke.expect_body(&format!(r#""name":"{}/{}""#, inputs.scope, inputs.name))?;
    smoke.expect_body(r#""verification":"verified""#)?;
    smoke.expect_body(r#""yanked":true"#)?;
    // Yanked stays browsable in the source viewer, like the artifact route.
    session_request(smoke, inputs, "GET", source_path, 206, &suffix_range())?;
    smoke.wrequest("PATCH", &yank_path, UNYANKED, &[200])?;
    smoke.expect_body(r#""yanked":false"#)?;
    smoke.expect_body(r#""changed":true"#)?;
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(r#""yanked":false"#)
}

fn artifact_checksum(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<()> {
    step("published artifact downloads with the published checksum");
    let artifact = download(smoke, Base::Registry, inputs.artifact_path)?;
    let got_hash = sha256_hex(&artifact);
    let old_hash = sha256_hex(inputs.fixture_archive);
    if got_hash != old_hash {
        bail!("artifact checksum mismatch: got {got_hash}, expected {old_hash}");
    }
    if !capture(inputs.fixture_metadata).contains(&format!("sha256:{old_hash}")) {
        bail!("fixture metadata does not carry sha256:{old_hash}");
    }
    // The unrevisioned filename does not parse as an artifact route, so
    // it never reaches storage and answers like any unknown path - the
    // uniform 401, token or not.
    let unrevisioned = format!(
        "/artifacts/{scope}/{name}/{scope}-{name}-{version}.zip",
        scope = inputs.scope,
        name = inputs.name,
        version = inputs.version,
    );
    let auth = smoke.auth.clone();
    uniform_401_with(smoke, Base::Registry, "GET", &unrevisioned, &auth, None)
}

fn source_viewer(smoke: &mut Smoke, inputs: &RevisionInputs<'_>, source_path: &str) -> Result<()> {
    step("the source route serves session-ranged reads of the verified archive");
    let size = inputs.fixture_archive.len();
    // The two artifact fetches since the counted-downloads step (the
    // heal re-check and the checksum download) land their deferred
    // increments here; awaiting the exact count keeps the flat-counter
    // assertion below race-free.  A new artifact fetch above must bump
    // this number.
    await_row_downloads("5")?;
    source_credentials(smoke, inputs, source_path)?;
    range_policy(smoke, inputs, source_path, size)?;
    range_reads(smoke, inputs, source_path, size)?;

    // Unknown versions and unparsable triples answer the plain 404.
    let package_source = format!("/api/v1/user/source/{}/{}", inputs.scope, inputs.name);
    let unknown = format!("{package_source}/9.9.9");
    session_request(smoke, inputs, "GET", &unknown, 404, &suffix_range())?;
    let unparsable = format!("{package_source}/notsemver");
    session_request(smoke, inputs, "GET", &unparsable, 404, &[])?;

    // Source reads are never downloads: the counter did not move.
    sleep(Duration::from_secs(1));
    if row_downloads()? != "5" {
        bail!(
            "source reads moved the download counter: {} (expected 5)",
            row_downloads()?
        );
    }
    Ok(())
}

/// No credential and a bearer token both answer the session
/// plane's plain 401, so the route never accepts the machine plane's
/// credential.
fn source_credentials(
    smoke: &mut Smoke,
    inputs: &RevisionInputs<'_>,
    source_path: &str,
) -> Result<()> {
    let url = smoke.url(Base::Web, source_path);
    let status = smoke.http("GET", &url, &suffix_range(), None)?;
    if status != 401 {
        bail!("a session-less source read answered {status}");
    }
    let mut bearer = suffix_range();
    bearer.push((
        "Authorization".to_owned(),
        format!("Bearer {}", inputs.token),
    ));
    let status = smoke.http("GET", &url, &bearer, None)?;
    if status != 401 {
        bail!("a bearer token opened the source route: {status}");
    }
    Ok(())
}

/// The range policy - required (400 when absent), single, bounded,
/// capped at 4 MiB (416 otherwise).
fn range_policy(
    smoke: &mut Smoke,
    inputs: &RevisionInputs<'_>,
    source_path: &str,
    size: usize,
) -> Result<()> {
    source_range(smoke, inputs, source_path, "", 400)?;
    if !capture(&smoke.body).contains("bounded range") {
        bail!(
            "the 400 does not name the range policy: {}",
            capture(&smoke.body)
        );
    }
    for bad in [
        "bytes=0-",
        "bytes=abc-5",
        "bytes=0-5,10-20",
        "bytes=-0",
        "bytes=0-4194304",
    ] {
        source_range(smoke, inputs, source_path, bad, 416)?;
    }
    // A start past the end is the size-relative 416 naming the actual size.
    let past_end = format!("bytes={size}-{}", size + 10);
    source_range(smoke, inputs, source_path, &past_end, 416)?;
    source_header(smoke, &format!(r"^content-range: bytes \*/{size}$"))
}

fn range_reads(
    smoke: &mut Smoke,
    inputs: &RevisionInputs<'_>,
    source_path: &str,
    size: usize,
) -> Result<()> {
    // A suffix read returns the exact EOCD bytes with the exact
    // headers, no-store and nosniff like every session-plane response.
    source_range(smoke, inputs, source_path, "bytes=-22", 206)?;
    if smoke.body.as_slice() != &inputs.fixture_archive[size - 22..] {
        bail!("the EOCD suffix read differs from the archive tail");
    }
    source_header(
        smoke,
        &format!("^content-range: bytes {}-{}/{size}$", size - 22, size - 1),
    )?;
    source_header(smoke, "^content-length: 22$")?;
    source_header(smoke, "^cache-control: no-store$")?;
    source_header(smoke, "^x-content-type-options: nosniff$")?;
    source_header(smoke, "^accept-ranges: bytes$")?;
    // A bounded read slices the archive's first bytes; an end past the
    // last byte is clamped HTTP-style.
    source_range(smoke, inputs, source_path, "bytes=0-3", 206)?;
    if smoke.body.as_slice() != &inputs.fixture_archive[..4] {
        bail!("the bounded read differs from the archive head");
    }
    let clamped = format!("bytes={}-{}", size - 10, size + 100);
    source_range(smoke, inputs, source_path, &clamped, 206)?;
    source_header(
        smoke,
        &format!("^content-range: bytes {}-{}/{size}$", size - 10, size - 1),
    )
}

/// The dev vars pin `SERVICE_MODE_TTL_SECS` to 0, so the running worker
/// sees the flipped mode immediately instead of after the 60 s cache
/// TTL.
fn writes_blocked(smoke: &mut Smoke, inputs: &RevisionInputs<'_>, source_path: &str) -> Result<()> {
    step("writes answer 503 while writes_blocked; reads stay open");
    d1("
  UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode';
  UPDATE meta SET value = 'forced by the smoke test' WHERE key = 'service_mode_reason';")?;
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[503])?;
    smoke.expect_body("registry_over_budget")?;
    let yank_path = format!("{}/yank", inputs.publish_path);
    smoke.wrequest("PATCH", &yank_path, UNYANKED, &[503])?;
    smoke.expect_body("registry_over_budget")?;
    smoke.check(inputs.package_path, &[200])?;
    // Source reads are reads: they never consult the service mode.
    session_request(smoke, inputs, "GET", source_path, 206, &suffix_range())?;
    // Verdicts are deliberately exempt from the budget gates: the
    // idempotent repeat lands (the queue drains while blocked), and an
    // unknown triple is the authenticated 404, never the 503.
    smoke.as_verifier();
    let verdict = verdict_path(inputs, inputs.version);
    smoke.verdict_patch(&verdict, inputs.verdict_verified, &[200])?;
    smoke.expect_body(r#""changed":false"#)?;
    let unknown = verdict_path(inputs, "9.9.9");
    smoke.verdict_patch(&unknown, inputs.verdict_verified, &[404])?;
    smoke.as_publisher();
    // The trusted-publishing exchange gates only its config arm, and
    // only past the backing owner's private-alpha admission: an
    // unverifiable JWT, and a configured one whose scope owner is not
    // admitted (user 3), stay the uniform 401 (the gate
    // speaks only to admitted callers, here as everywhere); an admitted
    // scope's run is the gate's 503; and the verifier arm mints
    // straight through - the verification pipeline's bootstrap is
    // exempt like the verdicts above.
    let exchange = smoke.url(Base::Web, TRUSTPUB_TOKENS_PATH);
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        TRUSTPUB_TOKENS_PATH,
        &[],
        Some(br#"{"jwt":"not-a-jwt"}"#),
    )?;
    let jwt = smoke.mint_verifier_jwt(
        r#"{"aud":"cabinpkg.com","workflow_ref":"cabinpkg/cabin/.github/workflows/ports-publish.yml@refs/heads/main"}"#,
    )?;
    let body = format!(r#"{{"jwt":"{jwt}"}}"#);
    // The migration-seeded cabin-ports config matches this JWT; back
    // it with an owner for the duration.
    d1_quiet(
        "INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('cabin-ports', 'github', '2', '1970-01-01T00:00:00.000Z');
         INSERT INTO scope_members (scope_name, user_id, role) VALUES ('cabin-ports', 3, 'owner');",
    )?;
    let got = smoke.http("PUT", &exchange, &[], Some(body.as_bytes()))?;
    if got != 401 {
        bail!("the config arm for an unadmitted owner answered {got}, expected the uniform 401");
    }
    d1_quiet("UPDATE scope_members SET user_id = 2 WHERE scope_name = 'cabin-ports'")?;
    let got = smoke.http("PUT", &exchange, &[], Some(body.as_bytes()))?;
    d1_quiet(
        "DELETE FROM scope_members WHERE scope_name = 'cabin-ports';
         DELETE FROM scopes WHERE name = 'cabin-ports';",
    )?;
    if got != 503 {
        bail!("the config arm under writes_blocked answered {got}, expected the gate's 503");
    }
    smoke.expect_body("registry_over_budget")?;
    if !header_line(&smoke.headers, "retry-after: 900") {
        bail!("the exchange 503 must carry Retry-After: 900");
    }
    let jwt = smoke.mint_verifier_jwt(r#"{"aud":"cabinpkg.com"}"#)?;
    let body = format!(r#"{{"jwt":"{jwt}"}}"#);
    let got = smoke.http("PUT", &exchange, &[], Some(body.as_bytes()))?;
    if got != 200 {
        bail!("the verifier arm under writes_blocked answered {got}, expected 200");
    }
    // Revocation skips the gate too (blocking it would keep a live
    // credential alive); revoking here both proves that positively and
    // leaves no live credential behind.
    let minted: Value = serde_json::from_slice(&smoke.body).context("parse the exchange answer")?;
    let token = minted
        .get("token")
        .and_then(Value::as_str)
        .with_context(|| format!("no token in the exchange answer: {minted}"))?
        .to_owned();
    let verifier_auth = vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    let got = smoke.http("DELETE", &exchange, &verifier_auth, None)?;
    if got != 204 {
        bail!("revoking the verifier-arm token under writes_blocked answered {got}, expected 204");
    }
    // Even blocked, the answer is the kind guard's: the publisher's
    // session-kind token is refused with the uniform 401 - never a 503,
    // and never a deletion.
    let auth = smoke.auth.clone();
    uniform_401_with(
        smoke,
        Base::Web,
        "DELETE",
        TRUSTPUB_TOKENS_PATH,
        &auth,
        None,
    )?;
    // Header-identical too: the kind-guard 401 must not carry the
    // generation debug stamp the unauthenticated 401 lacks, or the
    // stamp is a token-validity oracle on this route.
    if String::from_utf8_lossy(&smoke.headers)
        .to_ascii_lowercase()
        .contains("x-cabin-registry-generation")
    {
        bail!("the trustpub DELETE 401 must not carry the generation header");
    }
    // The session mint gates identically, but past admission: a
    // credential GitHub refuses learns nothing of the mode, and only
    // an admitted identity sees the gate's 503.
    let mint = smoke.url(Base::Web, SESSION_TOKENS_PATH);
    let got = smoke.http("PUT", &mint, &[], Some(br#"{"github_token":"gho_wrong"}"#))?;
    if got != 401 {
        bail!("a refused session mint under writes_blocked answered {got}, expected 401");
    }
    let got = smoke.http("PUT", &mint, &[], Some(br#"{"github_token":"gho_smoke"}"#))?;
    if got != 503 {
        bail!("the session mint under writes_blocked answered {got}, expected the gate's 503");
    }
    smoke.expect_body("registry_over_budget")?;
    if !header_line(&smoke.headers, "retry-after: 900") {
        bail!("the session-mint 503 must carry Retry-After: 900");
    }
    Ok(())
}

fn reads_blocked(smoke: &mut Smoke, inputs: &RevisionInputs<'_>, source_path: &str) -> Result<()> {
    step("reads answer 503 while reads_blocked; the exempt planes stay open");
    d1("
  UPDATE meta SET value = 'reads_blocked' WHERE key = 'service_mode';")?;
    let downloads_before = row_downloads()?;
    // The data plane refuses with the read-side envelope and the
    // cron-cadence Retry-After; writes stay blocked too (reads_blocked
    // sits above writes_blocked on the ladder).
    let package_url = smoke.url(Base::Registry, inputs.package_path);
    let auth = smoke.auth.clone();
    let got = smoke.http("GET", &package_url, &auth, None)?;
    if got != 503 {
        bail!("a read under reads_blocked answered {got}");
    }
    over_budget(smoke, "the read 503 must carry Retry-After: 900")?;
    // The artifact route is the verify scope's exemption, which the
    // session-shaped publisher token rides - the gated answer needs the
    // no-verify credential.  The static config.json is never gated.
    smoke.as_ci_publisher();
    smoke.check(inputs.artifact_path, &[503])?;
    smoke.check("/config.json", &[200])?;
    smoke.as_publisher();
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[503])?;
    // Anonymous readers get the private-alpha 401 before the gate, so
    // the service state never reaches them.  /healthz stays up.
    uniform_401_with(smoke, Base::Registry, "GET", inputs.package_path, &[], None)?;
    smoke.check("/healthz", &[200])?;
    // The exempt planes: the session plane and the stats (where
    // operators and users see what is happening), the admin plane, and
    // the verifier's config and artifact fetches - but not package
    // documents, which the verifier never reads.
    session_request(smoke, inputs, "GET", source_path, 206, &suffix_range())?;
    smoke.wcheck("/api/v1/stats", &[200])?;
    smoke.as_verifier();
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    smoke.check("/config.json", &[200])?;
    smoke.check(inputs.artifact_path, &[200])?;
    smoke.check(inputs.package_path, &[503])?;
    smoke.as_publisher();
    // The exempt fetch was served, but the download counter follows the
    // write plane's fail-closed rule and must not have moved.
    sleep(Duration::from_secs(1));
    if row_downloads()? != downloads_before {
        bail!(
            "a reads_blocked download moved the counter: {} (expected {downloads_before})",
            row_downloads()?
        );
    }
    Ok(())
}

/// The read-side refusal envelope and its `Retry-After`, asserted twice
/// with only the wording of the header failure differing.
fn over_budget(smoke: &Smoke, missing: &str) -> Result<()> {
    smoke.expect_body("registry_over_budget")?;
    smoke.expect_body("read budget")?;
    if !header_line(&smoke.headers, "retry-after: 900") {
        bail!("{missing}");
    }
    Ok(())
}

fn restore_normal(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<()> {
    step("restoring service_mode reopens writes");
    d1("
  UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
  UPDATE meta SET value = '' WHERE key = 'service_mode_reason';")?;
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[200])?;
    smoke.expect_body(r#""no_op":true"#)
}

/// One bucket per user: with the publisher's bucket drained, the same
/// user's trustpub token is refused too - minting a second token buys
/// no second burst. Drained explicitly (an empty balance stamped now)
/// rather than by counting the PUTs above, so refill timing cannot
/// let the attempt through.
fn shared_bucket(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<()> {
    step("a drained publish bucket refuses every token the user holds");
    d1(
        "\n  UPDATE users SET rl_tokens = 0, rl_updated_at = strftime('%s', 'now') * 1000 \
         WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke');",
    )?;
    smoke.as_ci_publisher();
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[429])?;
    smoke.as_publisher();
    Ok(())
}

/// The reject -> blob reclaim -> quota refund -> republish flow's
/// fixtures.  `0.2.1` carries the exact archive `0.2.0` published, so
/// this version's first revision carries the same id.
fn second_version(inputs: &RevisionInputs<'_>) -> Result<RevisionOutputs> {
    // `shared_bucket` drained the publish bucket; give the next leg a
    // full burst by resetting the user's bucket columns.
    d1(RESET_PUBLISH_BUCKET)?;
    let version2 = "0.2.1";
    let rev = revision_of(inputs.fixture_archive);
    // A global textual replace over the raw document, never a JSON
    // edit: `source.path` spells the version out too, and
    // re-serializing would move the document's own bytes.
    let metadata = replace_all(inputs.fixture_metadata, b"0.2.0", b"0.2.1");
    Ok(RevisionOutputs {
        version2: version2.to_owned(),
        publish2_path: format!(
            "/api/v1/packages/{}/{}/{version2}",
            inputs.scope, inputs.name
        ),
        artifact2_path: format!(
            "/artifacts/{scope}/{name}/{scope}-{name}-{version2}-{rev}.zip",
            scope = inputs.scope,
            name = inputs.name,
        ),
        verdict2_path: verdict_path(inputs, version2),
        publish2_bin: frame(&metadata, inputs.fixture_archive),
    })
}

/// `/api/v1/admin/versions/<scope>/<name>/<version>`, the verdict route.
fn verdict_path(inputs: &RevisionInputs<'_>, version: &str) -> String {
    format!(
        "/api/v1/admin/versions/{}/{}/{version}",
        inputs.scope, inputs.name
    )
}

fn suffix_range() -> Vec<(String, String)> {
    vec![("Range".to_owned(), "bytes=-22".to_owned())]
}

/// The minted session's cookie plus whatever the caller adds, and a
/// status that must match exactly.
fn session_request(
    smoke: &mut Smoke,
    inputs: &RevisionInputs<'_>,
    method: &str,
    path: &str,
    expected: u16,
    extra: &[(String, String)],
) -> Result<()> {
    let url = smoke.url(Base::Web, path);
    let mut headers = vec![("Cookie".to_owned(), inputs.session_cookie.to_owned())];
    headers.extend_from_slice(extra);
    let status = smoke.http(method, &url, &headers, None)?;
    if status != expected {
        bail!(
            "{method} {path} returned {status}, expected {expected} (body: {})",
            capture(&smoke.body)
        );
    }
    println!("    {method} {path} -> {status}");
    Ok(())
}

/// An empty range sends **no** `Range` header at all, which is not the
/// same as an empty-valued header.
fn source_range(
    smoke: &mut Smoke,
    inputs: &RevisionInputs<'_>,
    source_path: &str,
    range: &str,
    expected: u16,
) -> Result<()> {
    let url = smoke.url(Base::Web, source_path);
    let mut headers = vec![("Cookie".to_owned(), inputs.session_cookie.to_owned())];
    if !range.is_empty() {
        headers.push(("Range".to_owned(), range.to_owned()));
    }
    let got = smoke.http("GET", &url, &headers, None)?;
    let shown = if range.is_empty() {
        "<no range>"
    } else {
        range
    };
    if got != expected {
        bail!(
            "source {shown} returned {got}, expected {expected} (body: {})",
            capture(&smoke.body)
        );
    }
    println!("    source {shown} -> {got}");
    Ok(())
}

/// The last response must carry the header.
///
/// Every pattern this span passes is `^<literal>$` whose only
/// metacharacters are the two anchors and, once, an escaped `\*`, so
/// the match is a whole-line case-insensitive comparison rather than a
/// regex engine.  The pattern is taken verbatim because the failure
/// wording interpolates it.
fn source_header(smoke: &Smoke, pattern: &str) -> Result<()> {
    let line = pattern.strip_prefix('^').unwrap_or(pattern);
    let line = line.strip_suffix('$').unwrap_or(line);
    if header_line(&smoke.headers, &line.replace('\\', "")) {
        return Ok(());
    }
    bail!(
        "missing header {pattern}: {}",
        header_diagnostic(&smoke.headers)
    )
}

/// Whether the header block, CRs dropped, has `line` as a whole line,
/// case-insensitively.
fn header_line(block: &[u8], line: &str) -> bool {
    header_text(block)
        .lines()
        .any(|found| found.eq_ignore_ascii_case(line))
}

/// The first 20 header lines, CRs dropped: the header lines only, so
/// the status line and the terminating blank stay out of the
/// diagnostic.
fn header_diagnostic(block: &[u8]) -> String {
    header_text(block)
        .lines()
        .filter(|line| names_a_header(line))
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `^[a-z-]*:` matches case-insensitively: the run of letters
/// and hyphens starting the line is immediately followed by a colon.
fn names_a_header(line: &str) -> bool {
    line.find(|ch: char| !ch.is_ascii_alphabetic() && ch != '-')
        .is_some_and(|at| line[at..].starts_with(':'))
}

fn header_text(block: &[u8]) -> String {
    String::from_utf8_lossy(block).replace('\r', "")
}

/// The bytes are the caller's, and both shared buffers keep what the
/// previous request left in them.
fn download(smoke: &mut Smoke, at: Base, path: &str) -> Result<Vec<u8>> {
    let url = smoke.url(at, path);
    let auth = smoke.auth.clone();
    let previous_body = std::mem::take(&mut smoke.body);
    let previous_headers = std::mem::take(&mut smoke.headers);
    let outcome = smoke.http("GET", &url, &auth, None);
    smoke.headers = previous_headers;
    outcome?;
    Ok(std::mem::replace(&mut smoke.body, previous_body))
}

/// `session::listing_entry`, narrowed: this caller reads only
/// `checksum` and `published_at`, so the entry is handed back whole
/// rather than projected onto the six-field `PendingVersion` shape the
/// verifier legs need as bytes.  Both failures - unparsable listing,
/// no such entry - share one wording.
fn listing_entry(body: &[u8], name: &str, version: &str) -> Result<Value> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|doc| {
            doc.get("versions")?
                .as_array()?
                .iter()
                .find(|entry| {
                    entry.get("name").and_then(Value::as_str) == Some(name)
                        && entry.get("version").and_then(Value::as_str) == Some(version)
                })
                .cloned()
        })
        .with_context(|| format!("the pending listing has no {name}@{version}"))
}

/// The respin's rejection verdict, in this field order with compact
/// separators.
///
/// A missing field is written as `null`, a verdict the route refuses;
/// no admin listing entry omits either field.
fn verdict_respin(entry: &Value) -> String {
    format!(
        r#"{{"verdict":"rejected","reason":"smoke respin","checksum":{},"published_at":{}}}"#,
        json_field(entry, "checksum"),
        json_field(entry, "published_at"),
    )
}

fn json_field(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .map_or_else(|| "null".to_owned(), Value::to_string)
}

/// Resets the publish bucket's burst.
const RESET_PUBLISH_BUCKET: &str = "
  UPDATE users SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke');";

/// The version row's download counter, for the never-counted
/// assertions.
const DOWNLOADS_SQL: &str = "SELECT downloads FROM versions
     WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'";

/// The version row's download counter.
///
/// # Errors
///
/// If wrangler fails, or the row is absent.
pub fn row_downloads() -> Result<String> {
    scalar(DOWNLOADS_SQL, "downloads")
}

/// 20 attempts half a second apart, and the exact count is the point -
/// a deferred increment still in flight would make the flat-counter
/// assertions pass for the wrong reason.
///
/// # Errors
///
/// If the counter never reaches `expected`.
pub fn await_row_downloads(expected: &str) -> Result<()> {
    let mut last = String::new();
    for _ in 0..20 {
        last = row_downloads()?;
        if last == expected {
            println!("    downloads(smoke/withdep@0.2.0) = {expected}");
            return Ok(());
        }
        sleep(Duration::from_millis(500));
    }
    bail!("smoke/withdep@0.2.0 downloads never reached {expected} (last: {last})")
}

/// The first row's `column` of one `--json` read, as display text.
fn scalar(sql: &str, column: &str) -> Result<String> {
    scalar_of(&d1_json(sql)?, column)
}

fn scalar_of(json: &str, column: &str) -> Result<String> {
    let rows = results(json)?;
    let value = rows
        .first()
        .and_then(|row| row.get(column))
        .with_context(|| format!("the query returned no {column}"))?;
    Ok(display(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = concat!(
        "HTTP/1.1 206 Partial Content\r\n",
        "content-range: bytes 377-398/399\r\n",
        "Content-Length: 22\r\n",
        "cache-control: no-store\r\n",
        "retry-after: 900\r\n",
        "\r\n",
    );

    #[test]
    fn header_matching_is_whole_line_and_case_insensitive() {
        let block = BLOCK.as_bytes();
        assert!(header_line(block, "content-length: 22"));
        assert!(header_line(block, "retry-after: 900"));
        // Anchored: a prefix of a header line is not the header line.
        assert!(!header_line(block, "content-length: 2"));
        assert!(!header_line(block, "cache-control"));
    }

    /// The diagnostic keeps only header lines: `^[a-z-]*:` never
    /// matches the status line.
    #[test]
    fn the_diagnostic_drops_the_status_line() {
        assert!(names_a_header("content-length: 22"));
        assert!(names_a_header("X-Content-Type-Options: nosniff"));
        assert!(!names_a_header("HTTP/1.1 206 Partial Content"));
        assert!(!names_a_header(""));
    }

    /// The `0.2.1` metadata is a textual replace: every `0.2.0`,
    /// `source.path`'s included, becomes `0.2.1`, and every other byte
    /// is untouched.
    #[test]
    fn the_second_version_metadata_is_a_textual_replace() {
        let metadata =
            br#"{"version":"0.2.0","path":"smoke-withdep-0.2.0-abc.zip","dep":"^0.2.0"}"#;
        assert_eq!(
            String::from_utf8(replace_all(metadata, b"0.2.0", b"0.2.1")).expect("utf8"),
            r#"{"version":"0.2.1","path":"smoke-withdep-0.2.1-abc.zip","dep":"^0.2.1"}"#
        );
    }
}
