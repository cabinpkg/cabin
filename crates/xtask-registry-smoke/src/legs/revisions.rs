//! The packaging-revision respin, the yank cycle, the source viewer's
//! ranged session reads and the two service-mode gates -
//! `registry/scripts/smoke.sh` L1333-1598.
//!
//! The leg ends one line before Phase 10's first `step`: its last
//! statements build the `0.2.1` fixtures the shared-blob flow consumes,
//! which is why [`run`] returns them rather than printing anything.
//!
//! Three shell helpers are defined inside this span and stay private
//! here - `row_downloads` (L1420), `source_range` (L1436) and
//! `source_header` (L1448) - alongside private copies of two the span
//! only *uses*: `session_request` (L758) and `await_row_downloads`
//! (L1224).  Phase 10's `stored_bytes` (L1583) lives in its own leg
//! with its own private copy of the query.
//!
//! Every literal `sleep` is preserved: the two here (L1495, L1565)
//! precede assertions that a counter did **not** move, and in-process
//! HTTP is faster than the forked `curl` they were tuned against.
//!
//! `$headers` is written on every request, as [`Smoke::http`] writes
//! it, where the shell's `check_at`/`request_at`/`session_request` left
//! it alone; no assertion in this span reads a header block an earlier
//! request wrote, so the two never diverge observably.  `$body` is a
//! different matter - the artifact download at L1401 wrote neither
//! buffer, so `download` puts the previous body back.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use xtask_registry_admin::{display, results};

use crate::bytes::{frame, replace_all, retarget_hash, revision_of, sha256_hex, tamper_zip};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::uniform_401_with;
use crate::servers::{d1, d1_json, d1_quiet};
use crate::step;
use crate::text::capture;

/// What earlier legs leave behind that this span reads.  The paths are
/// the shell's own variables (L685-692) rather than rebuilt here; `rev`
/// is not among them because it is the leading 16 hex of `old_hash`,
/// which L1335 recomputes anyway.
pub struct RevisionInputs<'a> {
    /// `$scope`, `$name`, `$version` (L681-683).
    pub scope: &'a str,
    /// `$name`.
    pub name: &'a str,
    /// `$version`.
    pub version: &'a str,
    /// `$fixture_archive`: the frozen `smoke-withdep-0.2.0.zip` bytes.
    pub fixture_archive: &'a [u8],
    /// `$fixture_metadata`: the frozen canonical metadata document.
    pub fixture_metadata: &'a [u8],
    /// `$publish_path` (L685).
    pub publish_path: &'a str,
    /// `$package_path` (L686).
    pub package_path: &'a str,
    /// `$artifact_path` (L689).
    pub artifact_path: &'a str,
    /// `$work/publish.bin`, the framed first publish (L1013).
    pub publish_bin: &'a [u8],
    /// `$work/verdict-verified.json`, the verdict Phase 8 built (L1152).
    pub verdict_verified: &'a [u8],
    /// `$session_cookie`, the minted session (L756).
    pub session_cookie: &'a str,
    /// `$token`, the publisher's bearer credential - presented here
    /// only to prove the source route refuses it (L1433).
    pub token: &'a str,
}

/// The `0.2.1` fixtures L1590-1598 builds for Phase 10.
pub struct RevisionOutputs {
    /// `$version2`.
    pub version2: String,
    /// `$publish2_path`.
    pub publish2_path: String,
    /// `$artifact2_path`: identical bytes, so the same revision id.
    pub artifact2_path: String,
    /// `$verdict2_path`.
    pub verdict2_path: String,
    /// `$work/publish2.bin`.
    pub publish2_bin: Vec<u8>,
}

/// `printf '{"yanked":true}'` (L1379) - no trailing newline.  Phase 10
/// replays these exact bytes at L1667.
pub const YANKED: &[u8] = br#"{"yanked":true}"#;

/// `printf '{"yanked":false}'` (L1391).
pub const UNYANKED: &[u8] = br#"{"yanked":false}"#;

/// The whole span, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<RevisionOutputs> {
    // `$source_path` is only named from L1416, but the yank leg spells
    // the same triple out at L1394.
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
    second_version(inputs)
}

/// L1333-1377.
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
    //
    // L1348 is the one site in this span that redirected the result
    // table to `/dev/null`.
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
    smoke.wrequest(
        "PATCH",
        &verdict_path(inputs, inputs.version),
        verdict.as_bytes(),
        &[200],
    )?;
    smoke.expect_body(&format!(r#""revision":"{new_rev}""#))?;
    smoke.expect_body(r#""verification":"rejected""#)?;
    smoke.as_publisher();
    Ok(())
}

/// L1378-1399.
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

/// L1400-1414.
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
    // The pre-revision filename is not a route any more: it does not
    // parse, so it never reaches storage and answers like any unknown
    // path - the uniform 401, token or not.
    let unrevisioned = format!(
        "/artifacts/{scope}/{name}/{scope}-{name}-{version}.zip",
        scope = inputs.scope,
        name = inputs.name,
        version = inputs.version,
    );
    let auth = smoke.auth.clone();
    uniform_401_with(smoke, Base::Registry, "GET", &unrevisioned, &auth, None)
}

/// L1415-1500.
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

/// L1427-1434: no credential and a bearer token both answer the session
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

/// L1453-1462: the range policy - required (400 when absent), single,
/// bounded, capped at 4 MiB (416 otherwise).
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

/// L1464-1487.
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

/// L1501-1520.  The dev vars pin `SERVICE_MODE_TTL_SECS` to 0, so the
/// running worker sees the flipped mode immediately instead of after
/// the 60 s cache TTL.
fn writes_blocked(smoke: &mut Smoke, inputs: &RevisionInputs<'_>, source_path: &str) -> Result<()> {
    step("writes answer 503 while writes_blocked; reads stay open");
    d1("
  UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode';
  UPDATE meta SET value = 'forced by smoke.sh' WHERE key = 'service_mode_reason';")?;
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
    smoke.wrequest("PATCH", &verdict, inputs.verdict_verified, &[200])?;
    smoke.expect_body(r#""changed":false"#)?;
    let unknown = verdict_path(inputs, "9.9.9");
    smoke.wrequest("PATCH", &unknown, inputs.verdict_verified, &[404])?;
    smoke.as_publisher();
    Ok(())
}

/// L1521-1568.
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
    smoke.check(inputs.artifact_path, &[503])?;
    smoke.check("/config.json", &[503])?;
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[503])?;
    // Anonymous readers receive the same refusal - status, envelope,
    // and Retry-After.  A public over-budget answer reveals service
    // state; that is inherent to public reads (the recorded revision).
    // /healthz stays up.
    let anonymous = smoke.http("GET", &package_url, &[], None)?;
    if anonymous != 503 {
        bail!("an anonymous read under reads_blocked answered {anonymous}");
    }
    over_budget(smoke, "the anonymous read 503 must carry Retry-After: 900")?;
    smoke.check("/healthz", &[200])?;
    // The exempt planes: the session plane and the public stats (where
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
/// (L1530-1533, L1545-1548) with only the wording of the header failure
/// differing.
fn over_budget(smoke: &Smoke, missing: &str) -> Result<()> {
    smoke.expect_body("registry_over_budget")?;
    smoke.expect_body("read budget")?;
    if !header_line(&smoke.headers, "retry-after: 900") {
        bail!("{missing}");
    }
    Ok(())
}

/// L1569-1575.
fn restore_normal(smoke: &mut Smoke, inputs: &RevisionInputs<'_>) -> Result<()> {
    step("restoring service_mode reopens writes");
    d1("
  UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
  UPDATE meta SET value = '' WHERE key = 'service_mode_reason';")?;
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_bin, &[200])?;
    smoke.expect_body(r#""no_op":true"#)
}

/// L1576-1598: the reject -> blob reclaim -> quota refund -> republish
/// flow's fixtures.  `0.2.1` carries the exact archive `0.2.0`
/// published, so this version's first revision carries the same id.
fn second_version(inputs: &RevisionInputs<'_>) -> Result<RevisionOutputs> {
    // The PUTs above consumed the publish bucket's full burst; give the
    // next leg its own by resetting the token's bucket columns.
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

/// `/api/v1/admin/versions/$scope/$name/<version>`, the verdict route
/// this span patches at three versions.
fn verdict_path(inputs: &RevisionInputs<'_>, version: &str) -> String {
    format!(
        "/api/v1/admin/versions/{}/{}/{version}",
        inputs.scope, inputs.name
    )
}

/// `-H "Range: bytes=-22"`, the suffix read every session-plane source
/// probe in this span asks for.
fn suffix_range() -> Vec<(String, String)> {
    vec![("Range".to_owned(), "bytes=-22".to_owned())]
}

/// `session_request <method> <path> <expected> [curl args...]`
/// (L758-767): the minted session's cookie plus whatever the caller
/// adds, and a status that must match exactly.
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

/// `source_range <range-or-empty> <expected>` (L1436-1447).  An empty
/// range sends **no** `Range` header at all - the shell's
/// `${range_args[@]+...}` guard, which is not the same as an
/// empty-valued header.
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

/// `source_header <pattern>` (L1448-1451): the last response must carry
/// the header.
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

/// `tr -d '\r' <"$headers" | grep -qi '^<line>$'`.
fn header_line(block: &[u8], line: &str) -> bool {
    header_text(block)
        .lines()
        .any(|found| found.eq_ignore_ascii_case(line))
}

/// `tr -d '\r' <"$headers" | grep -i '^[a-z-]*:' | head -20`: the
/// header lines only, so the status line and the terminating blank
/// stay out of the diagnostic.
fn header_diagnostic(block: &[u8]) -> String {
    header_text(block)
        .lines()
        .filter(|line| names_a_header(line))
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `^[a-z-]*:` matches under `grep -i`: the run of letters and
/// hyphens starting the line is immediately followed by a colon.
fn names_a_header(line: &str) -> bool {
    line.find(|ch: char| !ch.is_ascii_alphabetic() && ch != '-')
        .is_some_and(|at| line[at..].starts_with(':'))
}

fn header_text(block: &[u8]) -> String {
    String::from_utf8_lossy(block).replace('\r', "")
}

/// `curl -sS -o <file>`: the bytes are the caller's, and both shared
/// buffers keep what the previous request left in them - L1401 wrote
/// the body to its own file and carried no `-D`, so `$headers` stayed
/// whatever the previous request left there too.
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

/// `listing_entry` (L704-716), narrowed: this caller reads only
/// `checksum` and `published_at`, so the entry is handed back whole
/// rather than projected onto the six-field `PendingVersion` shape the
/// verifier legs need as bytes.  Every failure the node program had -
/// unparsable listing, no such entry - is the one `fail` wording.
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

/// The verdict L1366-1372's `node -e` wrote, byte for byte:
/// `JSON.stringify` over an object literal, so insertion order and
/// compact separators.
///
/// Ceiling: JavaScript drops a key whose value is `undefined` where
/// this writes `null`.  Both are a verdict the route refuses, and no
/// admin listing entry omits either field.
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

/// L1347-1348 and L1579-1580, which reset the publish bucket's burst.
const RESET_PUBLISH_BUCKET: &str = "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';";

/// The version row's download counter, for the never-counted
/// assertions (L1420-1427).
const DOWNLOADS_SQL: &str = "SELECT downloads FROM versions
     WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'";

/// `row_downloads` (L1420).
///
/// # Errors
///
/// If wrangler fails, or the row the `node` program indexed into is
/// absent - where it threw and took the pipeline down with it.
pub fn row_downloads() -> Result<String> {
    scalar(DOWNLOADS_SQL, "downloads")
}

/// `await_row_downloads <expected>` (L1224-1237): 20 attempts half a
/// second apart, and the exact count is the point - a deferred
/// increment still in flight would make the flat-counter assertions
/// pass for the wrong reason.
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

/// One `--json` read rendered as its `console.log(out[0].results[0].<column>)`
/// rendered it.
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
    fn the_verdict_matches_the_node_program_byte_for_byte() {
        let entry = serde_json::json!({
            "name": "smoke/withdep",
            "version": "0.2.0",
            "revision": "deadbeefdeadbeef",
            "checksum": "sha256:1ca5e5",
            "published_at": "1970-01-01T00:00:00Z",
            "metadata": {"schema": 1},
        });
        assert_eq!(
            verdict_respin(&entry),
            concat!(
                r#"{"verdict":"rejected","reason":"smoke respin","#,
                r#""checksum":"sha256:1ca5e5","published_at":"1970-01-01T00:00:00Z"}"#,
            )
        );
    }

    /// A value needing escaping goes through `JSON.stringify`'s
    /// escaping, not raw interpolation.
    #[test]
    fn the_verdict_escapes_the_fields_it_carries() {
        let entry = serde_json::json!({"checksum": "a\"b", "published_at": "c\\d"});
        assert_eq!(
            verdict_respin(&entry),
            r#"{"verdict":"rejected","reason":"smoke respin","checksum":"a\"b","published_at":"c\\d"}"#
        );
    }

    #[test]
    fn a_missing_listing_entry_is_the_shells_wording() {
        let listing =
            br#"{"versions":[{"name":"smoke/withdep","version":"0.2.0","checksum":"c"}]}"#;
        assert_eq!(
            listing_entry(listing, "smoke/withdep", "0.2.0")
                .expect("present")
                .get("checksum")
                .and_then(Value::as_str),
            Some("c")
        );
        assert_eq!(
            listing_entry(listing, "smoke/withdep", "0.2.1")
                .expect_err("absent")
                .to_string(),
            "the pending listing has no smoke/withdep@0.2.1"
        );
        assert_eq!(
            listing_entry(b"not json", "smoke/withdep", "0.2.0")
                .expect_err("unparsable")
                .to_string(),
            "the pending listing has no smoke/withdep@0.2.0"
        );
    }

    /// `console.log` of an integer column prints the integer; of a text
    /// column, its own text.
    #[test]
    fn a_scalar_read_renders_as_console_log_did() {
        let downloads = r#"[{"success":true,"results":[{"downloads":5}]}]"#;
        assert_eq!(scalar_of(downloads, "downloads").expect("row"), "5");
        let stored = r#"[{"success":true,"results":[{"value":"1024"}]}]"#;
        assert_eq!(scalar_of(stored, "value").expect("row"), "1024");
        let empty = r#"[{"success":true,"results":[]}]"#;
        assert!(scalar_of(empty, "downloads").is_err());
    }

    #[test]
    fn header_matching_is_whole_line_and_case_insensitive() {
        let block = BLOCK.as_bytes();
        assert!(header_line(block, "content-length: 22"));
        assert!(header_line(block, "retry-after: 900"));
        // Anchored: a prefix of a header line is not the header line.
        assert!(!header_line(block, "content-length: 2"));
        assert!(!header_line(block, "cache-control"));
    }

    #[test]
    fn the_source_header_pattern_is_an_anchored_literal() {
        let mut smoke = Smoke::new(0, 0, "cabin_smoke".to_owned());
        smoke.headers = BLOCK.as_bytes().to_vec();
        source_header(&smoke, "^content-range: bytes 377-398/399$").expect("present");
        source_header(&smoke, "^Cache-Control: no-store$").expect("case-insensitive");
        // The escaped `\*` is a literal asterisk, and the failure
        // wording carries the pattern as the shell wrote it.
        assert_eq!(
            source_header(&smoke, r"^content-range: bytes \*/399$")
                .expect_err("absent")
                .to_string(),
            concat!(
                r"missing header ^content-range: bytes \*/399$: ",
                // Matched case-insensitively, but printed as it
                // arrived: `grep -i` never rewrites the line.
                "content-range: bytes 377-398/399\nContent-Length: 22\n",
                "cache-control: no-store\nretry-after: 900",
            )
        );
    }

    /// The diagnostic keeps only header lines: `grep -i '^[a-z-]*:'`
    /// never matches the status line.
    #[test]
    fn the_diagnostic_drops_the_status_line() {
        assert!(names_a_header("content-length: 22"));
        assert!(names_a_header("X-Content-Type-Options: nosniff"));
        assert!(!names_a_header("HTTP/1.1 206 Partial Content"));
        assert!(!names_a_header(""));
    }

    /// The `0.2.1` metadata is a textual replace, so `source.path`
    /// moves with `version` and every other byte is untouched.
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
