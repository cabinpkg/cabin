//! The publish and verification core, `registry/scripts/smoke.sh`
//! L998-1330: the validation 400s that refuse before any write, the
//! first publish, the pending version's invisibility to every reader
//! without a verify scope, the verifier's listing/download/verdict
//! round trip, download counting, backup replication, and the
//! idempotent re-publish that heals the primary blob only.
//!
//! Three shapes here look like they could be tidied and cannot be.
//!
//! The **oversized verdict** (L1095-1104) must reach the Worker with
//! no `Content-Length`, because the leg's whole subject is that the
//! body cap holds while a stream is read.  `curl` got that from
//! `-H "Transfer-Encoding: chunked"`, and `ureq` reads the same header
//! back off the caller's request: with a user-set `chunked` encoding it
//! skips its own `Content-Length` and chunk-frames the body it was
//! given (`ureq-2.12.1/src/unit.rs:50-83`, pinned upstream by that
//! crate's own `content_length_and_chunked` test).  So the ordinary
//! [`Smoke::http`] carries this leg; `send(reader)` would work too, but
//! only because an unsized reader lands in the same branch.
//!
//! **The `sleep 1`s** at L1059 and L1318 precede *negative* assertions:
//! no backup blob for a pending version, and no backup rewrite by a
//! re-publish.  In-process HTTP outruns a forked `curl`, so trimming
//! either one turns a real assertion into a race that passes for the
//! wrong reason (plan §7.9).
//!
//! **`curl -o <file>` never wrote `$body`.**  The two artifact
//! downloads and the header-only fetches left the shared response
//! buffers holding what the *previous* request put there (plan §7.6),
//! so they go through `detached_get` rather than clobbering
//! [`Smoke::body`].
//!
//! `listing_entry`, `run_verifier` and `session_request` are the
//! shell's shared helpers (L704-767), not this span's; the copies below
//! are private and exist only so this module compiles on its own.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
// BLOBS_BUCKET and BACKUP_BUCKET are the two R2 buckets this span
// reads: the content-addressed primary, and the append-only backup set
// the verdict batch replicates into.
use xtask_registry_admin::{BACKUP_BUCKET, BLOBS_BUCKET, display, output, results, wrangler};

use crate::bytes::{frame, replace_all, sha256_hex};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::uniform_401_with;
use crate::servers::d1_json;
use crate::step;
use crate::text::{contains, read, write};

/// `--file /dev/null`: the three sites that only ask whether the object
/// exists still make `wrangler` write it somewhere.
const DEV_NULL: &str = "/dev/null";

/// Every poll budget and pause in this span, the shell's literally:
/// `await_row_downloads` (L1224-1239), `await_backup_blob` (L1270-1279)
/// and the `backup_pending` drain (L1287-1296) are each 20 × 0.5s, and
/// the two bare `sleep 1`s guard negative assertions.
const POLLS: u32 = 20;
const HALF_SECOND: Duration = Duration::from_millis(500);
const SETTLE: Duration = Duration::from_secs(1);

/// The unbound verdict body, `printf`'d at L1077 and sent twice: once
/// without the verify scope (403) and once with it (400).
const VERDICT_UNBOUND: &[u8] = br#"{"verdict":"verified"}"#;

/// What this span needs out of the run's setup - the shell's variables
/// as they stood at L998, with the fixtures as *paths* because
/// `wrangler r2 object put` takes `--file` and three `cmp -s` sites
/// read the archive again.
pub struct PublishInputs<'a> {
    /// `$work`, the run's `mktemp -d`.
    pub work: &'a Path,
    /// `$fixture_archive`.
    pub fixture_archive: &'a Path,
    /// `$fixture_metadata`.
    pub fixture_metadata: &'a Path,
    /// `$verifier_bin`, already built.  Absolute: the shell's
    /// `../target/debug/cabin-registry-verify` resolved against
    /// `registry/`, which is not this process's working directory.
    pub verifier_bin: &'a Path,
    /// `$session_cookie`, the real one restored at L796.
    pub session_cookie: &'a str,
    /// `$scope`, `$name`, `$version` - `smoke`, `withdep`, `0.2.0`.
    pub scope: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    /// `$rev`, the archive's packaging revision id.
    pub rev: &'a str,
    /// `$blob_hash`, the archive's full SHA-256.
    pub blob_hash: &'a str,
    /// `$publish_path`, `$package_path`, `$artifact_path`.
    pub publish_path: &'a str,
    pub package_path: &'a str,
    pub artifact_path: &'a str,
}

/// The whole publish/verify core, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    let metadata = read(inputs.fixture_metadata)?;
    let archive = read(inputs.fixture_archive)?;

    bare_dependency_key(smoke, inputs, &metadata, &archive)?;
    let publish_body = first_publish(smoke, inputs, &metadata, &archive)?;
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
    anonymous_counting(smoke, inputs)?;
    backup_replication(inputs)?;
    heal_primary_only(smoke, inputs, &publish_body)?;
    byte_identical_no_op(smoke, inputs, &publish_body)
}

/// L998-1011.
fn bare_dependency_key(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<()> {
    step("a bare dependency key is a 400 before any write");
    // The one substitution the shell left ungreedy (`sed` without `g`,
    // so first-per-line): the scoped name occurs once in the frozen
    // fixture, which the setup's own `grep -qF` already pinned.
    let bare = replace_all(metadata, br#""smoke/nodep""#, br#""nodep""#);
    smoke.wrequest("PUT", inputs.publish_path, &frame(&bare, archive), &[400])?;
    smoke.expect_body("canonical <scope>/<name> names")?;
    // The refused attempt still charged the publish bucket (the rate
    // limit sits before validation); refund it so the downstream legs
    // keep the budget they were written against.
    refund()
}

/// L1012-1019.
fn first_publish(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<Vec<u8>> {
    step("first publish creates the version pending verification");
    let body = frame(metadata, archive);
    // Later legs re-send this exact file (L1505, L1537, L1573, L1732),
    // so it stays on disk as well as in hand.
    write(&inputs.work.join("publish.bin"), &body)?;
    smoke.wrequest("PUT", inputs.publish_path, &body, &[201])?;
    smoke.expect_body(r#""ok":true"#)?;
    smoke.expect_body(&format!(r#""name":"{}/{}""#, inputs.scope, inputs.name))?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    Ok(body)
}

/// L1020-1041.
fn twin_and_reserved_names(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    metadata: &[u8],
    archive: &[u8],
) -> Result<()> {
    step("reserved and -/_ twin package names are 400s");
    // The fixture pair renamed to `with-dep` (name and source path; the
    // archive bytes and checksum are untouched, and the shared blob is
    // not re-counted) creates the twinnable package; its `_` twin must
    // then be the deterministic 400 with no second row.
    let twin = replace_all(metadata, b"withdep", b"with-dep");
    let path = format!(
        "/api/v1/packages/{}/with-dep/{}",
        inputs.scope, inputs.version
    );
    smoke.wrequest("PUT", &path, &frame(&twin, archive), &[201])?;

    let under = replace_all(metadata, b"withdep", b"with_dep");
    let path = format!(
        "/api/v1/packages/{}/with_dep/{}",
        inputs.scope, inputs.version
    );
    smoke.wrequest("PUT", &path, &frame(&under, archive), &[400])?;
    smoke.expect_body("differs only in")?;

    // A reserved name answers in the same validation 400 family.
    let reserved = replace_all(metadata, b"withdep", b"con");
    let path = format!("/api/v1/packages/{}/con/{}", inputs.scope, inputs.version);
    smoke.wrequest("PUT", &path, &frame(&reserved, archive), &[400])?;
    smoke.expect_body("reserved")?;
    // Like the bare-dep leg: refund the extra publish charges so the
    // downstream legs keep the budget they were written against.
    refund()
}

/// L1042-1079.
fn pending_invisibility(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("pending versions are invisible to ordinary tokens and anonymous readers");
    smoke.check(inputs.package_path, &[404])?;
    smoke.check(inputs.artifact_path, &[404])?;
    // Anonymous readers see byte-identical 404s: pending, rejected, and
    // unknown stay indistinguishable from missing without a verify
    // scope.
    smoke.anonymous();
    smoke.check(inputs.package_path, &[404])?;
    let anon_pending_404 = smoke.body.clone();
    smoke.check(inputs.artifact_path, &[404])?;
    if smoke.body != anon_pending_404 {
        bail!(
            "anonymous pending 404s differ between routes: {}",
            text(&smoke.body)
        );
    }
    smoke.check("/packages/smoke/never-published.json", &[404])?;
    if smoke.body != anon_pending_404 {
        bail!(
            "an unknown package's anonymous 404 differs from a pending one: {}",
            text(&smoke.body)
        );
    }
    smoke.as_publisher();
    // And they have no backup copy: only versions that become verified
    // enter the durable backup set (the verdict batch enqueues the
    // work).
    sleep(SETTLE);
    if r2_get(BACKUP_BUCKET, &blob_key(inputs), Path::new(DEV_NULL), true)? {
        bail!("a pending version's blob was replicated to the BACKUP bucket");
    }
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

    smoke.wcheck("/api/v1/admin/versions?status=pending", &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.wcheck("/api/v1/admin/packages", &[403])?;
    smoke.expect_body("verify scope")?;
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

/// L1081-1123.  Returns the pending listing the advisory leg reads.
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
    let published_at = precap
        .get("published_at")
        .map_or_else(|| "undefined".to_owned(), display);
    let oversized = oversized_verdict(inputs.blob_hash, &published_at);
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
    smoke.expect_body(r#""published_by":1"#)?;
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

/// L1125-1127.
fn unbound_verdict(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("a verified verdict must name the listing it inspected");
    smoke.verdict_patch(&admin_version(inputs), VERDICT_UNBOUND, &[400])?;
    smoke.expect_body("requires the checksum")
}

/// L1129-1141.  Returns the listing entry every verdict below binds to.
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

/// L1142-1175.  Returns the bound verified verdict, which two later
/// legs re-send byte for byte.
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
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(&format!(r#""name":"{}""#, scoped_name(inputs)))?;
    smoke.expect_body(r#""0.2.0""#)?;
    // The composed entry names the revision it serves, and lists it in
    // the per-version revisions map with the canonical source path.
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(&format!(
        "../../artifacts/{scope}/{name}/{scope}-{name}-{version}-{rev}.zip",
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
    if !header_starts_with(&smoke.headers, "cache-control: no-store") {
        bail!(
            "a cache-miss artifact is missing the outward no-store: {}",
            grep_lines(&smoke.headers, "cache-control")
        );
    }
    Ok(verdict)
}

/// L1176-1182.
fn corpus_vetted(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("a verified version flips the corpus row to vetted");
    smoke.as_verifier();
    smoke.wcheck("/api/v1/admin/packages", &[200])?;
    smoke.expect_body(&vetted_row(inputs, inputs.name, true))?;
    smoke.expect_body(&vetted_row(inputs, "with-dep", false))?;
    smoke.as_publisher();
    Ok(())
}

/// L1183-1197.
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

/// L1198-1215.
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
    // every verdict now requires: the conflict must be the transition,
    // not a missing binding.
    let rejected = verdict_body(entry, "rejected", Some("smoke rejection"))?;
    write(&inputs.work.join("verdict-rejected.json"), &rejected)?;
    smoke.verdict_patch(&admin_version(inputs), &rejected, &[409])?;
    smoke.expect_body("immutable")?;
    smoke.as_publisher();
    Ok(())
}

/// L1216-1251.
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
    // The public totals reflect served downloads; >= keeps the assertion
    // meaningful whatever else the local state holds.
    let url = smoke.url(Base::Web, "/api/v1/stats");
    smoke.http("GET", &url, &[], None)?;
    if !stats_are_counted(&smoke.body) {
        bail!(
            "stats totals do not reflect the verified download: {}",
            text(&smoke.body)
        );
    }
    smoke.check(inputs.artifact_path, &[200])?;
    await_row_downloads("2")
}

/// L1252-1281.
fn anonymous_counting(smoke: &mut Smoke, inputs: &PublishInputs<'_>) -> Result<()> {
    step("anonymous readers see the verified version, and their downloads count");
    smoke.anonymous();
    smoke.check(inputs.package_path, &[200])?;
    smoke.expect_body(&format!(r#""name":"{}""#, scoped_name(inputs)))?;
    // A cache-hit download without a credential: served, counted, and
    // the outward answer stays no-store (the Worker-internal cache is
    // the one caching layer that keeps the counter accurate).
    let url = smoke.url(Base::Registry, inputs.artifact_path);
    let (status, _) = detached_get(smoke, &url, true)?;
    if status != 200 {
        bail!("an anonymous verified download returned {status}");
    }
    if !header_starts_with(&smoke.headers, "cache-control: no-store") {
        bail!(
            "an anonymous download is missing the outward no-store: {}",
            grep_lines(&smoke.headers, "cache-control")
        );
    }
    await_row_downloads("3")?;
    smoke.as_publisher();
    Ok(())
}

/// L1282-1303.
fn backup_replication(inputs: &PublishInputs<'_>) -> Result<()> {
    // The verdict batch enqueued the backup work transactionally with
    // the verified transition, and the verdict's waitUntil drain
    // replicated it.
    step("the verified blob replicates to the BACKUP bucket and drains its queue row");
    let replicated = inputs.work.join("replicated.zip");
    await_backup_blob(&blob_key(inputs), &replicated)?;
    if read(&replicated)? != read(inputs.fixture_archive)? {
        bail!("replicated blob differs from the published archive");
    }
    let mut queue_rows = String::new();
    for _ in 0..POLLS {
        queue_rows = backup_pending_rows(inputs)?;
        if queue_rows == "0" {
            break;
        }
        sleep(HALF_SECOND);
    }
    if queue_rows != "0" {
        bail!("the drained backup queue row was not deleted");
    }
    Ok(())
}

/// L1304-1326.
fn heal_primary_only(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    publish_body: &[u8],
) -> Result<()> {
    // The backup set is durable through the queue, not through
    // re-publish: an idempotent no-op heals a reclaim-raced primary blob
    // (the retry holds the bytes), while the append-only backup bucket
    // is never rewritten by the publish path - a lost backup object is
    // `cargo registry-backup-backfill` territory.
    step("an idempotent re-publish heals the primary blob only");
    let key = blob_key(inputs);
    r2_delete(BACKUP_BUCKET, &key)?;
    r2_delete(BLOBS_BUCKET, &key)?;
    smoke.wrequest("PUT", inputs.publish_path, publish_body, &[200])?;
    smoke.expect_body(r#""no_op":true"#)?;
    smoke.expect_body(r#""verification":"verified""#)?;
    // The heal runs before the response, so the primary object itself is
    // back (the artifact route could otherwise answer from the edge
    // cache).
    let healed = inputs.work.join("healed.zip");
    if !r2_get(BLOBS_BUCKET, &key, &healed, false)? {
        bail!("the idempotent re-publish did not heal the primary blob");
    }
    if read(&healed)? != read(inputs.fixture_archive)? {
        bail!("the healed primary blob differs from the published archive");
    }
    smoke.check(inputs.artifact_path, &[200])?;
    sleep(SETTLE);
    if r2_get(BACKUP_BUCKET, &key, Path::new(DEV_NULL), true)? {
        bail!("a re-publish rewrote the append-only BACKUP bucket");
    }
    // Put the copy back so later legs and the local state stay coherent.
    r2_put(BACKUP_BUCKET, &key, inputs.fixture_archive)
}

/// L1327-1331.
fn byte_identical_no_op(
    smoke: &mut Smoke,
    inputs: &PublishInputs<'_>,
    publish_body: &[u8],
) -> Result<()> {
    step("byte-identical re-publish is an idempotent no-op reporting the status");
    smoke.wrequest("PUT", inputs.publish_path, publish_body, &[200])?;
    smoke.expect_body(r#""no_op":true"#)?;
    smoke.expect_body(&format!(r#""revision":"{}""#, inputs.rev))?;
    smoke.expect_body(r#""verification":"verified""#)
}

// --- Helpers. ---

/// `listing_entry` (L704-715): the one admin-listing element the
/// verifier binary consumes, written to `out` exactly as
/// `JSON.stringify` wrote it - the six keys in that insertion order,
/// a key the listing does not carry omitted rather than nulled
/// (`preserve_order` is on workspace-wide, so a `Map` round trip keeps
/// both the outer order and `metadata`'s own).
///
/// The parsed entry comes back because two verdict bodies and the
/// oversized one bind to its `checksum` and `published_at`; the shell
/// read those back off the file it had just written.
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

/// `run_verifier` (L720-723): the built binary's JSON verdict, written
/// to `out` as the shell's redirect wrote it.  Exit 2 is an operational
/// failure with no verdict, which must abort the run rather than pass
/// silently.
///
/// The verdict comes back because the caller greps it; the shell
/// grepped the file it had just written.
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

/// The verifier binary with `args`, stdout captured (the shell
/// redirected it into a file) and stderr left on the operator's
/// terminal (the shell never redirected it).
fn verifier(bin: &Path, args: &[&OsStr]) -> Result<(bool, Vec<u8>)> {
    let run = Command::new(bin)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("run {}", bin.display()))?;
    Ok((run.status.success(), run.stdout))
}

/// `session_request <method> <path> <expected>` (L758-767) for the GET
/// sites this span uses: the website origin, the session cookie, one
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

/// `curl -o <file>` / `-o /dev/null`: the response body went somewhere
/// other than `$body`, so the shared buffer must keep what the previous
/// request left in it (plan §7.6).  `dump_headers` is true where the
/// call also passed `-D "$headers"`.
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

/// `await_row_downloads` (L1224-1239).
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

/// The `downloads` column of the one fixture row, as the shell's `node`
/// printed it (L1229-1235).  The package identity is spelled out rather
/// than derived: so was the SQL.
fn row_downloads_now() -> Result<String> {
    let json = d1_json(
        "SELECT downloads FROM versions
       WHERE scope = 'smoke' AND name = 'withdep' AND version = '0.2.0'",
    )?;
    column(&json, "downloads")
}

/// `await_backup_blob` (L1270-1279): replication runs via `waitUntil`
/// after the response, so poll the BACKUP bucket briefly.
fn await_backup_blob(key: &str, out: &Path) -> Result<()> {
    for _ in 0..POLLS {
        if r2_get(BACKUP_BUCKET, key, out, true)? {
            return Ok(());
        }
        sleep(HALF_SECOND);
    }
    bail!("blob {key} never appeared in the BACKUP bucket")
}

/// The `backup_pending` rows still queued for this blob (L1288-1294).
fn backup_pending_rows(inputs: &PublishInputs<'_>) -> Result<String> {
    let json = d1_json(&format!(
        "SELECT COUNT(*) AS n FROM backup_pending
     WHERE key = 'blobs/sha256/{}'",
        inputs.blob_hash
    ))?;
    column(&json, "n")
}

/// `out[0].results[0].<name>` as `console.log` printed it.
fn column(json: &str, name: &str) -> Result<String> {
    let rows = results(json)?;
    let row = rows.first().context("the query returned no row")?;
    let value = row
        .get(name)
        .with_context(|| format!("the query returned no {name} column"))?;
    Ok(display(value))
}

/// The publish-bucket refund (L1009-1010, L1039-1040), kept as the one
/// statement the shell sent, whitespace included.
fn refund() -> Result<()> {
    crate::servers::d1_quiet(
        "
  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';",
    )
}

/// `wrangler r2 object get <bucket>/<key> --file <out> --local`, and
/// whether it succeeded: three of the four sites read a *failure* as
/// the assertion (the object must be absent) rather than as an error.
/// `quiet` is the `2>&1` those three added.
fn r2_get(bucket: &str, key: &str, out: &Path, quiet: bool) -> Result<bool> {
    let target = format!("{bucket}/{key}");
    let mut command = wrangler(&[
        "r2",
        "object",
        "get",
        &target,
        "--file",
        utf8(out)?,
        "--local",
    ]);
    command.stdout(Stdio::null());
    if quiet {
        command.stderr(Stdio::null());
    }
    Ok(command.status().context("run wrangler")?.success())
}

/// `wrangler r2 object delete <bucket>/<key> --local`.
fn r2_delete(bucket: &str, key: &str) -> Result<()> {
    let target = format!("{bucket}/{key}");
    output(&mut wrangler(&[
        "r2", "object", "delete", &target, "--local",
    ]))?;
    Ok(())
}

/// `wrangler r2 object put <bucket>/<key> --file <file> --local`.
fn r2_put(bucket: &str, key: &str, file: &Path) -> Result<()> {
    let target = format!("{bucket}/{key}");
    output(&mut wrangler(&[
        "r2",
        "object",
        "put",
        &target,
        "--file",
        utf8(file)?,
        "--local",
    ]))?;
    Ok(())
}

// --- Pure shapes. ---

/// L1095-1097: `printf '…%4097s}'` with an empty argument - 4097 spaces
/// of padding *inside* an otherwise well-formed rejected verdict, so an
/// uncapped handler would parse and apply it and only the body cap can
/// be what refused.
fn oversized_verdict(checksum: &str, published_at: &str) -> Vec<u8> {
    format!(
        r#"{{"verdict":"rejected","reason":"oversized","checksum":"{checksum}","published_at":"{published_at}"{:4097}}}"#,
        ""
    )
    .into_bytes()
}

/// The bound verdict bodies (L1147-1152, L1205-1211) as
/// `JSON.stringify` wrote them: the keys in that insertion order, and a
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

/// L1244-1248: `packages`, `versions` and `downloads` must each be an
/// integer of at least 1.  `Number.isInteger`, so a fractional value or
/// a non-number fails and a whole `1.0` passes.
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

/// `grep -qi '^<prefix>'` over the header block: line-wise, anchored,
/// case-insensitive.  The block is CRLF, and `lines` drops the `\r`
/// that only ever trails a value.
fn header_starts_with(headers: &[u8], prefix: &str) -> bool {
    let block = String::from_utf8_lossy(headers).to_ascii_lowercase();
    block.lines().any(|line| line.starts_with(prefix))
}

/// `grep -i <fixed> "$headers"` as the two no-store failures
/// interpolated it: every matching line, `\r` and all, newline joined.
fn grep_lines(headers: &[u8], needle: &str) -> String {
    String::from_utf8_lossy(headers)
        .split('\n')
        .filter(|line| line.to_ascii_lowercase().contains(needle))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `$(cat "$body")` in a `fail` message.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn blob_key(inputs: &PublishInputs<'_>) -> String {
    format!("blobs/sha256/{}", inputs.blob_hash)
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

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn entry() -> Map<String, Value> {
        let mut entry = Map::new();
        entry.insert("checksum".to_owned(), Value::String("abc123".to_owned()));
        entry.insert(
            "published_at".to_owned(),
            Value::String("1970-01-01T00:00:00Z".to_owned()),
        );
        entry
    }

    #[test]
    fn the_verified_verdict_is_the_json_the_node_program_wrote() {
        let body = verdict_body(&entry(), "verified", None).expect("verdict");
        assert_eq!(
            String::from_utf8(body).expect("utf8"),
            r#"{"verdict":"verified","checksum":"abc123","published_at":"1970-01-01T00:00:00Z"}"#
        );
    }

    #[test]
    fn the_rejected_verdict_carries_its_reason_second() {
        let body = verdict_body(&entry(), "rejected", Some("smoke rejection")).expect("verdict");
        assert_eq!(
            String::from_utf8(body).expect("utf8"),
            concat!(
                r#"{"verdict":"rejected","reason":"smoke rejection","#,
                r#""checksum":"abc123","published_at":"1970-01-01T00:00:00Z"}"#
            )
        );
    }

    /// `JSON.stringify` omits an undefined field; a `null` binding is a
    /// different document and stays one.
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
        assert_eq!(body.len(), head.len() + 4097 + 1);
        assert!(body.starts_with(head.as_bytes()), "{}", text(&body));
        assert_eq!(&body[head.len()..head.len() + 4097], vec![b' '; 4097]);
        assert_eq!(body.last(), Some(&b'}'));
        // Padded past the cap, but still a document a handler that read
        // the whole stream would accept.
        serde_json::from_slice::<Value>(&body).expect("a valid rejected verdict");
    }

    #[test]
    fn the_listing_entry_keeps_the_six_keys_in_insertion_order() {
        let listing = br#"{"versions":[
            {"version":"0.1.0","name":"smoke/withdep"},
            {"metadata":{"z":1,"a":2},"published_at":"then","checksum":"c",
             "revision":"r","version":"0.2.0","name":"smoke/withdep","published_by":1}
        ]}"#;
        let out = assert_fs::NamedTempFile::new("entry.json").expect("temp");
        let entry =
            listing_entry(listing, "smoke/withdep", "0.2.0", out.path()).expect("the entry");

        assert_eq!(
            String::from_utf8(fs::read(out.path()).expect("read")).expect("utf8"),
            concat!(
                r#"{"name":"smoke/withdep","version":"0.2.0","revision":"r","#,
                r#""checksum":"c","published_at":"then","metadata":{"z":1,"a":2}}"#
            ),
            "published_by is not part of the shape, and metadata keeps its own order"
        );
        assert_eq!(entry.get("checksum").map(display), Some("c".to_owned()));
    }

    #[test]
    fn a_listing_without_the_version_is_the_shells_failure() {
        let out = assert_fs::NamedTempFile::new("entry.json").expect("temp");
        let error = listing_entry(br#"{"versions":[]}"#, "smoke/withdep", "0.2.0", out.path())
            .expect_err("no such version");
        assert_eq!(
            error.to_string(),
            "the pending listing has no smoke/withdep@0.2.0"
        );
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

    #[test]
    fn the_header_assertion_is_anchored_and_case_insensitive() {
        let block =
            b"HTTP/1.1 200 OK\r\nCache-Control: no-store\r\nX-Cache-Control: public\r\n\r\n";
        assert!(header_starts_with(block, "cache-control: no-store"));
        assert!(!header_starts_with(
            b"HTTP/1.1 200 OK\r\nX-Cache-Control: no-store\r\n\r\n",
            "cache-control: no-store"
        ));
        assert_eq!(
            grep_lines(block, "cache-control"),
            "Cache-Control: no-store\r\nX-Cache-Control: public\r"
        );
    }
}
