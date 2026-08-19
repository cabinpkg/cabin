//! Shared blobs, rejection accounting and the write plane's uniform
//! 403, `registry/scripts/smoke.sh` L1600-1741: a second version
//! publishing the exact bytes of the first (one blob, counted once),
//! the verdict binding's stale-listing 409, a rejection that keeps both
//! the shared blob and the accounting while going invisible to
//! everyone, the republish that supersedes it, the reclaim and refund
//! once the rejected blob is unshared, the byte-identical restart, and
//! the byte-identical refusal an unclaimed and a foreign scope must
//! both answer.
//!
//! Three legs here compare a response against one an *earlier* request
//! left in the shared buffer (the shell's `cp "$body" …` then `cmp -s`,
//! plan §7.6): each is a clone taken at exactly the line the shell
//! copied, compared over raw bytes.  Parsing either side would weaken
//! "byte-identical refusal" to "same fields", which is the property
//! these legs exist to prove.
//!
//! The leg inherits the publisher credential the previous phase left
//! set (L1562) and hands it back the same way, so nothing here selects
//! a credential on entry.

use std::fs;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use xtask_registry_admin::BLOBS_BUCKET;

use crate::bytes::{frame, retarget_hash, revision_of, sha256_hex, tamper_zip};
use crate::context::Smoke;
use crate::legs::session;
use crate::servers::{d1, d1_rows};
use crate::step;
use crate::text::{capture, contains};

/// What the publish/verify phases before this one computed and this
/// leg reads: the fixture identity, the two derived request paths for
/// `0.2.1`, the bodies already framed, and the minted session cookie.
pub struct BlobInputs<'a> {
    /// `$scope`.
    pub scope: &'a str,
    /// `$name`.
    pub name: &'a str,
    /// `$version` - the version the uniform-403 leg names on the two
    /// scopes it may not write to.
    pub version: &'a str,
    /// `$version2`, the shared-blob version.
    pub version2: &'a str,
    /// `$rev`, the fixture archive's packaging revision.
    pub rev: &'a str,
    /// `$blob_hash`, the fixture archive's SHA-256, which the
    /// replacement metadata is retargeted off.
    pub blob_hash: &'a str,
    /// `$artifact_path`, the verified `0.2.0` artifact.
    pub artifact_path: &'a str,
    /// `$package_path`, the package document.
    pub package_path: &'a str,
    /// `$publish2_path`.
    pub publish2_path: &'a str,
    /// `$artifact2_path`, `0.2.1`'s first revision - the same id as
    /// `0.2.0`'s, because the bytes are the same.
    pub artifact2_path: &'a str,
    /// `$verdict2_path`.
    pub verdict2_path: &'a str,
    /// `$work/withdep-0.2.1.json`, the metadata the replacement is
    /// retargeted from.
    pub metadata2: &'a [u8],
    /// `$work/publish2.bin`.
    pub publish2: &'a [u8],
    /// `$fixture_archive`, the bytes the tampered replacement is
    /// derived from.
    pub archive: &'a [u8],
    /// `$work/publish.bin`, the well-formed body the 403 leg sends to
    /// scopes it may not write to.
    pub publish: &'a [u8],
    /// `$work/yank.json`.
    pub yank: &'a [u8],
    /// `$session_cookie`.
    pub session_cookie: &'a str,
    /// `$work`, the run's scratch directory: `wrangler r2 object get`
    /// needs a real `--file` path.
    pub work: &'a Path,
}

/// The `published_at` a verdict must *not* carry: the epoch names no
/// publish event under this triple, which is what makes the binding
/// stale rather than absent.
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

/// `meta.total_stored_bytes`, the exact storage self-accounting.
const TOTAL_STORED_BYTES: &str = "SELECT value FROM meta WHERE key = 'total_stored_bytes'";

/// The publish bucket's burst, cleared so the attempts below are not
/// refused by the rate limit before the membership gate is reached.
const RESET_PUBLISH_BUCKET: &str =
    "\n  UPDATE tokens SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 'smoke';";

/// The whole leg, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<()> {
    let before_bytes = shared_blob(smoke, inputs)?;
    let entry = pending_entry(smoke, inputs)?;
    stale_verdict(smoke, inputs, &entry)?;
    reject_shared(smoke, inputs, &entry, &before_bytes)?;
    let replacement = republish_over_rejected(smoke, inputs, &before_bytes)?;
    reclaim_unshared(smoke, inputs, &before_bytes, &replacement)?;
    restart_verification(smoke, inputs, &before_bytes, &replacement)?;
    uniform_403(smoke, inputs)
}

/// L1600-1605.  The second version publishes the exact archive the
/// first did, so the blob is shared and the accounting must not move.
fn shared_blob(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<String> {
    step("publishing a second version with identical content shares the blob");
    let before_bytes = stored_bytes()?;
    smoke.wrequest("PUT", inputs.publish2_path, inputs.publish2, &[201])?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_bytes()? != before_bytes {
        bail!(
            "a shared blob was double-counted: {} (was {before_bytes})",
            stored_bytes()?
        );
    }
    Ok(before_bytes)
}

/// L1607-1614.  Every verdict binds to the listing entry of the version
/// it judges: the checksum names the revision, and `published_at` names
/// the publish event.  `0.2.1` shares `0.2.0`'s bytes - so the same
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

/// L1616-1629.  The checksum must be the real one - a bogus digest
/// names no revision under this triple and is a plain 404, never the
/// binding conflict.  The stale half is the publish event, exactly what
/// a byte-identical revival changes.
fn stale_verdict(smoke: &mut Smoke, inputs: &BlobInputs<'_>, entry: &Entry) -> Result<()> {
    step("a verdict bound to a stale listing conflicts");
    let verdict = verdict_stale(&entry.checksum);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[409])?;
    smoke.expect_body("changed since it was listed")
}

/// L1631-1667.  Bound to `0.2.1`'s own listing entry: the shared
/// checksum names the revision, its publish event names the generation.
fn reject_shared(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    entry: &Entry,
    before_bytes: &str,
) -> Result<()> {
    step("rejecting a version sharing its blob keeps the blob and the accounting");
    let verdict = verdict_rejected(&entry.checksum, &entry.published_at);
    smoke.verdict_patch(inputs.verdict2_path, verdict.as_bytes(), &[200])?;
    smoke.expect_body(r#""verification":"rejected""#)?;
    smoke.expect_body(r#""changed":true"#)?;
    smoke.check(inputs.artifact2_path, &[404])?;

    // Rejected content is invisible to anonymous readers too, and its
    // refusal is byte-identical to an unknown one: a rejected revision
    // must never be distinguishable from a version that never existed.
    smoke.anonymous();
    smoke.check(inputs.artifact2_path, &[404])?;
    let rejected_404 = smoke.body.clone();
    let unknown = artifact_path(inputs, "9.9.9", inputs.rev);
    smoke.check(&unknown, &[404])?;
    if smoke.body != rejected_404 {
        bail!(
            "an anonymous rejected 404 differs from an unknown one: {}",
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
            "rejecting a shared blob changed the accounting: {} (was {before_bytes})",
            stored_bytes()?
        );
    }
    smoke.check(inputs.artifact_path, &[200])?;
    smoke.check(inputs.package_path, &[200])?;
    // The shell spelled this needle out rather than interpolating
    // `$version2`, and so does the listing assertion at L1688.
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

/// L1669-1689.  Different bytes are a different revision, but the
/// version's only other revision is the rejected one - nothing live to
/// supersede - so this needs no new-revision opt-in.
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

    smoke.wrequest("PUT", inputs.publish2_path, &body, &[201])?;
    smoke.expect_body(&format!(r#""revision":"{revision}""#))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    let size = archive.len();
    if stored_bytes()? != sum(before_bytes, size)? {
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
        hash,
        revision,
        artifact,
        body,
        size,
    })
}

/// L1691-1707.  The rejected revision is the blob's only holder now, so
/// the reject must both refund the bytes and delete the object.
fn reclaim_unshared(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
    replacement: &Replacement,
) -> Result<()> {
    step("rejecting an unshared blob reclaims it and refunds the bytes");
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

    let reclaimed = inputs.work.join("reclaimed.zip");
    let file = reclaimed
        .to_str()
        .context("the work directory path is not UTF-8")?;
    let key = format!("{BLOBS_BUCKET}/blobs/sha256/{}", replacement.hash);
    let mut command =
        xtask_registry_admin::wrangler(&["r2", "object", "get", &key, "--file", file, "--local"]);
    // A successful read is the failure: the shell put this in an `if`,
    // where a non-zero exit is the expected outcome and never an abort.
    let got = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run wrangler r2 object get")?;
    if got.success() {
        bail!("the rejected version's unshared blob was not reclaimed");
    }
    Ok(())
}

/// L1709-1719.  Byte-identical, so this revives the rejected revision
/// in place rather than minting another one.
fn restart_verification(
    smoke: &mut Smoke,
    inputs: &BlobInputs<'_>,
    before_bytes: &str,
    replacement: &Replacement,
) -> Result<()> {
    step("republishing identical bytes over a rejected version restarts verification");
    smoke.wrequest("PUT", inputs.publish2_path, &replacement.body, &[201])?;
    smoke.expect_body(&format!(r#""revision":"{}""#, replacement.revision))?;
    smoke.expect_body(r#""verification":"pending""#)?;
    if stored_bytes()? != sum(before_bytes, replacement.size)? {
        bail!(
            "the re-uploaded blob was not re-counted: {}",
            stored_bytes()?
        );
    }
    smoke.as_verifier();
    smoke.check(&replacement.artifact, &[200])?;
    smoke.as_publisher();
    Ok(())
}

/// L1721-1741.  `ghost` was never claimed; `foreign` belongs only to
/// the seeded user 2.  Both must answer the byte-identical refusal, so
/// an authenticated publisher cannot probe which scopes exist.  The
/// gate fires before the body is read, so the well-formed publish body
/// is irrelevant.
fn uniform_403(smoke: &mut Smoke, inputs: &BlobInputs<'_>) -> Result<()> {
    // These attempts charge the publish bucket like any others (the
    // membership gate sits after the rate limit), so the leg gets its
    // own.
    reset_publish_bucket()?;

    step("publishing to an unclaimed or foreign scope is one uniform 403");
    let ghost = format!("/api/v1/packages/ghost/{}/{}", inputs.name, inputs.version);
    smoke.wrequest("PUT", &ghost, inputs.publish, &[403])?;
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
    smoke.wrequest("PUT", &foreign, inputs.publish, &[403])?;
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
/// between the three legs that publish it, reject it and republish it
/// unchanged.
struct Replacement {
    /// `$replacement_hash`.
    hash: String,
    /// `$replacement_rev`, which the restart leg names again.
    revision: String,
    /// `$artifact2_replacement_path`.
    artifact: String,
    /// `$work/replacement.bin`, re-sent verbatim by the restart leg.
    body: Vec<u8>,
    /// `$replacement_size`, the archive's own length and not the
    /// framed body's - only the blob is accounted for.
    size: usize,
}

/// The listing fields a verdict binds to: the checksum names the
/// revision, `published_at` the publish event.
#[derive(Debug)]
struct Entry {
    checksum: String,
    published_at: String,
}

/// The two halves of the binding, read back out of the entry file the
/// shared `listing_entry` just wrote - the node verdict builders read
/// that file rather than the listing, so the extraction runs through
/// the same document they did.
///
/// Node bound `entry.checksum` unconditionally and `JSON.stringify`
/// dropped an `undefined` key, sending a verdict with no binding whose
/// 400 would have surfaced as a status mismatch several lines later; a
/// missing half fails here instead.
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

/// `verdict-stale.json` (L1621-1627): the real checksum, so the
/// revision resolves, with a publish event that never happened.
fn verdict_stale(checksum: &str) -> String {
    format!(
        r#"{{"verdict":"verified","checksum":{},"published_at":"{EPOCH}"}}"#,
        json_string(checksum)
    )
}

/// `verdict-rejected2.json` / `verdict-rejected-replacement.json`
/// (L1634-1640, L1693-1699), which differ only in the entry they bind
/// to.
fn verdict_rejected(checksum: &str, published_at: &str) -> String {
    format!(
        r#"{{"verdict":"rejected","reason":"smoke rejection","checksum":{},"published_at":{}}}"#,
        json_string(checksum),
        json_string(published_at)
    )
}

/// `JSON.stringify` of one string: its quotes and any escaping.  Both
/// builders emit their keys in the literal order the object literal
/// wrote them, which a `serde_json::Map` would sort.
fn json_string(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

/// `stored_bytes` (L1583): `meta.total_stored_bytes` as the shell's
/// `console.log` printed it, so the comparisons stay the string
/// comparisons `[[ … == … ]]` made.
fn stored_bytes() -> Result<String> {
    let value = d1_rows(TOTAL_STORED_BYTES)?
        .first()
        .and_then(|row| row.get("value").cloned())
        .context("meta has no total_stored_bytes row")?;
    Ok(xtask_registry_admin::display(&value))
}

/// `$((before_bytes + size))`, rendered as the shell rendered it.
fn sum(before_bytes: &str, size: usize) -> Result<String> {
    let before: usize = before_bytes
        .parse()
        .with_context(|| format!("total_stored_bytes is not a number: {before_bytes}"))?;
    Ok((before + size).to_string())
}

/// L1724-1725, whose stdout the shell left on the terminal.
fn reset_publish_bucket() -> Result<()> {
    d1(RESET_PUBLISH_BUCKET).context("resetting the publish bucket")
}

/// `$scope/$name`, the scoped name the admin listing keys entries by.
fn scoped(inputs: &BlobInputs<'_>) -> String {
    format!("{}/{}", inputs.scope, inputs.name)
}

/// The artifact route for one version and revision of this package.
fn artifact_path(inputs: &BlobInputs<'_>, version: &str, revision: &str) -> String {
    let BlobInputs { scope, name, .. } = *inputs;
    format!("/artifacts/{scope}/{name}/{scope}-{name}-{version}-{revision}.zip")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKSUM: &str = "3f786850e387550fdab836ed7e6dc881de23001b3f786850e387550fdab836ed";
    const PUBLISHED_AT: &str = "2026-08-04T12:00:00.000Z";

    fn listing() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "versions": [
                {
                    "name": "smoke/withdep",
                    "version": "0.2.0",
                    "revision": "0000000000000000",
                    "checksum": "0".repeat(64),
                    "published_at": "2026-08-04T11:00:00.000Z",
                    "metadata": {},
                },
                {
                    "name": "smoke/withdep",
                    "version": "0.2.1",
                    "revision": "3f786850e3875501",
                    "checksum": CHECKSUM,
                    "published_at": PUBLISHED_AT,
                    "metadata": {},
                },
            ]
        }))
        .expect("listing")
    }

    #[test]
    fn the_stale_verdict_names_the_real_revision_and_no_publish_event() {
        assert_eq!(
            verdict_stale(CHECKSUM),
            format!(r#"{{"verdict":"verified","checksum":"{CHECKSUM}","published_at":"{EPOCH}"}}"#)
        );
    }

    /// The bytes, not the document: the key order is the object
    /// literal's and there is no whitespace.  Built by hand rather than
    /// serialized so the shape cannot follow `serde_json`'s
    /// `preserve_order` feature, which is a workspace-wide choice this
    /// leg must not depend on.
    #[test]
    fn the_rejected_verdict_keeps_the_literal_key_order() {
        assert_eq!(
            verdict_rejected(CHECKSUM, PUBLISHED_AT),
            format!(
                r#"{{"verdict":"rejected","reason":"smoke rejection","checksum":"{CHECKSUM}","published_at":"{PUBLISHED_AT}"}}"#
            )
        );
    }

    #[test]
    fn a_value_needing_escaping_is_escaped_as_json_stringify_escapes_it() {
        assert_eq!(json_string(r#"a"b"#), r#""a\"b""#);
        assert_eq!(json_string("a\nb"), r#""a\nb""#);
    }

    #[test]
    fn the_binding_comes_from_the_entry_the_shared_helper_wrote() {
        let work = tempfile::tempdir().expect("work");
        let out = work.path().join("entry2.json");
        let entry = binding(&listing(), "smoke/withdep", "0.2.1", &out).expect("entry");
        assert_eq!(entry.checksum, CHECKSUM);
        assert_eq!(entry.published_at, PUBLISHED_AT);
        // The shell's verdict builders read this file rather than the
        // listing, so it has to exist by the time they would run.
        assert!(out.is_file());
    }

    #[test]
    fn a_listing_without_the_version_fails_as_the_shell_worded_it() {
        let work = tempfile::tempdir().expect("work");
        let out = work.path().join("entry2.json");
        for listing in [listing(), b"not json".to_vec()] {
            assert_eq!(
                binding(&listing, "smoke/withdep", "9.9.9", &out)
                    .expect_err("absent")
                    .to_string(),
                "the pending listing has no smoke/withdep@9.9.9"
            );
        }
        // The scoped name is matched whole: the bare package name is a
        // different key.
        assert!(binding(&listing(), "withdep", "0.2.1", &out).is_err());
    }

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
            "/artifacts/smoke/withdep/smoke-withdep-9.9.9-3f786850e387550f.zip"
        );
    }

    #[test]
    fn the_accounting_sum_is_rendered_as_the_shell_rendered_it() {
        assert_eq!(sum("1024", 512).expect("sum"), "1536");
        assert_eq!(sum("0", 0).expect("sum"), "0");
        assert!(sum("", 1).is_err());
    }
}
