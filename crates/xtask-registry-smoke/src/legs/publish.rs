//! The publish and verification core: the validation 400s that refuse
//! before any write, the first publish and where its archive is stored,
//! the retry that completes a reservation whose store never happened,
//! the pending version's invisibility to every reader without a verify
//! scope, the verifier's listing/download/verdict round trip, download
//! counting, and the idempotent re-publish.
//!
//! One shape here looks like it could be tidied and cannot be.
//!
//! The **oversized verdict** must reach the Worker with no
//! `Content-Length`, because the leg's whole subject is that the body
//! cap holds while a stream is read.  `ureq` reads a
//! `Transfer-Encoding: chunked` header off the caller's request: with a
//! user-set `chunked` encoding it skips its own `Content-Length` and
//! chunk-frames the body it was given (`ureq-2.12.1/src/unit.rs:50-83`,
//! pinned upstream by that crate's own `content_length_and_chunked`
//! test).  So the ordinary [`Smoke::http`] carries this leg;
//! `send(reader)` would work too, but only because an unsized reader
//! lands in the same branch.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use xtask_registry_admin::{display, results};

use crate::bytes::{frame, replace_all, sha256_hex};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::{PRIVATE_ALPHA, uniform_401_with};
use crate::servers::{d1_json, d1_quiet, delete_upload, stored_upload};
use crate::step;
use crate::text::{capture, contains, grep_anywhere, grep_lines, read, text, write};

/// The poll budget of `await_row_downloads`: 20 × 0.5s.
const POLLS: u32 = 20;
const HALF_SECOND: Duration = Duration::from_millis(500);

/// The unbound verdict body, sent twice: once with the publisher's
/// bearer (the uniform 401) and once with a verifier JWT (400).
const VERDICT_UNBOUND: &[u8] = br#"{"verdict":"verified"}"#;

/// What this span needs out of the run's setup, with the fixtures as
/// the *paths* [`run`] reads them from.
pub struct PublishInputs<'a> {
    /// The run's scratch directory.
    pub work: &'a Path,
    pub fixture_archive: &'a Path,
    pub fixture_metadata: &'a Path,
    /// The verifier binary, already built, next to this tool's own binary.
    pub verifier_bin: &'a Path,
    /// The minted session cookie.
    pub session_cookie: &'a str,
    /// `smoke`, `withdep`, `0.2.0`.
    pub scope: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    /// The archive's packaging revision id.
    pub rev: &'a str,
    /// The archive's full SHA-256.
    pub blob_hash: &'a str,
    pub publish_path: &'a str,
    pub package_path: &'a str,
    pub artifact_path: &'a str,
}

/// The whole publish/verify core.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    let metadata = read(inputs.fixture_metadata)?;
    let archive = read(inputs.fixture_archive)?;

    bare_dependency_key(smoke, inputs, &metadata, &archive)?;
    upload_id_required(smoke, inputs, &frame(&metadata, &archive))?;
    let (publish_body, upload_id) = first_publish(smoke, inputs, &metadata, &archive)?;
    unstored_reservation(smoke, inputs, &publish_body, &upload_id, &archive)?;
    twin_and_reserved_names(smoke, inputs, &metadata, &archive)?;
    pending_invisibility(smoke, inputs)?;
    let pending = verify_scope_listing(smoke, inputs)?;
    unbound_verdict(smoke, inputs)?;
    let entry = advisory_abstain(inputs, &pending)?;
    let verdict_verified = real_verification(smoke, inputs, &entry)?;
    corpus_vetted(smoke, inputs)?;
    search_and_package_routes(smoke, inputs)?;
    verdict_idempotence(smoke, inputs, &entry, &verdict_verified)?;
    download_counting(smoke, inputs)?;
    closed_to_outsiders(smoke, inputs)?;
    byte_identical_no_op(smoke, inputs, &publish_body)
}

fn bare_dependency_key(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<()> {
    step("a bare dependency key is a 400 before any write");
    let bare = replace_all(metadata, br#""smoke/nodep""#, br#""nodep""#);
    smoke.publish(inputs.publish_path, &frame(&bare, archive), &[400])?;
    smoke.expect_body("canonical <scope>/<name> names")?;
    // The refused attempt still charged the publish bucket (the rate
    // limit sits before validation); refund it so the downstream legs
    // keep the budget they were written against.
    refund()
}

/// `body` is the first publish's, which is admitted right below under a
/// well-formed id: the id is the only thing these two requests lack.
fn upload_id_required(smoke: &mut Smoke, inputs: &PublishInputs<'_>, body: &[u8]) -> Result<()> {
    step("a publish without a well-formed Cabin-Upload-Id is a 400");
    smoke.wrequest("PUT", inputs.publish_path, body, &[400])?;
    // The right length, in the wrong case.
    let upper = "0123456789ABCDEF0123456789ABCDEF";
    smoke.publish_as(upper, inputs.publish_path, body, &[400])?;
    refund()
}

/// Returns the framed body and the upload id it was published under.
fn first_publish(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<(Vec<u8>, String)> {
    step("first publish creates the version pending verification");
    let body = frame(metadata, archive);
    // Later legs re-send this exact file, so it stays on disk as well
    // as in hand.
    write(&inputs.work.join("publish.bin"), &body)?;
    let upload_id = smoke.publish(inputs.publish_path, &body, &[201])?;
    smoke.expect_body(r#""ok":true"#)?;
    smoke.expect_body(&format!(r#""name":"{}/{}""#, inputs.scope, inputs.name))?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_upload(&upload_id, inputs.work)?.as_deref() != Some(archive) {
        bail!("the published archive is not in UPLOADS under its upload id");
    }
    Ok((body, upload_id))
}

/// A request that dies between its admission and its store leaves a
/// reservation: a pending row with no `stored_at` and no object.  That
/// state is written here directly, since no request can be killed at
/// that point from outside.  The same operation sent again must store
/// the archive and confirm the row; the verifier's listing and download
/// below only see a confirmed one.  A stored upload has no such path:
/// its retry is the no-op the last leg of this span checks.
fn unstored_reservation(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    publish_body: &[u8],
    upload_id: &str,
    archive: &[u8],
) -> Result<()> {
    step("the same upload sent again stores what its reservation is missing");
    d1_quiet(&format!(
        "UPDATE revisions SET stored_at = NULL WHERE upload_id = '{upload_id}';"
    ))?;
    delete_upload(upload_id, inputs.work)?;
    smoke.publish_as(upload_id, inputs.publish_path, publish_body, &[200])?;
    smoke.expect_body(r#""no_op":true"#)?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_upload(upload_id, inputs.work)?.as_deref() != Some(archive) {
        bail!("the retry did not store the reservation's archive");
    }
    Ok(())
}

fn twin_and_reserved_names(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<()> {
    step("reserved and -/_ twin package names are 400s");
    // The fixture pair renamed to `with-dep` (name and source path; the
    // archive bytes and checksum are untouched) creates the twinnable
    // package; its `_` twin must then be the deterministic 400 with no
    // second row.
    let twin = replace_all(metadata, b"withdep", b"with-dep");
    let path = format!(
        "/api/v1/packages/{}/with-dep/{}",
        inputs.scope, inputs.version
    );
    smoke.publish(&path, &frame(&twin, archive), &[201])?;

    let under = replace_all(metadata, b"withdep", b"with_dep");
    let path = format!(
        "/api/v1/packages/{}/with_dep/{}",
        inputs.scope, inputs.version
    );
    smoke.publish(&path, &frame(&under, archive), &[400])?;
    smoke.expect_body("differs only in")?;

    // A reserved name answers in the same validation 400 family.
    let reserved = replace_all(metadata, b"withdep", b"con");
    let path = format!("/api/v1/packages/{}/con/{}", inputs.scope, inputs.version);
    smoke.publish(&path, &frame(&reserved, archive), &[400])?;
    smoke.expect_body("reserved")?;
    // Like the bare-dep leg: refund the extra publish charges so the
    // downstream legs keep the budget they were written against.
    refund()
}

fn pending_invisibility(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("pending versions are invisible to no-verify tokens");
    // The no-verify subject is the CI publish-arm credential: the
    // seeded publisher session carries verify, so this is the valid
    // token pending must stay missing for.  Its 404s are byte-identical:
    // pending, rejected, and unknown stay indistinguishable from missing
    // without a verify scope.  (Anonymous readers never get this far:
    // the private-alpha 401 answers them first.)
    smoke.as_ci_publisher();
    smoke.check(inputs.package_path, &[404])?;
    let pending_404 = smoke.body.clone();
    smoke.check(inputs.artifact_path, &[404])?;
    if smoke.body != pending_404 {
        bail!(
            "no-verify pending 404s differ between routes: {}",
            text(&smoke.body)
        );
    }
    smoke.check("/packages/smoke/never-published.json", &[404])?;
    if smoke.body != pending_404 {
        bail!(
            "an unknown package's no-verify 404 differs from a pending one: {}",
            text(&smoke.body)
        );
    }
    smoke.check("/catalog.json", &[200])?;
    if contains(
        &smoke.body,
        format!(r#""name":"{}""#, scoped_name(inputs)).as_bytes(),
    ) {
        bail!("the catalog lists a pending package: {}", text(&smoke.body));
    }
    smoke.as_publisher();
    // The source viewer gates on verified the same way; a valid range
    // makes sure the 404 is the gate, not the range policy (checked
    // first).
    let source = format!(
        "/api/v1/user/source/{}/{}/{}",
        inputs.scope, inputs.name, inputs.version
    );
    session_get(
        smoke,
        inputs.session_cookie,
        &source,
        404,
        &[("Range".to_owned(), "bytes=-22".to_owned())],
    )?;
    // So do search and the package routes: a pending-only package has
    // no hits, no detail, and no dependents.
    let cookie = inputs.session_cookie;
    session_get(smoke, cookie, &search_path(inputs), 200, &[])?;
    smoke.expect_body(r#""results":[]"#)?;
    let package = format!("/api/v1/user/package/{}/{}", inputs.scope, inputs.name);
    session_get(smoke, cookie, &package, 404, &[])?;
    session_get(
        smoke,
        cookie,
        &format!("{package}/reverse-dependencies"),
        404,
        &[],
    )?;

    smoke.as_ci_publisher();
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.wcheck("/api/v1/admin/packages", &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.as_publisher();
    write(&inputs.work.join("verdict-unbound.json"), VERDICT_UNBOUND)?;
    // The verdict route takes no registry token at all: its credential
    // is the verifier workflow's OIDC JWT, so the publisher's bearer is
    // the uniform 401, never a scope 403.
    let publisher = smoke.auth.clone();
    uniform_401_with(
        smoke,
        Base::Web,
        "PATCH",
        &admin_version(inputs),
        &publisher,
        Some(VERDICT_UNBOUND),
    )
}

/// Returns the pending listing the advisory leg reads.
fn verify_scope_listing(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<Vec<u8>> {
    step("the verify scope lists and downloads pending versions");
    smoke.as_verifier();
    // Content-Length is only an optimization: a chunked request must
    // hit the same cap while the stream is read and leave the pending
    // row untouched.  The body must be a *semantically valid* rejected
    // verdict - bound to the pending revision's checksum and publish
    // event (every verdict requires both), padded past the cap with
    // whitespace inside the JSON document - so an uncapped handler
    // would parse and apply it, failing the pending-row checks below.
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    let listing = smoke.body.clone();
    let precap = listing_entry(
        &listing,
        &scoped_name(inputs),
        inputs.version,
        &inputs.work.join("entry-precap.json"),
    )?;
    let binding = |key| {
        precap
            .get(key)
            .map_or_else(|| "undefined".to_owned(), display)
    };
    let oversized = oversized_verdict(&binding("checksum"), &binding("published_at"));
    write(&inputs.work.join("oversized-verdict.json"), &oversized)?;

    let url = smoke.url(Base::Web, &admin_version(inputs));
    // Authenticated like the workflow (the cap must be what refuses,
    // not the credential), chunked so no Content-Length shortcut hides
    // an uncapped stream read. The jti burns on the 400; every verdict
    // below mints its own fresh JWT anyway.
    let jwt = smoke.mint_verifier_jwt("{}")?;
    let headers = vec![
        ("Authorization".to_owned(), format!("Bearer {jwt}")),
        ("Transfer-Encoding".to_owned(), "chunked".to_owned()),
    ];
    let status = smoke.http("PATCH", &url, &headers, Some(&oversized))?;
    if status != 400 {
        bail!(
            "oversized chunked verdict returned {status}, expected 400 (body: {})",
            text(&smoke.body)
        );
    }
    smoke.expect_body("the verdict body must be")?;

    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    smoke.expect_body(&format!(r#""name":"{}""#, scoped_name(inputs)))?;
    smoke.expect_body(r#""version":"0.2.0""#)?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""published_by":2"#)?;
    smoke.expect_body(r#""metadata":{"#)?;
    let pending = smoke.body.clone();
    write(&inputs.work.join("pending.json"), &pending)?;
    smoke.wcheck("/api/v1/admin/versions?status=bogus", &[400])?;

    // The corpus the name advisories read: every package, ordered, with
    // its vetted (any-version-verified) bit - both fixtures still
    // pending.
    smoke.wcheck("/api/v1/admin/packages", &[200])?;
    smoke.expect_body(r#""packages":["#)?;
    smoke.expect_body(&vetted_row(inputs, "with-dep", false))?;
    smoke.expect_body(&vetted_row(inputs, inputs.name, false))?;
    write(&inputs.work.join("corpus.json"), &smoke.body.clone())?;

    // The verifier downloads the pending artifact and inspects it out
    // of band.
    let url = smoke.url(Base::Registry, inputs.artifact_path);
    let (_, download) = detached_get(smoke, &url, false)?;
    write(&inputs.work.join("download.zip"), &download)?;
    if sha256_hex(&download) != inputs.blob_hash {
        bail!("the pending download differs from the published archive");
    }
    Ok(pending)
}

fn unbound_verdict(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("a verified verdict must name the listing it inspected");
    smoke.verdict_patch(&admin_version(inputs), VERDICT_UNBOUND, &[400])?;
    smoke.expect_body("requires the checksum")
}

/// Returns the listing entry every verdict below binds to.
fn advisory_abstain(inputs: &PublishInputs<'_>, pending: &[u8]) -> Result<Map<String, Value>> {
    step("the advisory gate abstains on the skeleton-equal fixture pair");
    // `withdep` and `with-dep` fold to the same skeleton, and neither is
    // vetted yet, so the workflow's pre-download gate abstains on the
    // real corpus - exercised through the real binary.  The bound
    // verdict below then plays the operator's manual resolution from the
    // runbook.
    let entry_path = inputs.work.join("entry.json");
    let entry = listing_entry(pending, &scoped_name(inputs), inputs.version, &entry_path)?;
    let corpus = inputs.work.join("corpus.json");
    let (ok, advice) = verifier(
        inputs.verifier_bin,
        &[
            OsStr::new("--name-advisories"),
            entry_path.as_os_str(),
            corpus.as_os_str(),
        ],
    )?;
    write(&inputs.work.join("advice.json"), &advice)?;
    if !ok {
        bail!("the advisory mode failed operationally: {}", text(&advice));
    }
    if !contains(&advice, br#""advice":"abstain""#) {
        bail!("the skeleton-equal pair did not abstain: {}", text(&advice));
    }
    if !contains(
        &advice,
        format!("confusable_package ({}/with-dep)", inputs.scope).as_bytes(),
    ) {
        bail!("the abstain does not name its rule: {}", text(&advice));
    }
    Ok(entry)
}

/// Returns the bound verified verdict, which two later legs re-send
/// byte for byte.
fn real_verification(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    entry: &Map<String, Value>,
) -> Result<Vec<u8>> {
    step("the real verifier verifies the fixture and the verdict makes it resolvable");
    let real = run_verifier(
        inputs.verifier_bin,
        &inputs.work.join("download.zip"),
        &inputs.work.join("entry.json"),
        &inputs.work.join("verdict-real.json"),
    )?;
    if !contains(&real, br#""verdict":"verified""#) {
        bail!("the verifier did not verify the fixture: {}", text(&real));
    }
    // The verdict binds to the checksum and published_at the listing
    // reported.
    let verdict = verdict_body(entry, "verified", None)?;
    write(&inputs.work.join("verdict-verified.json"), &verdict)?;
    smoke.verdict_patch(&admin_version(inputs), &verdict, &[200])?;
    smoke.expect_body(r#""verification":"verified""#)?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""changed":true"#)?;

    smoke.as_publisher();
    smoke.check("/catalog.json", &[200])?;
    smoke.expect_body(&format!(
        r#""name":"{}","latest":"{}","versions":[{{"version":"{}","revision":"{}""#,
        scoped_name(inputs),
        inputs.version,
        inputs.version,
        inputs.rev
    ))?;
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(&format!(r#""name":"{}""#, scoped_name(inputs)))?;
    smoke.expect_body(r#""0.2.0""#)?;
    // The composed entry names the revision it serves, and lists it in
    // the per-version revisions map with the canonical source path.
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(&format!(
        "../../archives/{scope}/{name}/{scope}-{name}-{version}-{rev}.zip",
        scope = inputs.scope,
        name = inputs.name,
        version = inputs.version,
        rev = inputs.rev
    ))?;
    // This is the first verified download (a cache-miss fill), so it
    // also proves the outward answer to an authenticated request never
    // licenses a shared cache: the `public` freshness header lives only
    // on the internal cache copy, and the client sees no-store.
    let url = smoke.url(Base::Registry, inputs.artifact_path);
    let (status, _) = detached_get(smoke, &url, true)?;
    if status != 200 {
        bail!("the first verified download returned {status}");
    }
    let block = text(&smoke.headers);
    if grep_lines(&block, "cache-control: no-store").is_empty() {
        bail!(
            "a cache-miss artifact is missing the outward no-store: {}",
            grep_anywhere(&block, "cache-control").join("\n")
        );
    }
    Ok(verdict)
}

fn corpus_vetted(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("a verified version flips the corpus row to vetted");
    smoke.as_verifier();
    smoke.wcheck("/api/v1/admin/packages", &[200])?;
    smoke.expect_body(&vetted_row(inputs, inputs.name, true))?;
    smoke.expect_body(&vetted_row(inputs, "with-dep", false))?;
    smoke.as_publisher();
    Ok(())
}

fn search_and_package_routes(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("search and the package routes see the verified version");
    let cookie = inputs.session_cookie;
    session_get(smoke, cookie, &search_path(inputs), 200, &[])?;
    smoke.expect_body(&format!(
        r#""scope":"{}","name":"{}","version":"{}""#,
        inputs.scope, inputs.name, inputs.version
    ))?;
    // A whitespace-only query is the fixed 400 detail.
    session_get(smoke, cookie, "/api/v1/user/search?q=%20", 400, &[])?;
    smoke.expect_body("1 to 64 characters")?;
    let package = format!("/api/v1/user/package/{}/{}", inputs.scope, inputs.name);
    session_get(smoke, cookie, &package, 200, &[])?;
    smoke.expect_body(&format!(r#""newest_version":"{}""#, inputs.version))?;
    smoke.expect_body(r#""smoke/nodep":"^0.1""#)?;
    session_get(
        smoke,
        cookie,
        &format!("{package}/reverse-dependencies"),
        200,
        &[],
    )?;
    smoke.expect_body(r#""dependents":[]"#)?;
    // The fixture's dependency itself was never published: an invisible
    // target is the authenticated 404, before any dependents walk.
    session_get(
        smoke,
        cookie,
        "/api/v1/user/package/smoke/nodep/reverse-dependencies",
        404,
        &[],
    )
}

fn verdict_idempotence(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    entry: &Map<String, Value>,
    verdict_verified: &[u8],
) -> Result<()> {
    step("verdicts are idempotent for the same value and conflict otherwise");
    smoke.as_verifier();
    smoke.verdict_patch(&admin_version(inputs), verdict_verified, &[200])?;
    smoke.expect_body(r#""changed":false"#)?;
    // Bound to the verified revision's checksum and publish event, which
    // every verdict requires: the conflict must be the transition, not a
    // missing binding.
    let rejected = verdict_body(entry, "rejected", Some("smoke rejection"))?;
    write(&inputs.work.join("verdict-rejected.json"), &rejected)?;
    smoke.verdict_patch(&admin_version(inputs), &rejected, &[409])?;
    smoke.expect_body("immutable")?;
    smoke.as_publisher();
    Ok(())
}

fn download_counting(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("verified downloads count; the verifier's pending fetch never did");
    // The counter lands off the response path (waitUntil), so poll the
    // version row itself - per-row, so verified packages left in the
    // local state by other work never skew the expectation.  The row was
    // recreated by this run's publish, and exactly one verified download
    // has happened, while the verifier fetched the artifact when it was
    // still pending - so 1 here also proves the pending fetch never
    // counted.
    await_row_downloads("1")?;
    // The totals (an admitted bearer caller's) reflect served downloads;
    // >= keeps the assertion meaningful whatever else the local state
    // holds.
    let url = smoke.url(Base::Web, "/api/v1/stats");
    let auth = smoke.auth.clone();
    smoke.http("GET", &url, &auth, None)?;
    if !stats_are_counted(&smoke.body) {
        bail!(
            "stats totals do not reflect the verified download: {}",
            text(&smoke.body)
        );
    }
    smoke.check(inputs.artifact_path, &[200])?;
    await_row_downloads("2")
}

/// The private alpha holds after an admitted caller warmed the edge
/// blob cache: neither an anonymous caller nor a valid token whose
/// account the allowlist does not admit reads the verified version, by
/// either route, and the outsider's token buys nothing on the website
/// origin either.
fn closed_to_outsiders(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("the warm verified version stays closed to anonymous and unadmitted callers");
    for path in [inputs.package_path, inputs.artifact_path] {
        uniform_401_with(smoke, Base::Registry, "GET", path, &[], None)?;
    }
    smoke.as_outsider();
    let outsider = smoke.auth.clone();
    for (at, method, path) in [
        (Base::Registry, "GET", "/catalog.json"),
        (Base::Registry, "GET", inputs.package_path),
        (Base::Registry, "GET", inputs.artifact_path),
        (Base::Registry, "GET", "/config.json"),
        (Base::Web, "PUT", inputs.publish_path),
        (Base::Web, "GET", "/api/v1/stats"),
    ] {
        let url = smoke.url(at, path);
        let body: Option<&[u8]> = (method == "PUT").then_some(b"x");
        let status = smoke.http(method, &url, &outsider, body)?;
        if status != 403 || capture(&smoke.body) != PRIVATE_ALPHA {
            bail!(
                "an unadmitted token's {method} {path} answered {status}: {}",
                text(&smoke.body)
            );
        }
    }
    // A cache-hit download by an admitted caller: served, counted, and
    // the outward answer stays no-store (the Worker-internal cache is
    // the one caching layer that keeps the counter accurate and the
    // admission check in front of it).
    smoke.as_ci_publisher();
    let auth = smoke.auth.clone();
    let url = smoke.url(Base::Registry, "/config.json");
    let status = smoke.http("PUT", &url, &auth, None)?;
    if status != 405 {
        bail!("an admitted PUT /config.json returned {status}, expected 405");
    }
    let url = smoke.url(Base::Registry, inputs.artifact_path);
    let (status, _) = detached_get(smoke, &url, true)?;
    if status != 200 {
        bail!("an admitted verified download returned {status}");
    }
    let block = text(&smoke.headers);
    if grep_lines(&block, "cache-control: no-store").is_empty() {
        bail!(
            "an admitted download is missing the outward no-store: {}",
            grep_anywhere(&block, "cache-control").join("\n")
        );
    }
    await_row_downloads("3")?;
    smoke.as_publisher();
    Ok(())
}

fn byte_identical_no_op(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    publish_body: &[u8],
) -> Result<()> {
    step("byte-identical re-publish is an idempotent no-op reporting the status");
    smoke.publish(inputs.publish_path, publish_body, &[200])?;
    smoke.expect_body(r#""no_op":true"#)?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""verification":"verified""#)
}

// --- Helpers. ---

/// The one admin-listing element the verifier binary consumes,
/// written to `out` - the six keys in this insertion order,
/// a key the listing does not carry omitted rather than nulled
/// (`preserve_order` is on workspace-wide, so a `Map` round trip keeps
/// both the outer order and `metadata`'s own).
///
/// The parsed entry comes back because two verdict bodies and the
/// oversized one bind to its `checksum` and `published_at`.
fn listing_entry(
    listing: &[u8],
    name: &str,
    version: &str,
    out: &Path,
) -> Result<Map<String, Value>> {
    let document: Value = serde_json::from_slice(listing)
        .with_context(|| format!("the pending listing has no {name}@{version}"))?;
    let found = document
        .get("versions")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .find(|candidate| {
            candidate.get("name").and_then(Value::as_str) == Some(name)
                && candidate.get("version").and_then(Value::as_str) == Some(version)
        })
        .cloned()
        .with_context(|| format!("the pending listing has no {name}@{version}"))?;

    let mut entry = Map::new();
    for key in [
        "name",
        "version",
        "revision",
        "checksum",
        "published_at",
        "metadata",
    ] {
        if let Some(value) = found.get(key) {
            entry.insert(key.to_owned(), value.clone());
        }
    }
    write(out, &serde_json::to_vec(&Value::Object(entry.clone()))?)?;
    Ok(entry)
}

/// The built binary's JSON verdict, written to `out`.  Exit 2 is an
/// operational failure with no verdict, which must abort the run
/// rather than pass silently.
///
/// The verdict comes back because the caller greps it.
fn run_verifier(bin: &Path, archive: &Path, entry: &Path, out: &Path) -> Result<Vec<u8>> {
    let (ok, verdict) = verifier(bin, &[archive.as_os_str(), entry.as_os_str()])?;
    write(out, &verdict)?;
    if !ok {
        bail!(
            "the verifier binary failed operationally on {}: {}",
            archive.display(),
            text(&verdict)
        );
    }
    Ok(verdict)
}

/// The verifier binary with `args`, stdout captured and stderr left on
/// the operator's terminal.
fn verifier(bin: &Path, args: &[&OsStr]) -> Result<(bool, Vec<u8>)> {
    let run = Command::new(bin)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("run {}", bin.display()))?;
    Ok((run.status.success(), run.stdout))
}

/// A session GET: the website origin, the session cookie, one
/// accepted status, and a fail wording of its own - it names the single
/// expectation where `check`/`request` list every accepted status.
fn session_get(
    smoke: &mut Smoke,
    cookie: &str,
    path: &str,
    expected: u16,
    extra: &[(String, String)],
) -> Result<()> {
    let url = smoke.url(Base::Web, path);
    let mut headers = vec![("Cookie".to_owned(), cookie.to_owned())];
    headers.extend_from_slice(extra);
    let status = smoke.http("GET", &url, &headers, None)?;
    if status != expected {
        bail!(
            "GET {path} returned {status}, expected {expected} (body: {})",
            text(&smoke.body)
        );
    }
    println!("    GET {path} -> {status}");
    Ok(())
}

/// A GET whose body is returned rather than stored: the shared body
/// buffer keeps what the previous request left in it.  `dump_headers`
/// also writes [`Smoke::headers`].
fn detached_get(smoke: &mut Smoke, url: &str, dump_headers: bool) -> Result<(u16, Vec<u8>)> {
    let auth = smoke.auth.clone();
    let previous_body = std::mem::take(&mut smoke.body);
    let previous_headers = smoke.headers.clone();
    let status = smoke.http("GET", url, &auth, None)?;
    let received = std::mem::replace(&mut smoke.body, previous_body);
    if !dump_headers {
        smoke.headers = previous_headers;
    }
    Ok((status, received))
}

fn await_row_downloads(expected: &str) -> Result<()> {
    let mut row_downloads = String::new();
    for _ in 0..POLLS {
        row_downloads = row_downloads_now()?;
        if row_downloads == expected {
            println!("    downloads(smoke/withdep@0.2.0) = {expected}");
            return Ok(());
        }
        sleep(HALF_SECOND);
    }
    bail!("smoke/withdep@0.2.0 downloads never reached {expected} (last: {row_downloads})")
}

/// The `downloads` column of the one fixture row.
fn row_downloads_now() -> Result<String> {
    let json = d1_json(
        "SELECT downloads FROM versions
       WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'",
    )?;
    column(&json, "downloads")
}

/// The first row's `name` column, as display text.
fn column(json: &str, name: &str) -> Result<String> {
    let rows = results(json)?;
    let row = rows.first().context("the query returned no row")?;
    let value = row
        .get(name)
        .with_context(|| format!("the query returned no {name} column"))?;
    Ok(display(value))
}

/// The publish-bucket refund on the smoke token's owner: the bucket is
/// the user's, not the token's.
fn refund() -> Result<()> {
    crate::servers::d1_quiet(
        "
  UPDATE users SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke');",
    )
}

// --- Pure shapes. ---

/// Padding *inside* an otherwise well-formed rejected verdict, so an
/// uncapped handler would parse and apply it and only the body cap can
/// be what refused.  Ten caps of it rather than one byte past: the
/// refused body then spans several chunked wire frames and runs past
/// any bound a drain might stop at, so the request after it succeeds
/// only if the worker read the body to its end.
const OVERSIZED_PADDING: usize = 10 * 4096;

fn oversized_verdict(checksum: &str, published_at: &str) -> Vec<u8> {
    format!(
        r#"{{"verdict":"rejected","reason":"oversized","checksum":"{checksum}","published_at":"{published_at}"{:pad$}}}"#,
        "",
        pad = OVERSIZED_PADDING
    )
    .into_bytes()
}

/// The bound verdict bodies: the keys in this insertion order, and a
/// binding the listing entry does not carry omitted rather than nulled.
fn verdict_body(
    entry: &Map<String, Value>,
    verdict: &str,
    reason: Option<&str>,
) -> Result<Vec<u8>> {
    let mut document = Map::new();
    document.insert("verdict".to_owned(), Value::String(verdict.to_owned()));
    if let Some(reason) = reason {
        document.insert("reason".to_owned(), Value::String(reason.to_owned()));
    }
    for key in ["checksum", "published_at"] {
        if let Some(value) = entry.get(key) {
            document.insert(key.to_owned(), value.clone());
        }
    }
    Ok(serde_json::to_vec(&Value::Object(document))?)
}

/// `packages`, `versions` and `downloads` must each be an integer of at
/// least 1: a fractional value or a non-number fails and a whole `1.0`
/// passes.
fn stats_are_counted(body: &[u8]) -> bool {
    let Ok(stats) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    ["packages", "versions", "downloads"].iter().all(|key| {
        stats
            .get(key)
            .and_then(Value::as_f64)
            .is_some_and(|total| total.fract() == 0.0 && total >= 1.0)
    })
}

fn scoped_name(inputs: &PublishInputs<'_>) -> String {
    format!("{}/{}", inputs.scope, inputs.name)
}

fn admin_version(inputs: &PublishInputs<'_>) -> String {
    format!(
        "/api/v1/admin/versions/{}/{}/{}",
        inputs.scope, inputs.name, inputs.version
    )
}

fn search_path(inputs: &PublishInputs<'_>) -> String {
    format!("/api/v1/user/search?q={}", inputs.name)
}

fn vetted_row(inputs: &PublishInputs<'_>, name: &str, vetted: bool) -> String {
    format!(
        r#""scope":"{}","name":"{name}","vetted":{vetted}"#,
        inputs.scope
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing field is omitted; a `null` binding is a different
    /// document and stays one.
    #[test]
    fn a_missing_binding_is_omitted_and_an_explicit_null_is_kept() {
        let mut sparse = Map::new();
        sparse.insert("published_at".to_owned(), Value::Null);
        let body = verdict_body(&sparse, "verified", None).expect("verdict");
        assert_eq!(
            String::from_utf8(body).expect("utf8"),
            r#"{"verdict":"verified","published_at":null}"#
        );
    }

    #[test]
    fn the_oversized_verdict_pads_inside_the_document() {
        let body = oversized_verdict("dead", "1970-01-01T00:00:00Z");
        let head = r#"{"verdict":"rejected","reason":"oversized","checksum":"dead","published_at":"1970-01-01T00:00:00Z""#;
        assert_eq!(body.len(), head.len() + OVERSIZED_PADDING + 1);
        assert!(body.starts_with(head.as_bytes()), "{}", text(&body));
        assert_eq!(
            &body[head.len()..head.len() + OVERSIZED_PADDING],
            vec![b' '; OVERSIZED_PADDING]
        );
        assert_eq!(body.last(), Some(&b'}'));
        // Padded past the cap, but still a document a handler that read
        // the whole stream would accept.
        serde_json::from_slice::<Value>(&body).expect("a valid rejected verdict");
    }

    #[test]
    fn the_stats_totals_must_be_whole_and_at_least_one() {
        assert!(stats_are_counted(
            br#"{"packages":1,"versions":2,"downloads":3}"#
        ));
        assert!(stats_are_counted(
            br#"{"packages":1.0,"versions":2,"downloads":3}"#
        ));
        assert!(!stats_are_counted(
            br#"{"packages":0,"versions":2,"downloads":3}"#
        ));
        assert!(!stats_are_counted(
            br#"{"packages":1.5,"versions":2,"downloads":3}"#
        ));
        assert!(!stats_are_counted(br#"{"packages":1,"versions":2}"#));
        assert!(!stats_are_counted(br#"{"packages":"1"}"#));
        assert!(!stats_are_counted(b"not json"));
    }
}
