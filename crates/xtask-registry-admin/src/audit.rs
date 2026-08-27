//! Read-only audit of the BACKUP bucket (`registry/docs/runbook.md`,
//! "Disaster recovery"): verified-only coverage (every verified
//! checksum has its backup copy), the dump/sidecar pairing, and - when
//! a registry token is available - the governor's backup and dump
//! pools against the bucket's actual contents.
//!
//! It never deletes anything: the deployed BACKUP bucket's `blobs/`
//! namespace is append-only (the nightly job prunes its own `d1/`
//! dumps), and an object the current verified set does not name may be
//! legitimate history (a pre-wipe backup, an older restore's blobs), so
//! cleanup is an operator decision made per object with
//! `wrangler r2 object delete` plus `cargo registry-governor release`,
//! never a bulk sweep.
//!
//! Requires `CLOUDFLARE_API_TOKEN` (the R2 listing) and wrangler auth
//! (D1).  `CABIN_REGISTRY_TOKEN` additionally compares the governor's
//! ledger; without it the ledger sections are skipped.
//!
//! This is where the crate departs from the disclosure rule
//! `cargo registry-diagnose` keeps (`docs/architecture.md`,
//! "`xtask-registry-admin`"): an operator cannot act on a divergence
//! without the keys naming it, and those keys are content checksums.
//! `--keys` prints them on request; a listing that cannot be paginated
//! carries the page body into its error either way, because a page
//! that will not say where to resume *is* the diagnosis.  Neither
//! output goes in an incident thread.
//!
//! Ceilings, where this deliberately stops short of the shell it
//! replaces.  All are fail-closed: the audit stops where the shell
//! carried on, never the reverse.
//!
//! - an answer must carry every field the request named.  A listed
//!   object with no key or no integer size, a verified row with no
//!   checksum or no integer archive size, a queue depth with no count,
//!   a page whose fields are not the documented types - each ends the
//!   run.  The shell rendered whatever came out through a template
//!   literal and audited *that*, so some of these reached the same
//!   non-zero exit by another route, and some did not: a queue depth
//!   with no count only ever reached the remedy line of a section that
//!   found nothing, so a healthy audit passed with an `undefined` it
//!   never printed;
//! - `--keys` is the only argument, and it must be the only one.  The
//!   shell read `$1` alone, so `--keys anything` ran with the keys
//!   shown and an empty first argument ran without them;
//! - a proxy-only network is not reachable.  `ureq` takes its proxy
//!   from the agent, not from `HTTPS_PROXY`/`ALL_PROXY`, where `curl`
//!   read the environment - the same ceiling the crate's other HTTP
//!   clients have;
//! - a divergent audit ends with an `error:` line naming the sections
//!   above it, where the shell exited 1 with nothing after them.

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

const UNDERSTATED: &str = concat!(
    "    the ledger understates the bucket - it must stay an upper bound;",
    " run cargo registry-backup-backfill from the repository root (backup pool)",
    " and investigate dumps",
);

/// One object of the bucket listing.
#[derive(Deserialize)]
struct Object {
    key: String,
    size: u64,
}

/// One page of the R2 REST listing.
#[derive(Deserialize)]
struct Page {
    success: bool,
    result: Vec<Object>,
    result_info: Option<PageInfo>,
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

/// The whole bucket, cursor-paginated.  Unlike the wipe's delete loop,
/// an audit must walk every page.
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

/// Neither `curl` carried `-L`, so a redirect was the answer, never a
/// step on the way to one - and a redirected read is the one way this
/// could audit a bucket or a ledger other than the one it names.
/// `ureq` returns the 3xx as `Ok` with this set, which is what `curl`
/// did too: the listing then fails to parse and the ledger read fails
/// its status check, each with the body the redirect carried.
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
/// missing metadata entirely, fails the audit rather than silently
/// reading a partial listing as the whole bucket.
///
/// # Errors
///
/// If the page is not a successful, paginatable R2 listing.
fn page(body: &str) -> Result<(Vec<Object>, String)> {
    let page: Page = serde_json::from_str(body)?;
    if !page.success {
        bail!("the listing reported failure");
    }
    let cursor = match page.result_info.context("no pagination metadata")? {
        PageInfo {
            is_truncated: Some(true),
            cursor,
        } => encode_uri_component(
            &cursor
                .filter(|cursor| !cursor.is_empty())
                .context("a truncated page carries no cursor")?,
        ),
        PageInfo {
            is_truncated: Some(false),
            ..
        } => String::new(),
        PageInfo { .. } => bail!("no is_truncated flag"),
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
    let blobs = blob_map(listing);
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
        "append-only history (pre-wipe backups, older restores); not deleted by tooling",
        false,
    );

    layout(&mut report, listing);

    // The governor's view, when a token was available: the ledger must
    // never understate the bucket (upper bound of reality).
    if let Some(snapshot) = snapshot {
        ledger(&mut report, snapshot, listing, &blobs);
    }

    Ok(report)
}

/// The `blobs/` namespace as `new Map(entries)` held it: one entry per
/// key, in first-seen order, carrying the last size listed for it.  A
/// bucket cannot list a key twice, so this only decides what the audit
/// reports if the API ever repeats one across pages - and there it
/// keeps the count and the byte total honest.
fn blob_map(listing: &[Object]) -> Vec<(&str, u64)> {
    let mut order: Vec<&str> = Vec::new();
    let mut sizes: HashMap<&str, u64> = HashMap::new();
    for object in listing
        .iter()
        .filter(|object| object.key.starts_with(BLOBS))
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
fn ledger(report: &mut Report, snapshot: &Snapshot, listing: &[Object], blobs: &[(&str, u64)]) {
    let (backup_bytes, backup_objects) = pool(snapshot, "backup");
    let (dump_bytes, dump_objects) = pool(snapshot, "dump");
    let blob_bytes: u64 = blobs.iter().map(|(_, size)| size).sum();
    let (d1_count, d1_bytes) = listing
        .iter()
        .filter(|object| object.key.starts_with("d1/"))
        .fold((0_usize, 0_u64), |(count, bytes), object| {
            (count + 1, bytes + object.size)
        });
    report.lines.push(format!(
        "backup pool ledger: {backup_bytes} B / {backup_objects} object(s); \
         bucket blobs/: {blob_bytes} B / {} object(s)",
        blobs.len()
    ));
    report.lines.push(format!(
        "dump pool ledger:   {dump_bytes} B / {dump_objects} object(s); \
         bucket d1/:    {d1_bytes} B / {d1_count} object(s)"
    ));
    if backup_bytes < blob_bytes || dump_bytes < d1_bytes {
        report.lines.push(UNDERSTATED.to_owned());
        report.failed = true;
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
            // corrupt or pre-migration row the clean break does not
            // read - refuse loudly instead of deriving a bogus key.
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
    //! Every expectation below was taken from the shell the port
    //! replaces, run over the same fixtures.  They live here rather
    //! than in `tests/` because what they exercise - the listing and
    //! ledger wire shapes, and what the audit reads them to mean -
    //! is not this crate's API.

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

    /// A bucket that agrees with D1 reports six empty sections, the dump
    /// retention count, and - given a token - both ledgers, and passes.
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
                { "pool": "backup", "state": "live", "bytes": 1000, "objects": 5 },
                { "pool": "dump", "state": "live", "bytes": 1000, "objects": 5 },
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
    /// set does not name is history (a pre-wipe backup, an older
    /// restore) - reported because it holds ledger allowance, never a
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
            "    append-only history (pre-wipe backups, older restores); not deleted by tooling"
        );
    }

    /// The ledger is an upper bound of the bucket, so understating either
    /// pool is a failure; overstating it is the append-only history above.
    #[test]
    fn a_ledger_that_understates_the_bucket_fails() {
        let checksum = "c".repeat(64);
        let understated = |backup: u64, dump: u64| {
            run(
                &[
                    (&format!("blobs/sha256/{checksum}"), 100),
                    ("d1/2026-01-01.sql", 20),
                    ("d1/2026-01-01.sql.sha256", 64),
                ],
                &serde_json::json!([{ "checksum": format!("sha256:{checksum}"), "size": 100 }]),
                0,
                Some(&snapshot(&serde_json::json!([
                    { "pool": "backup", "state": "live", "bytes": backup, "objects": 1 },
                    { "pool": "dump", "state": "live", "bytes": dump, "objects": 2 },
                ]))),
                false,
            )
        };
        assert!(!understated(100, 84).failed, "an exact ledger is a bound");
        assert!(understated(99, 84).failed, "the backup pool understates");
        assert!(understated(100, 83).failed, "the dump pool understates");
        assert!(
            !understated(1000, 1000).failed,
            "an overstating ledger is the history section's business"
        );
    }

    /// A key the listing repeats is one object, carrying the last size
    /// listed for it - the `Map` and the `Set` the shell built its
    /// `blobs/` view and its sidecar set from.  Only those two collapse:
    /// the dump count and the `d1/` ledger were plain filters, so they
    /// count the repeat.  Nothing in R2 lists a key twice today; this is
    /// what keeps a paging quirk from inventing an orphan sidecar or
    /// failing an exact ledger.
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
                { "pool": "backup", "state": "live", "bytes": 1000, "objects": 5 },
                { "pool": "dump", "state": "live", "bytes": 1000, "objects": 5 },
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
                "    append-only history (pre-wipe backups, older restores); not deleted by tooling",
                "dumps without a validating sidecar: 0",
                "sidecars without their dump: 0",
                "keys outside the blobs/ and d1/ layouts: 0",
                "dumps retained: 1 (retention keeps 30 dailies + 12 monthly firsts)",
                "backup pool ledger: 1000 B / 5 object(s); bucket blobs/: 57 B / 2 object(s)",
                "dump pool ledger:   1000 B / 5 object(s); bucket d1/:    148 B / 3 object(s)",
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

    /// Pagination is the audit's one chance to be wrong about the whole
    /// bucket: a page that claims more without saying where, or says
    /// nothing about truncation at all, must stop the run.  Reading a
    /// first page as the whole bucket would report every later object as
    /// a missing backup.
    #[test]
    fn a_page_that_cannot_be_paginated_stops_the_audit() {
        let (objects, cursor) = page(
            r#"{"success":true,"result":[{"key":"a","size":1}],
                "result_info":{"is_truncated":false}}"#,
        )
        .unwrap();
        assert_eq!(objects.len(), 1);
        assert!(cursor.is_empty(), "a complete listing ends the loop");

        let (_, cursor) = page(
            r#"{"success":true,"result":[],
                "result_info":{"is_truncated":true,"cursor":"a+b/c="}}"#,
        )
        .unwrap();
        assert_eq!(cursor, "a%2Bb%2Fc%3D", "the cursor is escaped for a query");

        for refused in [
            r#"{"success":false,"result":[],"result_info":{"is_truncated":false}}"#,
            r#"{"success":true,"result":[]}"#,
            r#"{"success":true,"result":[],"result_info":{}}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":"false"}}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":true}}"#,
            r#"{"success":true,"result":[],"result_info":{"is_truncated":true,"cursor":""}}"#,
            r#"{"success":true,"result":[{"key":"a"}],"result_info":{"is_truncated":false}}"#,
            "not json",
        ] {
            assert!(page(refused).is_err(), "accepted {refused}");
        }
    }

    /// `encodeURIComponent`, which is what the cursor was escaped with
    /// before it went back into the query string.
    #[test]
    fn the_cursor_is_escaped_as_javascript_escaped_it() {
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
