//! Operator surface for the cost governor's ledger
//! (`registry/docs/runbook.md`, "The cost governor"): the guarded
//! spellings of the admin endpoint's actions, so ledger maintenance
//! never starts from a hand-typed `curl`.
//!
//! `usage`, `compare` and `reconcile` are safe on a live registry
//! (reconcile is increase-only).  `release` and `wipe` mutate the
//! ledger, and every guard in front of them serves one invariant: the
//! ledger must never UNDERSTATE reality, because an understating
//! ledger admits writes past the true R2 cap.  Releasing an entry for
//! an object that still exists is exactly that, so absence has to be
//! *proven* - never inferred from a check that failed.
//!
//! Every action needs `CABIN_REGISTRY_TOKEN` (a login-session token
//! whose `verify` scope authenticates the admin endpoint).
//! `release` and `wipe` also need `CLOUDFLARE_API_TOKEN`: their guards
//! prove object absence through the R2 REST API, and unlike the audit
//! and the diagnostics bundle - which skip a section when a token is
//! missing - an absent token here is fatal.  A skipped evidence check
//! is not evidence.
//!
//! `CABIN_API_ORIGIN` overrides the API origin for scratch rehearsal
//! deployments (default `https://cabinpkg.com`; https only).  The
//! bearer token goes to whichever origin the operator names here, so
//! a rehearsal uses a session minted on that scratch deployment,
//! never the production one.
//!
//! Two places where this is deliberately STRICTER than the shell it
//! replaces, because the shell contradicted its own stated rule
//! ("absence must be proven, never inferred from an error"):
//!
//! - the shell built its listing URL with
//!   `?prefix=$(node -e '...encodeURIComponent...')`.  A command
//!   substitution in an argument is not covered by `set -e`, so a
//!   `node` that died for any reason expanded to the empty string and
//!   listed the whole bucket - five arbitrary objects, none of them
//!   the key being released, read as "affirmatively absent".  Here the
//!   encoder is a pure function that cannot fail, and an empty prefix
//!   is refused outright;
//! - the shell's evidence snippets exited 1 for "affirmatively absent"
//!   and 2 for "unparsable", but `node` also exits 1 on an uncaught
//!   exception - so a listing like `{"result":[null]}` threw past the
//!   `try` and was read as absence.  Here the three answers are a
//!   `Result<bool>`: `Ok(false)` is proven absence and every other
//!   failure is an error.  A row that is not an object with a `key`
//!   ends the run.
//!
//! Ceilings, where it stops short of the shell instead.  All are
//! fail-closed:
//!
//! - a listing whose page is truncated is still read as the whole
//!   match set, as the shell read it.  What makes one page enough is
//!   the key grammar checked before the listing: a `blobs/sha256/`
//!   key is fixed-length so nothing extends it, and a dump key has at
//!   most its `.sha256` sibling, so `per_page=5` cannot overflow.  The
//!   grammar check is part of the guard, not input hygiene;
//! - a proxy-only network is not reachable, the same ceiling the
//!   crate's other HTTP clients carry.

use std::io::{Read as _, Write as _};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    BACKUP_BUCKET as BACKUP, BLOBS_BUCKET as PRIMARY, account_id, display, output, results, step,
    wrangler,
};

/// The service modes that mean no publisher can land a write while the
/// evidence is being gathered.
const COORDINATED: [&str; 2] = ["writes_blocked", "reads_blocked"];

const SERVICE_MODE: &str = "SELECT value FROM meta WHERE key = 'service_mode'";

const TOTALS: &str = "
      SELECT
        (SELECT COUNT(*) FROM (SELECT checksum FROM revisions
          WHERE verification != 'rejected' GROUP BY checksum)) AS live_objects,
        (SELECT COALESCE(SUM(size), 0) FROM (SELECT MAX(archive_size) AS size
          FROM revisions WHERE verification != 'rejected' GROUP BY checksum)) AS live_bytes,
        (SELECT COUNT(*) FROM (SELECT checksum FROM revisions
          WHERE verification = 'verified' GROUP BY checksum)) AS verified_objects,
        (SELECT COALESCE(SUM(size), 0) FROM (SELECT MAX(archive_size) AS size
          FROM revisions WHERE verification = 'verified' GROUP BY checksum)) AS verified_bytes
    ";

// Expiry mirrors the auth lookup's strict lexicographic bound over the
// schema's canonical timestamp shape, on the server's clock: trustpub
// exchange tokens routinely expire un-revoked, and a leftover one must
// not read as a live publisher and block the wipe's evidence gate.
const PUBLISHERS: &str = "SELECT COUNT(*) AS n FROM tokens \
     WHERE scopes LIKE '%publish%' AND revoked_at IS NULL \
     AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))";

/// The governor's usage snapshot.  The audit deserializes the same
/// endpoint's answer, so the shape lives here once.
#[derive(Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) storage: Vec<StorageRow>,
    ops: Vec<OpRow>,
}

#[derive(Deserialize)]
pub(crate) struct StorageRow {
    pub(crate) pool: String,
    state: String,
    pub(crate) bytes: u64,
    pub(crate) objects: u64,
}

#[derive(Deserialize)]
struct OpRow {
    pool: String,
    window: String,
    used: u64,
}

/// The reconcile report.
#[derive(Deserialize)]
struct Report {
    added: Vec<String>,
    unreferenced: Vec<String>,
    mismatched: Vec<String>,
}

/// One page of the R2 REST listing.  `key` is required: a row that
/// does not carry one is not an answer about this key, and reading it
/// as absence is what the shell did by accident.
#[derive(Deserialize)]
struct Page {
    success: bool,
    result: Vec<Object>,
}

#[derive(Deserialize)]
struct Object {
    key: String,
}

/// Which pool a release names, and the bucket its evidence comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Primary,
    Backup,
    Dump,
}

impl Pool {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "primary" => Some(Self::Primary),
            "backup" => Some(Self::Backup),
            "dump" => Some(Self::Dump),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Backup => "backup",
            Self::Dump => "dump",
        }
    }

    fn bucket(self) -> &'static str {
        match self {
            Self::Primary => PRIMARY,
            Self::Backup | Self::Dump => BACKUP,
        }
    }

    /// The key grammar, which bounds how many objects can share the
    /// key as a prefix and is therefore what makes a single listing
    /// page sufficient evidence.
    fn admits(self, key: &str) -> bool {
        match self {
            // Fixed length, so no valid key extends another.
            Self::Primary | Self::Backup => key.strip_prefix("blobs/sha256/").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }),
            // At most the dump and its sidecar.
            Self::Dump => is_dump_key(key),
        }
    }

    fn shape(self) -> &'static str {
        match self {
            Self::Primary => "a primary key looks like blobs/sha256/<64 hex>",
            Self::Backup => "a backup key looks like blobs/sha256/<64 hex>",
            Self::Dump => "a dump key looks like d1/<YYYY-MM-DD>.sql[.sha256]",
        }
    }
}

/// `^d1/[0-9]{4}-[0-9]{2}-[0-9]{2}\.sql(\.sha256)?$`, hand-rolled.
fn is_dump_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("d1/") else {
        return false;
    };
    let rest = rest.strip_suffix(".sha256").unwrap_or(rest);
    let Some(date) = rest.strip_suffix(".sql") else {
        return false;
    };
    let parts: Vec<&str> = date.split('-').collect();
    matches!(parts.as_slice(), [year, month, day]
        if year.len() == 4 && month.len() == 2 && day.len() == 2
            && [year, month, day]
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_digit())))
}

/// Runs a governor action.
///
/// # Errors
///
/// If the endpoint refuses, if any evidence check cannot be proven, or
/// if the action itself fails.
pub fn run(action: &Action) -> Result<()> {
    let api = Api::new()?;
    match action {
        Action::Usage => usage(&api),
        Action::Compare => compare(&api),
        Action::Reconcile { keys } => reconcile(&api, *keys),
        Action::Release { pool, key } => release(&api, *pool, key),
        Action::Wipe => wipe(&api),
    }
}

/// What the CLI asked for.
pub enum Action {
    Usage,
    Compare,
    Reconcile { keys: bool },
    Release { pool: Pool, key: String },
    Wipe,
}

fn origin() -> Result<String> {
    let origin = std::env::var(cabin_env::CABIN_API_ORIGIN)
        .ok()
        .filter(|origin| !origin.is_empty())
        .unwrap_or_else(|| "https://cabinpkg.com".to_owned());
    if !origin.starts_with("https://") {
        bail!("{} must be https", cabin_env::CABIN_API_ORIGIN);
    }
    Ok(origin)
}

/// The admin governor endpoint.
pub(crate) struct Api {
    endpoint: String,
    token: String,
}

impl Api {
    /// # Errors
    ///
    /// If `CABIN_API_ORIGIN` is not https, or the registry token is
    /// missing.
    pub(crate) fn new() -> Result<Self> {
        let token = std::env::var(cabin_env::CABIN_REGISTRY_TOKEN).unwrap_or_default();
        if token.is_empty() {
            bail!(
                "{} is required: mint a login-session token and export it \
                 (registry/docs/runbook.md)",
                cabin_env::CABIN_REGISTRY_TOKEN
            );
        }
        Ok(Self {
            endpoint: format!("{}/api/v1/admin/governor", origin()?),
            token,
        })
    }

    /// The shell's `api`: the status is the verdict and the body is
    /// read for its message either way, so a refusal is not raised.
    fn call(&self, body: Option<&str>) -> Result<(u16, String)> {
        let agent = crate::audit::agent();
        let request = match body {
            Some(_) => agent
                .post(&self.endpoint)
                .set("Content-Type", "application/json"),
            None => agent.get(&self.endpoint),
        }
        .set("Authorization", &format!("Bearer {}", self.token));
        let response = match body {
            Some(body) => request.send_string(body),
            None => request.call(),
        };
        match response {
            Ok(response) => {
                let status = response.status();
                // The status is the verdict and the body is whatever
                // arrived, exactly as `-w '%{http_code}'` and
                // `-o "$body"` split them: curl left the partial file
                // in place and the callers read it. Keeping the prefix
                // matters twice over - a body that fails to arrive
                // after a mutation is already committed must not abort
                // before the post-mutation R2 check that repairs a
                // reappearing object, and a refusal whose message is
                // cut short is still worth printing.
                let mut body = Vec::new();
                let _ = response.into_reader().read_to_end(&mut body);
                Ok((status, String::from_utf8_lossy(&body).into_owned()))
            }
            Err(ureq::Error::Status(status, response)) => {
                Ok((status, response.into_string().unwrap_or_default()))
            }
            Err(error) => Err(error).context("the governor endpoint request failed"),
        }
    }

    /// `require_api_ok`: anything but 200 ends the run, naming what
    /// was asked for and what came back.
    pub(crate) fn ok(&self, body: Option<&str>, what: &str) -> Result<String> {
        let (status, answer) = self.call(body)?;
        if status != 200 {
            bail!("{what} answered {status}: {answer}");
        }
        Ok(answer)
    }
}

fn usage(api: &Api) -> Result<()> {
    step(&format!("governor usage snapshot ({})", api.endpoint));
    for line in snapshot_lines(api)? {
        println!("{line}");
    }
    Ok(())
}

/// The snapshot as the lines `usage` prints, without its heading.
///
/// The diagnostics bundle shells this in under a heading of its own
/// and indents it, which is why the rendering is separate from the
/// printing.
///
/// # Errors
///
/// If the endpoint refuses or answers something that is not a
/// snapshot.
pub fn usage_lines() -> Result<Vec<String>> {
    snapshot_lines(&Api::new()?)
}

fn snapshot_lines(api: &Api) -> Result<Vec<String>> {
    let snapshot: Snapshot = serde_json::from_str(&api.ok(None, "the usage snapshot")?)
        .context("parse the usage snapshot")?;
    Ok(render_snapshot(&snapshot))
}

fn render_snapshot(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = vec!["storage (bytes are the ledger, an upper bound of R2):".to_owned()];
    lines.extend(snapshot.storage.iter().map(|row| {
        format!(
            "    {}/{}: {} B in {} object(s)",
            row.pool, row.state, row.bytes, row.objects
        )
    }));
    if snapshot.storage.is_empty() {
        lines.push("    (empty)".to_owned());
    }
    lines.push("ops (used of the UTC-month window):".to_owned());
    lines.extend(
        snapshot
            .ops
            .iter()
            .map(|row| format!("    {}[{}]: {}", row.pool, row.window, row.used)),
    );
    if snapshot.ops.is_empty() {
        lines.push("    (no window opened yet)".to_owned());
    }
    lines
}

/// Totals only, deliberately: the snapshot is aggregate, and the
/// key-level divergence list is `reconcile`'s report.
fn compare(api: &Api) -> Result<()> {
    step("governor usage snapshot");
    let snapshot: Snapshot = serde_json::from_str(&api.ok(None, "the usage snapshot")?)
        .context("parse the usage snapshot")?;

    step("D1's authoritative view (live and verified blob totals)");
    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        TOTALS,
    ]))
    .context("the D1 totals query failed")?;
    let rows = results(&answer).context("the D1 totals query failed")?;
    let row = rows
        .first()
        .context("the D1 totals query answered no row")?;
    let total = |column: &str| -> Result<u64> {
        row.get(column)
            .and_then(serde_json::Value::as_u64)
            .with_context(|| format!("the D1 totals query answered no {column}"))
    };
    let (live_objects, live_bytes) = (total("live_objects")?, total("live_bytes")?);
    let (verified_objects, verified_bytes) = (total("verified_objects")?, total("verified_bytes")?);

    let pool = |name: &str| -> (u64, u64) {
        snapshot
            .storage
            .iter()
            .filter(|row| row.pool == name)
            .fold((0, 0), |(bytes, objects), row| {
                (bytes + row.bytes, objects + row.objects)
            })
    };
    let (primary_bytes, primary_objects) = pool("primary");
    let (backup_bytes, backup_objects) = pool("backup");
    let (dump_bytes, dump_objects) = pool("dump");

    println!("primary ledger: {primary_bytes} B in {primary_objects} object(s)");
    println!("D1 live view:   {live_bytes} B in {live_objects} blob(s)");
    if primary_bytes < live_bytes || primary_objects < live_objects {
        println!("    ledger understates D1: run cargo registry-governor reconcile");
    }
    if primary_objects > live_objects {
        println!(
            "    ledger holds entries D1 does not prove live: candidate orphans, or a\n    \
             pre-wipe ledger if a registry wipe just ran (not proof by itself - an\n    \
             empty registry looks the same); cargo registry-governor reconcile lists keys"
        );
    }
    println!("backup ledger:  {backup_bytes} B in {backup_objects} object(s)");
    println!("D1 verified:    {verified_bytes} B in {verified_objects} blob(s)");
    println!(
        "    (the backup pool may legitimately exceed the verified view: the BACKUP\n    \
         bucket is append-only and keeps history; cargo registry-backup-audit,\n    \
         from the repository root, audits it)"
    );
    println!("dump ledger:    {dump_bytes} B in {dump_objects} object(s)");
    println!(
        "    (audit the d1/ prefix against this with cargo registry-backup-audit,\n    \
         run from the repository root)"
    );
    Ok(())
}

fn reconcile(api: &Api, keys: bool) -> Result<()> {
    step("on-demand increase-only reconcile (primary pool from D1)");
    let report: Report =
        serde_json::from_str(&api.ok(Some(r#"{"reconcile":true}"#), "the reconcile")?)
            .context("parse the reconcile report")?;
    for line in render_report(&report, keys) {
        println!("{line}");
    }
    println!(
        "reconcile OK (operation windows, backup, and dump accounting are\n\
         not touched; docs/runbook.md, \"Known ceilings\")"
    );
    Ok(())
}

fn render_report(report: &Report, keys: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut show = |label: &str, list: &[String]| {
        lines.push(format!("    {label}: {}", list.len()));
        if keys {
            lines.extend(list.iter().map(|key| format!("        {key}")));
        }
    };
    show(
        "added (previously unledgered, now committed)",
        &report.added,
    );
    show(
        "unreferenced (candidate orphans; release needs evidence)",
        &report.unreferenced,
    );
    show(
        "mismatched (ledger kept the larger byte count)",
        &report.mismatched,
    );
    lines
}

/// The live service mode, for the write-coordination gates.
fn service_mode() -> Result<String> {
    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        SERVICE_MODE,
    ]))
    .context("could not read meta.service_mode")?;
    let rows = results(&answer).context("could not read meta.service_mode")?;
    Ok(rendered_mode(&rows))
}

/// `results.length === 0 ? "__MISSING__" : String(results[0].value)`.
fn rendered_mode(rows: &[serde_json::Map<String, serde_json::Value>]) -> String {
    let Some(row) = rows.first() else {
        return "__MISSING__".to_owned();
    };
    match row.get("value") {
        Some(value) => display(value),
        None => "undefined".to_owned(),
    }
}

/// Writes must be coordinated, or a publisher can land a put after the
/// evidence and before the release.
fn require_coordination(reverted: bool) -> Result<()> {
    let mode = service_mode()?;
    if COORDINATED.contains(&mode.as_str()) {
        return Ok(());
    }
    if reverted {
        bail!(
            "service_mode reverted to '{mode}' during the evidence checks (the breaker cron \
             restores it); re-apply the override and re-run"
        );
    }
    bail!(
        "service_mode is '{mode}'; block writes first, wait out the in-flight window, then \
         release (docs/runbook.md, \"The cost governor\")"
    );
}

fn release(api: &Api, pool: Pool, key: &str) -> Result<()> {
    if !pool.admits(key) {
        bail!("{}", pool.shape());
    }
    let bucket = pool.bucket();

    if pool == Pool::Backup {
        // Deleting from the append-only BACKUP bucket is an incident
        // action, never routine maintenance - the extra confirmation
        // marks the boundary, and the evidence rule is the same.
        confirm(
            cabin_env::CABIN_GOVERNOR_RELEASE_BACKUP_YES,
            "Backup-pool accounting is append-only; releasing marks an incident, not \
             maintenance. Type \"release-backup\" to confirm: ",
            "release-backup",
        )?;
    }

    if pool == Pool::Primary {
        // Observation alone cannot close the race with an in-flight
        // publisher (reserve taken, R2 put not yet landed: the key
        // reads absent now and appears after the release - unledgered
        // spend reconciliation can never discover, because D1 never
        // references it either). Coordination does.
        step("evidence: writes are blocked (no publisher can race the release)");
        require_coordination(false)?;
    }

    step(&format!("evidence: {key} must be absent from {bucket}"));
    if key_exists(bucket, key)? {
        bail!(
            "{key} still exists in {bucket}; a release for a live object would make the \
             ledger understate reality"
        );
    }

    if pool == Pool::Primary {
        step("evidence: no non-rejected D1 revision references the checksum");
        let hex = key
            .strip_prefix("blobs/sha256/")
            .context("a primary key carries its checksum")?;
        let refs = count(&format!(
            "
        SELECT COUNT(*) AS n FROM revisions
        WHERE verification != 'rejected'
          AND checksum = 'sha256:{hex}'"
        ))
        .context("the D1 reference check failed")?;
        if refs != 0 {
            bail!(
                "{refs} live D1 revision(s) still reference this checksum; reconciliation \
                 would re-add the entry"
            );
        }
        // The breaker cron overwrites a manual service-mode override
        // within 15 minutes; re-check immediately before the release
        // so the coordination cannot have silently lapsed during the
        // evidence steps above.
        require_coordination(true)?;
    }

    step(&format!("releasing {} {key}", pool.as_str()));
    let body = serde_json::json!({ "release": { "pool": pool.as_str(), "key": key } });
    api.ok(Some(&body.to_string()), "the release")?;

    // The evidence checks race a concurrent same-checksum write by
    // nature (the endpoint cannot inspect R2 atomically), so the window
    // is closed from the other side: re-check, and if the key came back
    // the ledger is repaired immediately instead of waiting for a cron.
    step("post-release verification: the key is still absent");
    if key_exists(bucket, key)? {
        eprintln!("WARNING: {key} reappeared in {bucket} inside the release window.");
        match pool {
            Pool::Primary => {
                step("repairing: reconcile re-adds every D1-referenced object now");
                api.ok(Some(r#"{"reconcile":true}"#), "the repair reconcile")?;
                eprintln!("verify with cargo registry-governor compare before moving on");
            }
            Pool::Backup => eprintln!(
                "run cargo registry-backup-backfill from the repository root so the drain \
                 re-ledgers it"
            ),
            Pool::Dump => eprintln!(
                "the nightly dump job re-commits its objects; audit with \
                 cargo registry-backup-audit from the repository root"
            ),
        }
        bail!("the release window closed over a reappearing object");
    }
    println!("release OK");
    Ok(())
}

fn wipe(api: &Api) -> Result<()> {
    // Mirrors the registry wipe: confirmation first, the guard
    // immediately before the destructive call so a flag flipped while
    // the prompt sat waiting still refuses.
    confirm(
        cabin_env::CABIN_GOVERNOR_WIPE_YES,
        "About to WIPE the governor ledger's primary rows (pre-launch only). Type \
         \"governor-wipe\" to confirm: ",
        "governor-wipe",
    )?;

    step("launch guard");
    crate::launch_guard::run(crate::launch_guard::Mode::Remote)?;

    // A delayed publisher (request in flight since before the registry
    // wipe) could still land a put after the emptiness check below.
    // The freshly-wiped database normally holds no publish-capable
    // token at all - proving that (or blocked writes) closes the race.
    // Revoked tokens do not count: the auth lookup refuses them, so
    // they cannot admit a publisher.
    step("evidence: no publish-capable token exists, or writes are blocked");
    let publishers = count(PUBLISHERS).context("could not count publish-capable tokens")?;
    if publishers != 0 {
        let mode = service_mode()?;
        if !COORDINATED.contains(&mode.as_str()) {
            bail!(
                "{publishers} publish-capable token(s) exist and service_mode is '{mode}'; \
                 block writes first (docs/runbook.md, \"Budget breaker and service mode\")"
            );
        }
    }

    // The ledger wipe is the registry wipe's step 7: with primary blobs
    // still present, wiping the ledger would undercount objects that
    // keep billing (reconciliation cannot see them - the wiped D1 no
    // longer references them).
    step(&format!("evidence: {PRIMARY} carries no blobs/ objects"));
    if prefix_nonempty(PRIMARY, "blobs/")? {
        bail!("{PRIMARY} still holds blobs/ objects; finish the registry wipe first");
    }
    if publishers != 0 {
        // The breaker cron can restore the mode during the R2 checks
        // above; re-check immediately before the wipe posts, exactly
        // like the release path.
        require_coordination(true)?;
    }

    step("wiping the governor's primary rows and fairness windows");
    api.ok(Some(r#"{"wipe":true}"#), "the ledger wipe")?;

    // The emptiness check races a concurrent writer by nature; re-check
    // after the wipe so an interleaved write cannot leave the fresh
    // ledger silently undercounting.
    step(&format!(
        "post-wipe verification: {PRIMARY} still carries no blobs/ objects"
    ));
    if prefix_nonempty(PRIMARY, "blobs/")? {
        eprintln!("WARNING: blobs/ objects appeared in {PRIMARY} during the wipe window;");
        eprintln!(
            "finish the registry wipe, investigate the writer, and re-run \
             cargo registry-governor wipe"
        );
        bail!("the wipe window closed over an appearing object");
    }
    println!(
        "governor wipe OK (backup, dump, and the monthly op windows survive\n\
         on purpose; docs/runbook.md, \"The cost governor\")"
    );
    Ok(())
}

/// An interactive confirmation, skipped when its escape hatch is set
/// to exactly `1`.
fn confirm(variable: &str, prompt: &str, expected: &str) -> Result<()> {
    if std::env::var(variable).as_deref() == Ok("1") {
        return Ok(());
    }
    print!("{prompt}");
    std::io::stdout().flush().context("write the prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read the confirmation")?;
    // `read -r answer` keeps everything but the newline.
    if answer.trim_end_matches(['\n', '\r']) != expected {
        bail!("not confirmed");
    }
    Ok(())
}

/// One `COUNT(*) AS n` from D1.
fn count(sql: &str) -> Result<u64> {
    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        sql,
    ]))?;
    let rows = results(&answer)?;
    rows.first()
        .and_then(|row| row.get("n"))
        .and_then(serde_json::Value::as_u64)
        .context("the count query answered no n")
}

/// One page of a bucket listing under `prefix`.
///
/// The token is required, not optional: the callers are evidence
/// gates, and a check that did not run is not evidence.
fn list_page(bucket: &str, prefix: &str) -> Result<Page> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CLOUDFLARE_API_TOKEN is required for the R2 evidence check");
    }
    if prefix.is_empty() {
        bail!("an empty prefix would list the whole bucket and prove nothing");
    }
    let account = account_id()?;
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account}/r2/buckets/{bucket}/objects\
         ?prefix={}&per_page=5",
        encode_prefix(prefix)
    );
    let body = crate::audit::get(&crate::audit::agent(), &url, &token)?;
    let page: Page = serde_json::from_str(&body)
        .with_context(|| format!("unexpected R2 list response: {body}"))?;
    if !page.success {
        bail!("unexpected R2 list response: {body}");
    }
    Ok(page)
}

/// The shell's
/// `prefix.split("/").map(encodeURIComponent).join("/")`: the
/// separators stay separators and everything else is escaped.
pub(crate) fn encode_prefix(prefix: &str) -> String {
    prefix
        .split('/')
        .map(crate::audit::encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether the exact key exists.  `Ok(false)` is proven absence; every
/// failure is an error, because absence inferred from a failed check
/// is what lets a release understate the ledger.
fn key_exists(bucket: &str, key: &str) -> Result<bool> {
    let page = list_page(bucket, key).context("cannot prove absence")?;
    Ok(page.result.iter().any(|object| object.key == key))
}

/// Whether at least one object lives under the prefix.
fn prefix_nonempty(bucket: &str, prefix: &str) -> Result<bool> {
    let page = list_page(bucket, prefix).context("cannot prove emptiness")?;
    Ok(!page.result.is_empty())
}

#[cfg(test)]
mod tests {
    //! Every expectation below was taken from the shell the port
    //! replaces, run over the same fixtures.  They live here because
    //! none of what they exercise - the key grammars that bound the
    //! evidence, the prefix encoder, the two renderers - is this
    //! crate's API.

    use super::*;

    /// The grammars are part of the evidence guard, not input
    /// hygiene: they bound how many objects can share the key as a
    /// prefix, which is what makes one `per_page=5` listing proof.
    #[test]
    fn the_key_grammars_bound_the_evidence() {
        let hex = "0123456789abcdef".repeat(4);
        for pool in [Pool::Primary, Pool::Backup] {
            assert!(pool.admits(&format!("blobs/sha256/{hex}")));
            for refused in [
                format!("blobs/sha256/{}", hex.to_uppercase()),
                format!("blobs/sha256/{}", &hex[..63]),
                format!("blobs/sha256/{hex}x"),
                "blobs/sha256/".to_owned(),
                "d1/2026-08-04.sql".to_owned(),
                String::new(),
            ] {
                assert!(!pool.admits(&refused), "accepted {refused:?}");
            }
        }
        assert!(Pool::Dump.admits("d1/2026-08-04.sql"));
        // The sidecar is admitted here, unlike the restore drill's
        // near-identical grammar, which deliberately refuses it.
        assert!(Pool::Dump.admits("d1/2026-08-04.sql.sha256"));
        for refused in [
            "d1/2026-8-4.sql",
            "d1/2026-08-04.SQL",
            "d1/2026-08-04.sql.sha256.sha256",
            "blobs/sha256/x",
            "",
        ] {
            assert!(!Pool::Dump.admits(refused), "accepted {refused:?}");
        }
    }

    /// `prefix.split("/").map(encodeURIComponent).join("/")`: the
    /// separators stay separators, everything else is escaped.  The
    /// shell built this with a command substitution that expanded to
    /// the empty string when `node` died, listing the whole bucket and
    /// proving absence of everything; here it cannot fail.
    #[test]
    fn the_prefix_encoder_keeps_separators() {
        assert_eq!(encode_prefix("blobs/sha256/abc"), "blobs/sha256/abc");
        assert_eq!(encode_prefix("blobs/"), "blobs/");
        assert_eq!(encode_prefix("a b/c+d"), "a%20b/c%2Bd");
        assert_eq!(encode_prefix("a/b?c=d&e"), "a/b%3Fc%3Dd%26e");
        assert_eq!(encode_prefix("x/-_.!~*\'()"), "x/-_.!~*\'()");
    }

    #[test]
    fn the_report_counts_and_optionally_lists() {
        let report: Report = serde_json::from_value(serde_json::json!({
            "added": ["blobs/sha256/aa"],
            "unreferenced": ["blobs/sha256/bb", "blobs/sha256/cc"],
            "mismatched": [],
        }))
        .unwrap();
        assert_eq!(
            render_report(&report, false),
            [
                "    added (previously unledgered, now committed): 1",
                "    unreferenced (candidate orphans; release needs evidence): 2",
                "    mismatched (ledger kept the larger byte count): 0",
            ]
        );
        assert_eq!(
            render_report(&report, true),
            [
                "    added (previously unledgered, now committed): 1",
                "        blobs/sha256/aa",
                "    unreferenced (candidate orphans; release needs evidence): 2",
                "        blobs/sha256/bb",
                "        blobs/sha256/cc",
                "    mismatched (ledger kept the larger byte count): 0",
            ]
        );
    }

    /// `results.length === 0 ? "__MISSING__" : String(results[0].value)`.
    /// The sentinel matters: it must not compare equal to a coordinated
    /// mode, and it must not be mistaken for an empty answer.
    #[test]
    fn the_service_mode_reads_as_the_shell_read_it() {
        let rows = |json: &str| results(json).unwrap();
        assert_eq!(
            rendered_mode(&rows(
                r#"[{"results":[{"value":"writes_blocked"}],"success":true}]"#
            )),
            "writes_blocked"
        );
        assert_eq!(
            rendered_mode(&rows(r#"[{"results":[],"success":true}]"#)),
            "__MISSING__"
        );
        assert_eq!(
            rendered_mode(&rows(r#"[{"results":[{"value":null}],"success":true}]"#)),
            "null"
        );
        assert_eq!(
            rendered_mode(&rows(r#"[{"results":[{"other":1}],"success":true}]"#)),
            "undefined"
        );
        for mode in ["__MISSING__", "null", "undefined", "normal", ""] {
            assert!(!COORDINATED.contains(&mode), "{mode} passed as coordinated");
        }
    }

    /// A listing row that is not an object with a `key` ends the run.
    /// The shell read exactly this shape as proven absence, because
    /// `node` exits 1 on an uncaught `TypeError` just as it did for
    /// "affirmatively absent".
    #[test]
    fn a_listing_row_without_a_key_is_not_absence() {
        let page = |body: &str| serde_json::from_str::<Page>(body);
        assert!(page(r#"{"success":true,"result":[{"key":"a"}]}"#).is_ok());
        assert!(page(r#"{"success":true,"result":[]}"#).is_ok());
        for refused in [
            r#"{"success":true,"result":[null]}"#,
            r#"{"success":true,"result":[{"size":1}]}"#,
            r#"{"success":true,"result":["a"]}"#,
            r#"{"success":true}"#,
        ] {
            assert!(page(refused).is_err(), "accepted {refused}");
        }
    }
}
