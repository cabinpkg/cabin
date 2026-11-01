//! Read-only audit of the BACKUP bucket (`registry/docs/runbook.md`,
//! "Disaster recovery"): verified-only coverage (every verified
//! checksum has its backup copy), the dump/sidecar pairing, and - when
//! a registry token is available - the governor's backup and dump
//! pools against the bucket's actual contents.
//!
//! It never deletes anything: the deployed BACKUP bucket's `blobs/`
//! namespace is append-only outside the pre-launch wipe (the nightly
//! job prunes its own `d1/` dumps), and an object the current verified
//! set does not name may be legitimate history (an older restore's
//! blobs), so cleanup is an operator decision made per object with
//! `wrangler r2 object delete` plus `cargo registry-governor release`,
//! never a bulk sweep.
//!
//! Requires `CLOUDFLARE_API_TOKEN` (the R2 listing) and wrangler auth
//! (D1).  `CABIN_REGISTRY_TOKEN` additionally compares the governor's
//! ledger; without it the ledger sections are skipped.
//!
//! This is where the crate departs from the disclosure rule
//! `cargo registry-diagnose` keeps (`registry/docs/runbook.md`,
//! "Logs"): an operator cannot act on a divergence
//! without the keys naming it, and those keys are content checksums.
//! `--keys` prints them on request; a listing that cannot be paginated
//! carries the page body into its error either way, because a page
//! that will not say where to resume *is* the diagnosis.  Neither
//! output goes in an incident thread.
//!
//! Ceilings, all fail-closed:
//!
//! - an answer must carry every field the request named.  A listed
//!   object with no key or no integer size, a verified row with no
//!   checksum or no integer archive size, a queue depth with no count,
//!   a page whose fields are not the documented types - each ends the
//!   run;
//! - `--keys` is the only argument, and it must be the only one;
//! - a proxy-only network is not reachable.  `ureq` takes its proxy
//!   from the agent, not from `HTTPS_PROXY`/`ALL_PROXY` - the same
//!   ceiling the crate's other HTTP clients have.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::governor::Snapshot;
use crate::{BACKUP_BUCKET, account_id, display, output, results, step, wrangler};

const BLOBS: &str = "blobs/sha256/";

const VERIFIED: &str = "
  SELECT checksum, MAX(archive_size) AS size FROM revisions
  WHERE verification = 'verified' GROUP BY checksum";

const QUEUE_DEPTH: &str = "SELECT COUNT(*) AS n FROM backup_pending";

/// One object of the bucket listing.
#[derive(Deserialize)]
struct Object {
    key: String,
    size: u64,
}

/// One page of the R2 REST listing.  `result_info` is `None` only when
/// the key is missing: a `null` is metadata that is not an object, and
/// fails like one.
#[derive(Deserialize)]
struct Page {
    success: bool,
    result: Vec<Object>,
    #[serde(default, deserialize_with = "present")]
    result_info: Option<PageInfo>,
}

/// `deserialize_with` for an R2 listing's `result_info`, so only a
/// missing key reads as `None`.
pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    value: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(value).map(Some)
}

#[derive(Deserialize)]
struct PageInfo {
    is_truncated: Option<bool>,
    cursor: Option<String>,
}

/// Runs the audit.
///
/// # Errors
///
/// If a listing, D1 read or ledger snapshot fails or answers in an
/// unexpected shape, or if the audit itself finds a divergence.
pub fn run(show_keys: bool) -> Result<()> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CLOUDFLARE_API_TOKEN is required to list the backup bucket");
    }
    let account = account_id()?;

    step(&format!("listing {BACKUP_BUCKET}"));
    let listing = list(&token, &account)?;
    println!("    {} object(s)", listing.len());

    step("reading the verified checksums and queue depth from D1");
    let verified = d1(VERIFIED).context("the verified-checksum query failed")?;
    let queue = d1(QUEUE_DEPTH).context("the queue-depth query failed")?;

    let snapshot = snapshot()?;

    step("auditing");
    let report = audit(&listing, &verified, &queue, snapshot.as_ref(), show_keys)?;
    for line in &report.lines {
        println!("{line}");
    }
    if report.failed {
        bail!("the backup audit found divergences the sections above name");
    }
    println!("backup audit OK");
    Ok(())
}

fn d1(command: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        command,
    ]))?;
    results(&answer)
}

/// The whole bucket, cursor-paginated.
fn list(token: &str, account: &str) -> Result<Vec<Object>> {
    let agent = agent();
    let endpoint = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account}/r2/buckets/{BACKUP_BUCKET}/objects"
    );
    let mut listing = Vec::new();
    let mut cursor = String::new();
    loop {
        let url = if cursor.is_empty() {
            format!("{endpoint}?per_page=1000")
        } else {
            format!("{endpoint}?per_page=1000&cursor={cursor}")
        };
        let body =
            get(&agent, &url, token).with_context(|| format!("listing {BACKUP_BUCKET} failed"))?;
        let objects;
        (objects, cursor) = page(&body)
            .with_context(|| format!("unexpected or unpaginatable R2 list response: {body}"))?;
        listing.extend(objects);
        if cursor.is_empty() {
            break;
        }
    }
    Ok(listing)
}

/// No redirects: a redirect is the answer, never a step on the way to
/// one - and a redirected read is the one way this could audit a
/// bucket or a ledger other than the one it names.  `ureq` returns the
/// 3xx as `Ok` with this set: the listing then fails to parse and the
/// ledger read fails its status check, each with the body the redirect
/// carried.
pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().redirects(0).build()
}

pub(crate) fn get(agent: &ureq::Agent, url: &str, token: &str) -> Result<String> {
    Ok(agent
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()?
        .into_string()?)
}

/// One page's objects, and the cursor the next request needs - empty
/// once the listing is complete.
///
/// Pagination is explicit: a truncated page without a cursor, or
/// metadata without a truncation flag, fails the audit rather than
/// silently reading a partial listing as the whole bucket.  The API
/// leaves the metadata out of a listing's last page altogether, so a
/// missing `result_info` key ends the listing, as `is_truncated: false`
/// does.
///
/// # Errors
///
/// If the page is not a successful, paginatable R2 listing.
fn page(body: &str) -> Result<(Vec<Object>, String)> {
    let page: Page = serde_json::from_str(body)?;
    if !page.success {
        bail!("the listing reported failure");
    }
    let cursor = match page.result_info {
        None
        | Some(PageInfo {
            is_truncated: Some(false),
            ..
        }) => String::new(),
        Some(PageInfo {
            is_truncated: Some(true),
            cursor,
        }) => encode_uri_component(
            &cursor
                .filter(|cursor| !cursor.is_empty())
                .context("a truncated page carries no cursor")?,
        ),
        Some(PageInfo { .. }) => bail!("no is_truncated flag"),
    };
    Ok((page.result, cursor))
}

/// `encodeURIComponent`, because the cursor goes into a query string
/// and R2 hands back base64 - `+`, `/` and `=` all change meaning
/// there.
pub(crate) fn encode_uri_component(value: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// The governor's ledger, when a registry token is to hand.
fn snapshot() -> Result<Option<Snapshot>> {
    // The token itself is read again by `Api::new`; this only decides
    // whether the ledger sections run at all, and reads through
    // `var`'s lossy default so a non-UTF-8 value still skips rather
    // than erroring one call later.
    if std::env::var(cabin_env::CABIN_REGISTRY_TOKEN)
        .unwrap_or_default()
        .is_empty()
    {
        step("CABIN_REGISTRY_TOKEN unset; skipping the governor ledger sections");
        return Ok(None);
    }
    step("reading the governor ledger snapshot");
    let body = crate::governor::Api::new()?.ok(None, "the governor snapshot")?;
    Ok(Some(
        serde_json::from_str(&body).context("parse the governor snapshot")?,
    ))
}

/// What the audit found: the lines it reports, in order, and whether
/// any of them is a divergence the operator must act on.
struct Report {
    lines: Vec<String>,
    failed: bool,
    show_keys: bool,
}

impl Report {
    /// One section: the count, then - only when it found something -
    /// the remedy, the keys if the operator asked for them, and (for
    /// every section but the append-only history) the audit's failure.
    fn section(&mut self, label: &str, keys: &[String], remedy: &str, hard: bool) {
        self.lines.push(format!("{label}: {}", keys.len()));
        if keys.is_empty() {
            return;
        }
        self.lines.push(format!("    {remedy}"));
        if self.show_keys {
            self.lines
                .extend(keys.iter().map(|key| format!("        {key}")));
        }
        self.failed |= hard;
    }
}

/// The audit itself, over answers already read.
///
/// # Errors
///
/// If an answer omits a field the request named.
fn audit(
    listing: &[Object],
    verified: &[serde_json::Map<String, serde_json::Value>],
    queue: &[serde_json::Map<String, serde_json::Value>],
    snapshot: Option<&Snapshot>,
    show_keys: bool,
) -> Result<Report> {
    let blobs = by_key(listing, BLOBS);
    let sizes: HashMap<&str, u64> = blobs.iter().copied().collect();
    let depth = display(
        queue
            .first()
            .and_then(|row| row.get("n"))
            .context("the queue-depth query answered no count")?,
    );
    let recorded = archives(verified)?;

    let mut report = Report {
        lines: Vec::new(),
        failed: false,
        show_keys,
    };

    // Verified-only coverage: every currently-verified checksum must
    // have its backup copy (or a queue row still working toward one),
    // and the copy must be the recorded size - a truncated object
    // under the right content-addressed key is not a backup.
    let missing: Vec<String> = recorded
        .iter()
        .filter(|(key, _)| !sizes.contains_key(key.as_str()))
        .map(|(key, _)| key.clone())
        .collect();
    report.section(
        "verified blobs missing from the backup",
        &missing,
        &format!(
            "queue depth is {depth}; if rows are stale, \
             run cargo registry-backup-backfill from the repository root"
        ),
        true,
    );

    let wrong_size: Vec<String> = recorded
        .iter()
        .filter_map(|(key, size)| {
            let held = sizes.get(key.as_str())?;
            (held != size).then(|| format!("{key} (backup {held} B, recorded {size} B)"))
        })
        .collect();
    report.section(
        "backup copies whose size disagrees with the recorded archive",
        &wrong_size,
        "a truncated or overwritten copy; re-copy via \
         cargo registry-backup-backfill (repository root) after deleting it",
        true,
    );

    // History the current verified set does not name: legitimate under
    // the append-only policy, reported because each object holds
    // backup-pool ledger allowance until an operator decides otherwise.
    let named: HashSet<&str> = recorded.iter().map(|(key, _)| key.as_str()).collect();
    let extras: Vec<String> = blobs
        .iter()
        .filter(|(key, _)| !named.contains(key))
        .map(|(key, _)| (*key).to_owned())
        .collect();
    report.section(
        "backup blobs beyond the current verified set",
        &extras,
        "append-only history (such as older restores); only the pre-launch wipe deletes it",
        false,
    );

    layout(&mut report, listing);

    // The governor's view, when a token was available: the ledger must
    // never understate the bucket (upper bound of reality).
    if let Some(snapshot) = snapshot {
        ledger(
            &mut report,
            snapshot,
            &by_key(listing, "d1/"),
            &blobs,
            &depth,
        );
    }

    Ok(report)
}

/// The keys under `prefix`: one entry per key, in first-seen order,
/// carrying the last size listed for it.  A
/// bucket cannot list a key twice, so this only decides what the audit
/// reports if the API ever repeats one across pages - and there it
/// keeps the count and the byte total honest.
fn by_key<'a>(listing: &'a [Object], prefix: &str) -> Vec<(&'a str, u64)> {
    let mut order: Vec<&str> = Vec::new();
    let mut sizes: HashMap<&str, u64> = HashMap::new();
    for object in listing
        .iter()
        .filter(|object| object.key.starts_with(prefix))
    {
        if sizes.insert(object.key.as_str(), object.size).is_none() {
            order.push(object.key.as_str());
        }
    }
    order.into_iter().map(|key| (key, sizes[key])).collect()
}

/// The bucket's own layout, independent of what D1 records: the
/// dump/sidecar pairing (the sidecar is written strictly after
/// validation, so a dump without one is an unvalidated leftover the
/// nightly job normally deletes; a sidecar without its dump means the
/// dump object was lost) and anything outside both namespaces.
fn layout(report: &mut Report, listing: &[Object]) {
    let dumps: Vec<&Object> = listing
        .iter()
        .filter(|object| is_dump(&object.key))
        .collect();
    // A `Set`, so a key listed twice is one sidecar in first-seen
    // order, not two orphans.
    let mut seen = HashSet::new();
    let sidecars: Vec<&str> = listing
        .iter()
        .map(|object| object.key.as_str())
        .filter(|key| key.ends_with(".sql.sha256") && seen.insert(*key))
        .collect();
    let strays: Vec<String> = listing
        .iter()
        .filter(|object| !object.key.starts_with(BLOBS) && !is_dump_or_sidecar(&object.key))
        .map(|object| object.key.clone())
        .collect();

    let unvalidated: Vec<String> = dumps
        .iter()
        .map(|dump| dump.key.clone())
        .filter(|key| !sidecars.contains(&format!("{key}.sha256").as_str()))
        .collect();
    report.section(
        "dumps without a validating sidecar",
        &unvalidated,
        "unvalidated leftovers; the next nightly pass deletes or replaces them",
        true,
    );

    let orphans: Vec<String> = sidecars
        .iter()
        .filter(|sidecar| {
            !dumps
                .iter()
                .any(|dump| format!("{}.sha256", dump.key) == **sidecar)
        })
        .map(|sidecar| (*sidecar).to_owned())
        .collect();
    report.section(
        "sidecars without their dump",
        &orphans,
        "the dump object is gone; investigate before trusting that date",
        true,
    );

    report.section(
        "keys outside the blobs/ and d1/ layouts",
        &strays,
        "nothing in the service writes these; investigate",
        true,
    );
    report.lines.push(format!(
        "dumps retained: {} (retention keeps 30 dailies + 12 monthly firsts)",
        dumps.len()
    ));
}

/// The governor's backup and dump pools against the bucket's actual
/// contents.  The ledger is an upper bound of reality, so understating
/// it is the failure; overstating it is the append-only history the
/// section above already reported.
///
/// Bytes and objects are separate bounds, each checked per pool: the
/// ledger holds one row per `(pool, key)`, so fewer ledger objects than
/// listed objects names an R2 object no row accounts for, however
/// generously the other rows' byte counts cover the total.
///
/// The listing precedes the snapshot, and every Worker write reserves
/// before its put, so an in-flight drain copy or dump is already in the
/// ledger the snapshot reads.  A pool can still read short honestly,
/// so each shortfall's remedy says to rerun before investigating: a
/// `cargo registry-backup-backfill` copy reaches the backup pool only
/// when the drain retires its queue row, and an operator release
/// (either pool) or a dump-job delete (prune or invalid-dump removal)
/// between the two reads drops a row whose object the listing still
/// saw.  The totals are aggregate, so a pass is not per-key proof: a
/// leaked reservation can offset an object no row names.
fn ledger(
    report: &mut Report,
    snapshot: &Snapshot,
    dumps: &[(&str, u64)],
    blobs: &[(&str, u64)],
    depth: &str,
) {
    for (name, prefix, listed, remedy) in [
        (
            "backup",
            "blobs/",
            blobs,
            format!(
                "backfilled copies reach the ledger only as the drain retires their queue rows \
                 (queue depth is {depth}), and an operator release between the listing and the \
                 snapshot reads short honestly, so rerun once the queue is empty and no release \
                 is in flight; a gap that outlasts both is real: run cargo \
                 registry-backup-backfill from the repository root and investigate"
            ),
        ),
        (
            "dump",
            "d1/",
            dumps,
            "a dump-job delete or an operator release between the listing and the snapshot \
             reads short honestly, so rerun once none is in flight; a gap that outlasts it is \
             real: investigate"
                .to_owned(),
        ),
    ] {
        let (ledger_bytes, ledger_objects) = pool(snapshot, name);
        let bytes: u64 = listed.iter().map(|(_, size)| size).sum();
        let objects = listed.len() as u64;
        report.lines.push(format!(
            "{:<20}{ledger_bytes} B / {ledger_objects} object(s); {:<15}{bytes} B / {objects} \
             object(s)",
            format!("{name} pool ledger:"),
            format!("bucket {prefix}:"),
        ));
        let short: Vec<&str> = [
            ("bytes", ledger_bytes < bytes),
            ("objects", ledger_objects < objects),
        ]
        .into_iter()
        .filter_map(|(measure, understated)| understated.then_some(measure))
        .collect();
        if !short.is_empty() {
            report.lines.push(format!(
                "    the {name} pool ledger understates the bucket in {} - it must stay an \
                 upper bound in bytes and in objects alike; {remedy}",
                short.join(" and ")
            ));
            report.failed = true;
        }
    }
}

/// The backup key and recorded archive size of every verified
/// checksum.
fn archives(verified: &[serde_json::Map<String, serde_json::Value>]) -> Result<Vec<(String, u64)>> {
    verified
        .iter()
        .map(|row| {
            let checksum = row
                .get("checksum")
                .context("a verified row carries no checksum")?;
            let size = row
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .context("a verified row carries no integer archive size")?;
            // The column holds the canonical `sha256:<hex>` value;
            // R2 keys keep the OCI-style layout, so the key drops
            // the algorithm prefix.  Anything non-canonical is a
            // corrupt or unsupported pre-migration row: refuse it
            // instead of deriving a bogus key.
            let value = display(checksum);
            if !crate::backfill::is_checksum(&value) {
                anyhow::bail!("a verified row carries a non-canonical checksum: {value}");
            }
            Ok((format!("{BLOBS}{}", &value["sha256:".len()..]), size))
        })
        .collect()
}

/// One storage pool's ledger totals, summed across its states.
fn pool(snapshot: &Snapshot, name: &str) -> (u64, u64) {
    snapshot
        .storage
        .iter()
        .filter(|row| row.pool == name)
        .fold((0, 0), |(bytes, objects), row| {
            (bytes + row.bytes, objects + row.objects)
        })
}

/// `^d1/\d{4}-\d{2}-\d{2}\.sql$`, the layout the nightly job writes.
fn is_dump(key: &str) -> bool {
    let Some(date) = key
        .strip_prefix("d1/")
        .and_then(|rest| rest.strip_suffix(".sql"))
    else {
        return false;
    };
    let date = date.as_bytes();
    date.len() == 10
        && date[4] == b'-'
        && date[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&index| date[index].is_ascii_digit())
}

/// The same, with the optional `.sha256` sidecar suffix.
fn is_dump_or_sidecar(key: &str) -> bool {
    is_dump(key.strip_suffix(".sha256").unwrap_or(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(objects: &[(&str, u64)]) -> Vec<Object> {
        objects
            .iter()
            .map(|(key, size)| {
                serde_json::from_value(serde_json::json!({ "key": key, "size": size })).unwrap()
            })
            .collect()
    }

    fn answer(rows: &serde_json::Value) -> Vec<serde_json::Map<String, serde_json::Value>> {
        results(&serde_json::json!([{ "results": rows, "success": true }]).to_string()).unwrap()
    }

    fn snapshot(pools: &serde_json::Value) -> Snapshot {
        serde_json::from_value(serde_json::json!({ "storage": pools, "ops": [] })).unwrap()
    }

    fn run(
        objects: &[(&str, u64)],
        verified: &serde_json::Value,
        depth: u64,
        ledger: Option<&Snapshot>,
        show_keys: bool,
    ) -> Report {
        audit(
            &listing(objects),
            &answer(verified),
            &answer(&serde_json::json!([{ "n": depth }])),
            ledger,
            show_keys,
        )
        .unwrap()
    }

    #[test]
    fn a_healthy_bucket_reports_every_section_empty() {
        let checksum = "0123456789abcdef".repeat(4);
        let blob = format!("blobs/sha256/{checksum}");
        let report = run(
            &[
                (&blob, 100),
                ("d1/2026-01-01.sql", 20),
                ("d1/2026-01-01.sql.sha256", 64),
            ],
            &serde_json::json!([{ "checksum": format!("sha256:{checksum}"), "size": 100 }]),
            0,
            Some(&snapshot(&serde_json::json!([
                { "pool": "backup", "state": "committed", "bytes": 1000, "objects": 5 },
                { "pool": "dump", "state": "committed", "bytes": 1000, "objects": 5 },
            ]))),
            false,
        );
        assert!(!report.failed);
        assert_eq!(
            report.lines,
            [
                "verified blobs missing from the backup: 0",
                "backup copies whose size disagrees with the recorded archive: 0",
                "backup blobs beyond the current verified set: 0",
                "dumps without a validating sidecar: 0",
                "sidecars without their dump: 0",
                "keys outside the blobs/ and d1/ layouts: 0",
                "dumps retained: 1 (retention keeps 30 dailies + 12 monthly firsts)",
                "backup pool ledger: 1000 B / 5 object(s); bucket blobs/: 100 B / 1 object(s)",
                "dump pool ledger:   1000 B / 5 object(s); bucket d1/:    84 B / 2 object(s)",
            ]
        );
    }

    /// Every hard section at once, with `--keys`: each prints its count,
    /// its remedy and its keys, and the audit fails.  A section that
    /// stopped reporting - or reported without failing - would let an
    /// unbacked registry read as healthy.
    #[test]
    fn every_divergence_names_itself_and_fails_the_audit() {
        let held = "a".repeat(64);
        let missing = "b".repeat(64);
        let report = run(
            &[
                (&format!("blobs/sha256/{held}"), 99),
                ("d1/2026-01-01.sql", 20),
                ("d1/2026-02-01.sql.sha256", 64),
                ("stray", 1),
            ],
            &serde_json::json!([
                { "checksum": format!("sha256:{held}"), "size": 100 },
                { "checksum": format!("sha256:{missing}"), "size": 50 },
            ]),
            7,
            None,
            true,
        );
        assert!(report.failed);
        assert_eq!(
            report.lines,
            [
                "verified blobs missing from the backup: 1".to_owned(),
                "    queue depth is 7; if rows are stale, run cargo registry-backup-backfill \
                 from the repository root"
                    .to_owned(),
                format!("        blobs/sha256/{missing}"),
                "backup copies whose size disagrees with the recorded archive: 1".to_owned(),
                "    a truncated or overwritten copy; re-copy via cargo registry-backup-backfill \
                 (repository root) after deleting it"
                    .to_owned(),
                format!("        blobs/sha256/{held} (backup 99 B, recorded 100 B)"),
                "backup blobs beyond the current verified set: 0".to_owned(),
                "dumps without a validating sidecar: 1".to_owned(),
                "    unvalidated leftovers; the next nightly pass deletes or replaces them"
                    .to_owned(),
                "        d1/2026-01-01.sql".to_owned(),
                "sidecars without their dump: 1".to_owned(),
                "    the dump object is gone; investigate before trusting that date".to_owned(),
                "        d1/2026-02-01.sql.sha256".to_owned(),
                "keys outside the blobs/ and d1/ layouts: 1".to_owned(),
                "    nothing in the service writes these; investigate".to_owned(),
                "        stray".to_owned(),
                "dumps retained: 1 (retention keeps 30 dailies + 12 monthly firsts)".to_owned(),
            ]
        );
    }

    /// The one section that reports without failing.  The BACKUP bucket's
    /// `blobs/` namespace is append-only, so a blob the current verified
    /// set does not name is history (an older restore, say) - reported
    /// because it holds ledger allowance, never a
    /// reason to fail a run or to delete anything.
    #[test]
    fn history_beyond_the_verified_set_is_reported_but_not_a_failure() {
        let report = run(
            &[(&format!("blobs/sha256/{}", "b".repeat(64)), 50)],
            &serde_json::json!([]),
            0,
            None,
            false,
        );
        assert!(!report.failed);
        assert_eq!(
            report.lines[2],
            "backup blobs beyond the current verified set: 1"
        );
        assert_eq!(
            report.lines[3],
            "    append-only history (such as older restores); only the pre-launch wipe deletes it"
        );
    }

    /// The ledger is an upper bound of the bucket in bytes and in objects,
    /// each pool and each measure on its own: understating any one fails
    /// and names itself, whatever the others hold - generous bytes never
    /// cover a missing object row.  Overstating is the append-only history
    /// above.
    #[test]
    fn a_ledger_that_understates_the_bucket_fails() {
        let checksum = "c".repeat(64);
        let audited = |storage: serde_json::Value| {
            run(
                &[
                    (&format!("blobs/sha256/{checksum}"), 100),
                    ("d1/2026-01-01.sql", 20),
                    ("d1/2026-01-01.sql.sha256", 64),
                ],
                &serde_json::json!([{ "checksum": format!("sha256:{checksum}"), "size": 100 }]),
                3,
                Some(&snapshot(&storage)),
                false,
            )
        };
        let understated = |backup: (u64, u64), dump: (u64, u64)| {
            audited(serde_json::json!([
                { "pool": "backup", "state": "committed", "bytes": backup.0, "objects": backup.1 },
                { "pool": "dump", "state": "committed", "bytes": dump.0, "objects": dump.1 },
            ]))
        };
        let short = |report: &Report| -> Vec<String> {
            report
                .lines
                .iter()
                .filter_map(|line| {
                    let (pool, rest) = line
                        .strip_prefix("    the ")?
                        .split_once(" pool ledger understates the bucket in ")?;
                    Some(format!("{pool} {}", rest.split_once(" - ")?.0))
                })
                .collect()
        };

        // An in-flight sidecar is a reservation: a pool's bound sums
        // its reserved and committed rows.
        let exact = audited(serde_json::json!([
            { "pool": "backup", "state": "committed", "bytes": 100, "objects": 1 },
            { "pool": "dump", "state": "committed", "bytes": 20, "objects": 1 },
            { "pool": "dump", "state": "reserved", "bytes": 64, "objects": 1 },
        ]));
        assert!(
            !exact.failed,
            "an exact ledger is a bound: {:?}",
            exact.lines
        );
        for (backup, dump, measures) in [
            ((99, 1), (84, 2), &["backup bytes"][..]),
            ((1000, 0), (84, 2), &["backup objects"]),
            ((100, 1), (83, 2), &["dump bytes"]),
            ((100, 1), (1000, 1), &["dump objects"]),
            (
                (99, 0),
                (83, 1),
                &["backup bytes and objects", "dump bytes and objects"],
            ),
        ] {
            let report = understated(backup, dump);
            assert!(report.failed, "{measures:?}");
            assert_eq!(short(&report), measures);
        }
        // Each pool names its own honest-lag causes, the backup pool
        // with the queue depth that would explain a backfill lag.
        let both = understated((0, 0), (0, 0));
        let line = |pool: &str| {
            both.lines
                .iter()
                .find(|line| line.starts_with(&format!("    the {pool} pool")))
                .cloned()
                .unwrap_or_default()
        };
        for (pool, causes) in [
            ("backup", ["queue depth is 3", "an operator release"]),
            ("dump", ["a dump-job delete", "an operator release"]),
        ] {
            let line = line(pool);
            for cause in causes {
                assert!(line.contains(cause), "{pool}: {line}");
            }
        }
        assert!(
            !understated((1000, 9), (1000, 9)).failed,
            "an overstating ledger is the history section's business"
        );
    }

    /// A key the listing repeats is one object, carrying the last size
    /// listed for it, in the `blobs/` and `d1/` views and the sidecar
    /// set; only the dump retention count is a plain filter that counts
    /// the repeat.  Nothing in R2 lists a key twice today; this is what
    /// keeps a paging quirk from inventing an orphan sidecar or failing
    /// an exact ledger.
    #[test]
    fn a_repeated_key_is_one_object_carrying_its_last_size() {
        let checksum = "0123456789abcdef".repeat(4);
        let blob = format!("blobs/sha256/{checksum}");
        let history = format!("blobs/sha256/{}", "b".repeat(64));
        let report = run(
            &[
                (&blob, 100),
                (&history, 50),
                (&blob, 7),
                (&history, 50),
                ("d1/2026-01-01.sql", 20),
                ("d1/2026-01-01.sql.sha256", 64),
                ("d1/2026-01-01.sql.sha256", 64),
            ],
            &serde_json::json!([{ "checksum": format!("sha256:{checksum}"), "size": 100 }]),
            0,
            Some(&snapshot(&serde_json::json!([
                { "pool": "backup", "state": "committed", "bytes": 1000, "objects": 5 },
                { "pool": "dump", "state": "committed", "bytes": 1000, "objects": 5 },
            ]))),
            false,
        );
        assert!(
            report.failed,
            "the second listing of the blob is a mismatch"
        );
        assert_eq!(
            report.lines,
            [
                "verified blobs missing from the backup: 0",
                "backup copies whose size disagrees with the recorded archive: 1",
                "    a truncated or overwritten copy; re-copy via \
                 cargo registry-backup-backfill (repository root) after deleting it",
                "backup blobs beyond the current verified set: 1",
                "    append-only history (such as older restores); only the pre-launch wipe deletes it",
                "dumps without a validating sidecar: 0",
                "sidecars without their dump: 0",
                "keys outside the blobs/ and d1/ layouts: 0",
                "dumps retained: 1 (retention keeps 30 dailies + 12 monthly firsts)",
                "backup pool ledger: 1000 B / 5 object(s); bucket blobs/: 57 B / 2 object(s)",
                "dump pool ledger:   1000 B / 5 object(s); bucket d1/:    84 B / 2 object(s)",
            ]
        );
    }

    /// Only `d1/<date>.sql` and its `.sha256` sidecar belong beside
    /// `blobs/sha256/`.  A near-miss - an unpadded date, a doubled
    /// suffix, a sidecar outside `d1/` - is a key nothing in the service
    /// writes, and the audit says so rather than quietly counting it as a
    /// dump.
    #[test]
    fn the_dump_layout_admits_only_padded_dates_and_one_sidecar() {
        let report = run(
            &[
                ("d1/2026-01-01.sql", 1),
                ("d1/2026-01-01.sql.sha256", 2),
                ("d1/2026-1-1.sql", 3),
                ("d1/2026-01-01.sql.sha256.sha256", 4),
                ("d1/2026-01-01.SQL", 5),
                ("notes.sql.sha256", 6),
                ("d1/", 7),
                ("", 8),
            ],
            &serde_json::json!([]),
            0,
            None,
            true,
        );
        assert!(report.failed);
        let strays: Vec<&String> = report
            .lines
            .iter()
            .skip_while(|line| !line.starts_with("keys outside"))
            .skip(2)
            .take_while(|line| line.starts_with("        "))
            .collect();
        assert_eq!(
            strays,
            [
                "        d1/2026-1-1.sql",
                "        d1/2026-01-01.sql.sha256.sha256",
                "        d1/2026-01-01.SQL",
                "        notes.sql.sha256",
                "        d1/",
                "        ",
            ]
        );
        // `notes.sql.sha256` is both a stray and a sidecar with no dump:
        // the sidecar set is every `.sql.sha256` key, wherever it sits.
        assert!(
            report
                .lines
                .contains(&"sidecars without their dump: 1".to_owned())
        );
        assert!(report.lines.contains(
            &"dumps retained: 1 (retention keeps 30 dailies + 12 monthly firsts)".to_owned()
        ));
    }

    /// A page that claims more without saying where, or carries metadata
    /// that says nothing about truncation, stops the run: reading a first
    /// page as the whole bucket would audit only part of it and could
    /// still report it OK.
    #[test]
    fn a_page_that_cannot_be_paginated_stops_the_audit() {
        let (objects, cursor) = page(
            r#"{"success":true,"result":[{"key":"a","size":1}],
                "result_info":{"is_truncated":false}}"#,
        )
        .unwrap();
        assert_eq!(objects.len(), 1);
        assert!(cursor.is_empty(), "a complete listing ends the loop");
        // The live API's last page carries no metadata at all.
        let (objects, cursor) =
            page(r#"{"success":true,"errors":[],"result":[{"key":"a","size":1}]}"#).unwrap();
        assert_eq!(objects.len(), 1);
        assert!(cursor.is_empty(), "a last page ends the loop");

        let (_, cursor) = page(
            r#"{"success":true,"result":[],
                "result_info":{"is_truncated":true,"cursor":"a+b/c="}}"#,
        )
        .unwrap();
        assert_eq!(cursor, "a%2Bb%2Fc%3D", "the cursor is escaped for a query");

        for refused in [
            r#"{"success":false,"result":[],"result_info":{"is_truncated":false}}"#,
            r#"{"success":true,"result":[],"result_info":{}}"#,
            r#"{"success":true,"result":[],"result_info":null}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":"false"}}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":true}}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":true,"cursor":""}}"#,
            r#"{"success":true,"result":[{"key":"a"}],"result_info":{"is_truncated":false}}"#,
            "not json",
        ] {
            assert!(page(refused).is_err(), "accepted {refused}");
        }
    }

    #[test]
    fn the_cursor_is_percent_encoded_like_encode_uri_component() {
        for (raw, escaped) in [
            ("simple", "simple"),
            (
                "AAAA////++++====",
                "AAAA%2F%2F%2F%2F%2B%2B%2B%2B%3D%3D%3D%3D",
            ),
            ("with space", "with%20space"),
            ("-_.!~*'()", "-_.!~*'()"),
            ("%already", "%25already"),
            ("a&b?c#d", "a%26b%3Fc%23d"),
            ("\u{e9}", "%C3%A9"),
        ] {
            assert_eq!(encode_uri_component(raw), escaped, "escaping {raw}");
        }
    }
}
