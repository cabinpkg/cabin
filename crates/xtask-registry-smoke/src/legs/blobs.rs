//! Per-upload storage accounting and the write plane's uniform 403: a
//! second version publishing the exact bytes of the first (its own
//! upload, counted again), the verdict binding's stale-listing 409, a
//! rejection that refunds the bytes while going invisible to everyone,
//! the republish that supersedes it and its refund in turn, the upload
//! id that already names another revision, the byte-identical revival
//! under a new one, the cron's release of an expired reservation and
//! the resume under its own id, the two ends of an archive placement
//! that did not finish in its verdict's request, the documents a yank
//! and the cron leave in INDEX, and the byte-identical refusal an
//! unclaimed and a foreign scope must both answer.
//!
//! Three comparisons here check a response against one an *earlier*
//! request left in the shared buffer: each reads a clone taken before
//! the next request overwrites it, over raw bytes.  Parsing either side
//! would weaken "byte-identical refusal" to "same fields", which is the
//! property these legs exist to prove.
//!
//! The leg inherits the publisher credential the previous leg left set
//! and hands it back the same way, so nothing here selects a credential
//! on entry.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::bytes::{frame, retarget_hash, revision_of, sha256_hex, tamper_zip};
use crate::context::Smoke;
use crate::legs::finale::BREAKER_CRON;
use crate::legs::revisions::{UNYANKED, YANKED};
use crate::legs::session;
use crate::servers::{d1, d1_quiet, d1_rows, delete_upload, stored_document, stored_upload};
use crate::step;
use crate::text::{capture, contains};

/// What the publish/verify phases before this one computed and this
/// leg reads: the fixture identity, the derived request paths for
/// `0.2.1`, the bodies already framed, and the minted session cookie.
pub struct BlobInputs<'a> {
    pub scope: &'a str,
    pub name: &'a str,
    /// The version the uniform-403 leg names on the two scopes it may
    /// not write to.
    pub version: &'a str,
    /// The second version, which republishes the first one's bytes.
    pub version2: &'a str,
    /// The fixture archive's packaging revision.
    pub rev: &'a str,
    /// The fixture archive's SHA-256, which the replacement metadata is
    /// retargeted off.
    pub blob_hash: &'a str,
    /// The verified `0.2.0` artifact.
    pub artifact_path: &'a str,
    /// The package document.
    pub package_path: &'a str,
    pub publish2_path: &'a str,
    /// `0.2.1`'s first revision - the same id as `0.2.0`'s, because the
    /// bytes are the same.
    pub artifact2_path: &'a str,
    pub verdict2_path: &'a str,
    /// The `0.2.1` metadata the replacement is retargeted from.
    pub metadata2: &'a [u8],
    pub publish2: &'a [u8],
    /// The bytes the tampered replacement is derived from.
    pub archive: &'a [u8],
    /// The well-formed body the 403 leg sends to scopes it may not
    /// write to.
    pub publish: &'a [u8],
    pub yank: &'a [u8],
    pub session_cookie: &'a str,
    /// The run's scratch directory.
    pub work: &'a Path,
}

/// The `published_at` a verdict must *not* carry: the epoch names no
/// publish event under this triple, which is what makes the binding
/// stale rather than absent.
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

/// The publisher's `users.stored_bytes`, the exact storage accounting
/// every admission, rejection and revival moves.
const STORED_BYTES: &str =
    "SELECT stored_bytes FROM users WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke')";

/// The publish bucket's burst, cleared so the attempts below are not
/// refused by the rate limit before the membership gate is reached.
const RESET_PUBLISH_BUCKET: &str = "
  UPDATE users SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke');";

/// The whole leg.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<()> {
    let (before_bytes, first_upload) = same_bytes_again(smoke, inputs)?;
    let entry = pending_entry(smoke, inputs)?;
    stale_verdict(smoke, inputs, &entry)?;
    reject_refunds(smoke, inputs, &entry, &before_bytes)?;
    let replacement = republish_over_rejected(smoke, inputs, &before_bytes)?;
    reject_replacement(smoke, inputs, &before_bytes)?;
    let revival = restart_verification(smoke, inputs, &before_bytes, &replacement, &first_upload)?;
    resume_released(smoke, inputs, &before_bytes, &replacement, &revival)?;
    cron_records_placed_archives(smoke)?;
    index_holds_served_documents(smoke, inputs)?;
    uniform_403(smoke, inputs)
}

/// The second version publishes the exact archive the first did.  An
/// archive is stored per upload, not per content, so it is charged
/// again.  Returns the bytes charged before it and its upload id.
fn same_bytes_again(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<(String, String)> {
    step("a second version with identical content is its own counted upload");
    let before_bytes = stored_bytes()?;
    let upload_id = smoke.publish(inputs.publish2_path, inputs.publish2, &[201])?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_bytes()? != sum(&before_bytes, inputs.archive.len())? {
        bail!(
            "the second upload of the same bytes was not counted: {} (was {before_bytes})",
            stored_bytes()?
        );
    }
    Ok((before_bytes, upload_id))
}

/// Every verdict binds to the listing entry of the version it judges:
/// the checksum names the revision, and `published_at` names the
/// publish event.  `0.2.1` shares `0.2.0`'s bytes - so the same
/// checksum - but is its own publish event, so it needs its own entry.
fn pending_entry(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<Entry> {
    smoke.as_verifier();
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    binding(
        &smoke.body,
        &scoped(inputs),
        inputs.version2,
        &inputs.work.join("entry2.json"),
    )
}

/// The checksum must be the real one - a bogus digest names no revision
/// under this triple and is a plain 404, never the binding conflict.
/// The stale half is the publish event, exactly what a byte-identical
/// revival changes.
fn stale_verdict(smoke: &mut Smoke, inputs: &BlobInputs<'_>, entry: &Entry) -> Result<()> {
    step("a verdict bound to a stale listing conflicts");
    let verdict = verdict_stale(&entry.checksum);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[409])?;
    smoke.expect_body("changed since it was listed")
}

/// Bound to `0.2.1`'s own listing entry: the shared checksum names the
/// revision, its publish event names the generation.
fn reject_refunds(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    entry: &Entry,
    before_bytes: &str,
) -> Result<()> {
    step("rejecting a version refunds its bytes and leaves the other version's alone");
    let verdict = verdict_rejected(&entry.checksum, &entry.published_at);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[200])?;
    smoke.expect_body(r#""verification":"rejected""#)?;
    smoke.expect_body(r#""changed":true"#)?;
    smoke.check(inputs.artifact2_path, &[404])?;

    // Rejected content is invisible to no-verify readers too, and its
    // refusal is byte-identical to an unknown one: a rejected revision
    // must never be distinguishable from a version that never existed.
    smoke.as_ci_publisher();
    smoke.check(inputs.artifact2_path, &[404])?;
    let rejected_404 = smoke.body.clone();
    let unknown = artifact_path(inputs, "9.9.9", inputs.rev);
    smoke.check(&unknown, &[404])?;
    if smoke.body != rejected_404 {
        bail!(
            "a no-verify rejected 404 differs from an unknown one: {}",
            capture(&smoke.body)
        );
    }

    // Rejected versions are served to no one - not even the verify
    // scope, which can still read pending ones - and are invisible to
    // the source viewer.
    smoke.as_verifier();
    smoke.check(inputs.artifact2_path, &[404])?;
    let source = format!(
        "/api/v1/user/source/{}/{}/{}",
        inputs.scope, inputs.name, inputs.version2
    );
    session::session_request(
        smoke,
        inputs.session_cookie,
        "GET",
        &source,
        404,
        &[("Range".to_owned(), "bytes=-22".to_owned())],
        None,
    )?;

    smoke.as_publisher();
    if stored_bytes()? != before_bytes {
        bail!(
            "the rejection did not refund the upload's bytes: {} (was {before_bytes})",
            stored_bytes()?
        );
    }
    smoke.check(inputs.artifact_path, &[200])?;
    smoke.check(inputs.package_path, &[200])?;
    if contains(&smoke.body, br#""0.2.1""#) {
        bail!(
            "a rejected version leaked into the package document: {}",
            capture(&smoke.body)
        );
    }
    smoke.wrequest(
        "PATCH",
        &format!("{}/yank", inputs.publish2_path),
        inputs.yank,
        &[404],
    )
}

/// Different bytes are a different revision, but the version's only
/// other revision is the rejected one - nothing live to supersede - so
/// this needs no new-revision opt-in.
fn republish_over_rejected(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
) -> Result<Replacement> {
    step("republishing over a rejected version replaces it as pending");
    let archive = tamper_zip(inputs.archive, 2);
    let hash = sha256_hex(&archive);
    let revision = revision_of(&archive);
    let artifact = artifact_path(inputs, inputs.version2, &revision);
    let metadata = retarget_hash(inputs.metadata2, inputs.blob_hash, &hash);
    let body = frame(&metadata, &archive);

    smoke.publish(inputs.publish2_path, &body, &[201])?;
    smoke.expect_body(&format!(r#""revision":"{revision}""#))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_bytes()? != sum(before_bytes, archive.len())? {
        bail!(
            "the replacement archive was not counted: {}",
            stored_bytes()?
        );
    }

    smoke.as_verifier();
    smoke.wcheck("/api/v1/admin/versions?status=pending", &[200])?;
    smoke.expect_body(r#""version":"0.2.1""#)?;
    smoke.expect_body(&hash)?;
    Ok(Replacement {
        revision,
        artifact,
        body,
        archive,
    })
}

/// The refund is the whole effect: the archive itself is left to the
/// UPLOADS bucket's expiry.
fn reject_replacement(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
) -> Result<()> {
    step("rejecting the replacement refunds its bytes");
    let entry = binding(
        &smoke.body,
        &scoped(inputs),
        inputs.version2,
        &inputs.work.join("entry-replacement.json"),
    )?;
    let verdict = verdict_rejected(&entry.checksum, &entry.published_at);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[200])?;
    smoke.as_publisher();
    if stored_bytes()? != before_bytes {
        bail!(
            "the rejection did not refund the replacement bytes: {}",
            stored_bytes()?
        );
    }
    Ok(())
}

/// Byte-identical, so this revives the rejected revision in place
/// rather than minting another one - as a new upload operation, whose
/// id must not be one that already names another revision.
/// `first_upload` is the id the version's first revision was published
/// under; the revival's own is returned.
fn restart_verification(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
    replacement: &Replacement,
    first_upload: &str,
) -> Result<String> {
    // The request the next step admits, apart from its id.
    step("an upload id that already names another revision is refused");
    smoke.publish_as(
        first_upload,
        inputs.publish2_path,
        &replacement.body,
        &[409],
    )?;
    smoke.expect_body("already names a different upload")?;

    step("republishing identical bytes over a rejected version restarts verification");
    let upload_id = smoke.publish(inputs.publish2_path, &replacement.body, &[201])?;
    smoke.expect_body(&format!(r#""revision":"{}""#, replacement.revision))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_bytes()? != sum(before_bytes, replacement.archive.len())? {
        bail!("the revived upload was not re-counted: {}", stored_bytes()?);
    }
    if stored_upload(&upload_id, inputs.work)?.as_deref() != Some(replacement.archive.as_slice()) {
        bail!("the revived archive is not in UPLOADS under the new upload id");
    }
    smoke.as_verifier();
    smoke.check(&replacement.artifact, &[200])?;
    smoke.as_publisher();
    Ok(upload_id)
}

/// A reservation whose store never happened expires after fifteen
/// minutes: the cron then releases it and refunds its bytes, and the
/// same upload id may take it up again.  The run cannot wait that out,
/// so the expiry is written into the revival's row directly - no
/// `stored_at`, a `published_at` past the limit, its object deleted.
/// The release and the resume are the Worker's own.
///
/// The resumed revision is then verified with its upload deleted
/// again and its upload id cleared, as a row is thirty days after its
/// verdict: with no archive to place it is rejected and refunded
/// rather than left verified and unserved.
fn resume_released(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
    replacement: &Replacement,
    upload_id: &str,
) -> Result<()> {
    step("the cron releases an expired reservation and its upload id resumes it");
    reset_publish_bucket()?;
    d1_quiet(&format!(
        "UPDATE revisions SET stored_at = NULL,
           published_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-16 minutes'),
           charged_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-16 minutes')
         WHERE upload_id = '{upload_id}';"
    ))?;
    delete_upload(upload_id, inputs.work)?;
    smoke.check(BREAKER_CRON, &[200])?;
    if stored_bytes()? != before_bytes {
        bail!(
            "the cron did not release and refund the expired reservation: {}",
            stored_bytes()?
        );
    }

    smoke.publish_as(upload_id, inputs.publish2_path, &replacement.body, &[201])?;
    if stored_bytes()? != sum(before_bytes, replacement.archive.len())? {
        bail!("the resumed upload was not re-counted: {}", stored_bytes()?);
    }
    if stored_upload(upload_id, inputs.work)?.as_deref() != Some(replacement.archive.as_slice()) {
        bail!("the resumed archive is not in UPLOADS under its upload id");
    }

    step("a verified revision whose upload is gone is rejected, never served");
    let entry = pending_entry(smoke, inputs)?;
    delete_upload(upload_id, inputs.work)?;
    d1_quiet(&format!(
        "UPDATE revisions SET upload_id = NULL WHERE upload_id = '{upload_id}';"
    ))?;
    let verdict = verdict_verified(&entry.checksum, &entry.published_at);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[200])?;
    let rows = d1_rows(&format!(
        "SELECT verification, verification_reason FROM revisions
         WHERE checksum = '{}' AND published_at = '{}'",
        entry.checksum, entry.published_at
    ))?;
    let field = |key| {
        rows.first()
            .and_then(|row| row.get(key))
            .and_then(Value::as_str)
    };
    if (field("verification"), field("verification_reason"))
        != (Some("rejected"), Some("publication_source_lost"))
    {
        bail!("the revision with no archive to place was not rejected: {rows:?}");
    }
    // The rejection refunds like any other, and frees the pending slot
    // the legs after this one need: the default class holds three
    // pending uploads at a time (`registry/src/quota.rs`).
    if stored_bytes()? != before_bytes {
        bail!("the lost upload was not refunded: {}", stored_bytes()?);
    }
    smoke.check(&replacement.artifact, &[404])?;
    smoke.as_publisher();
    smoke.check(&replacement.artifact, &[404])
}

/// A placement pass that died after its copy leaves the object in
/// ARCHIVES and the revision unserved.  That state is written into
/// every served row directly; the cron's pass must find each object
/// and record it.
fn cron_records_placed_archives(smoke: &mut Smoke) -> Result<()> {
    step("the cron serves a revision whose archive a dead pass had placed");
    let unserved = || {
        d1_rows("SELECT 1 FROM revisions WHERE verification = 'verified' AND archived_at IS NULL")
    };
    d1_quiet("UPDATE revisions SET archived_at = NULL;")?;
    if unserved()?.is_empty() {
        bail!("no served revision was left to un-record");
    }
    smoke.check(BREAKER_CRON, &[200])?;
    if !unserved()?.is_empty() {
        bail!("the cron left a placed archive unrecorded");
    }
    Ok(())
}

/// INDEX is where the documents will be read from, so it must hold
/// what the read plane composes.  A yank records a job and puts
/// nothing itself - it has no quota, and a put would draw on the
/// infrastructure pools; its package's document and the catalog follow
/// in the next cron pass.  The cron completes a change only once it has
/// settled, which the last step stands in for by ageing the open jobs.
fn index_holds_served_documents(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<()> {
    step("INDEX holds the documents the read plane serves");
    let open_jobs = || d1_rows("SELECT 1 FROM publication_jobs WHERE completed_at IS NULL");
    let document = inputs.package_path.trim_start_matches('/');
    let yank_path = format!(
        "/api/v1/packages/{}/{}/{}/yank",
        inputs.scope, inputs.name, inputs.version
    );
    for flag in [YANKED, UNYANKED] {
        let recorded = open_jobs()?.len();
        smoke.wrequest("PATCH", &yank_path, flag, &[200])?;
        smoke.expect_body(r#""changed":true"#)?;
        if open_jobs()?.len() != recorded + 1 {
            bail!("a yank did not record one publication job");
        }
        smoke.check(inputs.package_path, &[200])?;
        let served = smoke.body.clone();
        if stored_document(document, inputs.work)?.as_ref() == Some(&served) {
            bail!("a yank's request put its document into INDEX");
        }
        smoke.check(BREAKER_CRON, &[200])?;
        if open_jobs()?.is_empty() {
            bail!("the cron completed a publication job that had not settled");
        }
        if stored_document(document, inputs.work)?.as_ref() != Some(&served) {
            bail!("INDEX does not hold the package document a yank changed");
        }
        smoke.check("/catalog.json", &[200])?;
        if stored_document("catalog.json", inputs.work)?.as_deref() != Some(&smoke.body[..]) {
            bail!("INDEX does not hold the catalog the read plane serves");
        }
    }
    d1_quiet(
        "UPDATE publication_jobs SET created_at = '2000-01-01T00:00:00.000Z' \
         WHERE completed_at IS NULL;",
    )?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !open_jobs()?.is_empty() {
        bail!("the cron left a settled publication job open");
    }
    Ok(())
}

/// `ghost` was never claimed; `foreign` belongs only to the seeded
/// user 2.  Both must answer the byte-identical refusal, so an
/// authenticated publisher cannot probe which scopes exist.  The gate
/// fires before the body is read, so the well-formed publish body is
/// irrelevant.
fn uniform_403(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<()> {
    // These attempts charge the publish bucket like any others (the
    // membership gate sits after the rate limit), so the leg gets its
    // own.
    reset_publish_bucket()?;

    step("publishing to an unclaimed or foreign scope is one uniform 403");
    let ghost = format!("/api/v1/packages/ghost/{}/{}", inputs.name, inputs.version);
    smoke.publish(&ghost, inputs.publish, &[403])?;
    let refusal = smoke.body.clone();
    if !contains(&refusal, b"not a member") {
        bail!(
            "the refusal is not the membership detail: {}",
            capture(&refusal)
        );
    }

    let foreign = format!(
        "/api/v1/packages/foreign/{}/{}",
        inputs.name, inputs.version
    );
    smoke.publish(&foreign, inputs.publish, &[403])?;
    if smoke.body != refusal {
        bail!(
            "foreign-scope and unclaimed-scope refusals differ: {}",
            capture(&smoke.body)
        );
    }

    smoke.wrequest("PATCH", &format!("{foreign}/yank"), inputs.yank, &[403])?;
    if smoke.body != refusal {
        bail!(
            "the yank refusal differs from the publish refusal: {}",
            capture(&smoke.body)
        );
    }
    Ok(())
}

/// The tampered archive the version is republished with, carried
/// between the legs that publish it and republish it unchanged.
struct Replacement {
    /// The revision id, which the restart leg names again.
    revision: String,
    artifact: String,
    /// The framed body, re-sent verbatim by the restart leg.
    body: Vec<u8>,
    /// The archive alone: its length, not the framed body's, is what is
    /// accounted for, and its bytes are what UPLOADS must hold.
    archive: Vec<u8>,
}

/// The listing fields a verdict binds to: the checksum names the
/// revision, `published_at` the publish event.
#[derive(Debug)]
struct Entry {
    checksum: String,
    published_at: String,
}

/// The two halves of the binding, read back out of the entry file the
/// shared `listing_entry` just wrote.  A missing half fails here rather
/// than as a verdict with no binding, whose 400 would surface as a
/// status mismatch several lines later.
fn binding(listing: &[u8], name: &str, version: &str, out: &Path) -> Result<Entry> {
    session::listing_entry(listing, name, version, out)?;
    let written = fs::read(out).with_context(|| format!("read {}", out.display()))?;
    let entry: Value =
        serde_json::from_slice(&written).with_context(|| format!("parse {}", out.display()))?;
    let half = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .with_context(|| format!("the listing entry for {name}@{version} has no {key}"))
    };
    Ok(Entry {
        checksum: half("checksum")?,
        published_at: half("published_at")?,
    })
}

/// The real checksum, so the revision resolves, with a publish event
/// that never happened.
fn verdict_stale(checksum: &str) -> String {
    format!(
        r#"{{"verdict":"verified","checksum":{},"published_at":"{EPOCH}"}}"#,
        json_string(checksum)
    )
}

/// A verified verdict bound to its entry.
fn verdict_verified(checksum: &str, published_at: &str) -> String {
    format!(
        r#"{{"verdict":"verified","checksum":{},"published_at":{}}}"#,
        json_string(checksum),
        json_string(published_at)
    )
}

/// The two rejection verdicts, which differ only in the entry they
/// bind to.
fn verdict_rejected(checksum: &str, published_at: &str) -> String {
    format!(
        r#"{{"verdict":"rejected","reason":"smoke rejection","checksum":{},"published_at":{}}}"#,
        json_string(checksum),
        json_string(published_at)
    )
}

/// One string as JSON: its quotes and any escaping.
fn json_string(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

/// The publisher's `stored_bytes` as display text; the comparisons are
/// string comparisons.
fn stored_bytes() -> Result<String> {
    let value = d1_rows(STORED_BYTES)?
        .first()
        .and_then(|row| row.get("stored_bytes").cloned())
        .context("the smoke token's user has no row")?;
    Ok(xtask_registry_admin::display(&value))
}

/// `before_bytes + size`, as text.
fn sum(before_bytes: &str, size: usize) -> Result<String> {
    let before: usize = before_bytes
        .parse()
        .with_context(|| format!("stored_bytes is not a number: {before_bytes}"))?;
    Ok((before + size).to_string())
}

fn reset_publish_bucket() -> Result<()> {
    d1(RESET_PUBLISH_BUCKET).context("resetting the publish bucket")
}

/// `<scope>/<name>`, the scoped name the admin listing keys entries by.
fn scoped(inputs: &BlobInputs<'_>) -> String {
    format!("{}/{}", inputs.scope, inputs.name)
}

/// The artifact route for one version and revision of this package.
fn artifact_path(inputs: &BlobInputs<'_>, version: &str, revision: &str) -> String {
    let BlobInputs { scope, name, .. } = *inputs;
    format!("/archives/{scope}/{name}/{scope}-{name}-{version}-{revision}.zip")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKSUM: &str = "3f786850e387550fdab836ed7e6dc881de23001b3f786850e387550fdab836ed";

    #[test]
    fn the_artifact_route_spells_out_scope_name_version_and_revision() {
        let inputs = BlobInputs {
            scope: "smoke",
            name: "withdep",
            version: "0.2.0",
            version2: "0.2.1",
            rev: "3f786850e387550f",
            blob_hash: CHECKSUM,
            artifact_path: "",
            package_path: "",
            publish2_path: "",
            artifact2_path: "",
            verdict2_path: "",
            metadata2: b"",
            publish2: b"",
            archive: b"",
            publish: b"",
            yank: b"",
            session_cookie: "",
            work: Path::new("/tmp"),
        };
        assert_eq!(scoped(&inputs), "smoke/withdep");
        assert_eq!(
            artifact_path(&inputs, "9.9.9", inputs.rev),
            "/archives/smoke/withdep/smoke-withdep-9.9.9-3f786850e387550f.zip"
        );
    }
}
