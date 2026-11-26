//! The run's tail: the breaker cron leg, the alert mail legs, the admin
//! cron endpoint, the download-log pass, the strict zip-container profile, the governor's hard limits across two mid-run
//! dev-server restarts, the admin governor endpoint, the real
//! verification driver's pass over two fresh versions (`driver_pass`),
//! the bounded-concurrency leg, and the admission leg, after which the
//! run closes with `smoke OK`.
//!
//! The concurrency leg runs its waves on threads released by a
//! [`Barrier`], not as `xtask_ci::spawn_tracked` children: that table is
//! for children the teardown must kill, it holds eight, and wave two
//! alone wants ten.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use xtask_registry_admin::display;
use xtask_registry_admin::verify::{Endpoints, run_with};

use crate::bytes::{frame, replace_all, retarget_hash, revision_of, sha256_hex};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::uniform_401_with;
use crate::legs::session;
use crate::servers::{DevServers, d1, d1_quiet, d1_rows, stored_upload};
use crate::step;
use crate::text::{capture, contains, grep_anywhere, grep_lines, read, text, write};

const POLLS: u32 = 20;
const HALF_SECOND: Duration = Duration::from_millis(500);

/// The cron expression `wrangler dev --test-scheduled` routes.
/// The `+` are the expression's spaces and must reach the query string
/// as `+`: percent-encoding them to `%2B` names a different cron.
pub(crate) const BREAKER_CRON: &str = "/__scheduled?cron=*/15+*+*+*+*";

const PENDING_LISTING: &str = "/api/v1/admin/versions?status=pending";
const GOVERNOR: &str = "/api/v1/admin/governor";
const CRON_STATE: &str = "/api/v1/admin/cron";

/// The line a driver pass prints when the cron's state is in order.
const CRON_IN_ORDER: &str = "cron: the alert pass is recent and no mail is stuck";

/// Every kind of cron trouble at once, for the driver to name: a pass
/// that last recorded a run long ago, an alert whose mail keeps
/// failing, and an account's mail that four tries have not sent and
/// that has been due since, with no mail sent in that time. No cron
/// pass runs while they stand.
const CRON_TROUBLE: &str = "
  UPDATE meta SET value = '2026-01-01T00:00:00.000Z'
    WHERE key IN ('alerts_checked_at', 'notifications_sent_at');
  UPDATE alerts SET attempts = 4 WHERE kind = 'service_mode';
  INSERT INTO notifications (kind, event, recipient, detail, created_at, attempts, attempted_at)
    SELECT 'account_created', '/!cron', id, '{}', '2026-01-01T00:00:00.000Z', 4,
           '2026-01-01T00:00:00.000Z'
    FROM users WHERE notification_email IS NOT NULL LIMIT 1;";

/// Undoes [`CRON_TROUBLE`]: the passes are recent again, as the cron's
/// clock writes them.
const CRON_TROUBLE_OVER: &str = "
  UPDATE meta SET value = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE key IN ('alerts_checked_at', 'notifications_sent_at');
  UPDATE alerts SET attempts = 0 WHERE kind = 'service_mode';
  DELETE FROM notifications WHERE event = '/!cron';";

/// The publish burst (bucket size 5) reset before each leg that
/// publishes, so a rate limit can never stand in for the refusal a leg
/// is actually asserting.
const BURST_RESET: &str = "
  UPDATE users SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = (SELECT user_id FROM tokens WHERE id = 'smoke');";

/// The frozen fixture's own version, which the metadata names in
/// several places and every derived fixture textually replaces.
const FIXTURE_VERSION: &str = "0.2.0";

const PROFILE_VERSION: &str = "0.3.0";
const ISO_PENDING_VERSION: &str = "0.4.0";
const ISO_VERIFIED_VERSION: &str = "0.5.0";
const BLOCKED_VERSION: &str = "0.6.0";

/// The two versions the verification driver judges: a clean repack of
/// the fixture, and a traversal archive.
const DRIVER_VERIFIED_VERSION: &str = "0.12.0";
const DRIVER_REJECTED_VERSION: &str = "0.13.0";

/// The fixture's `cabin.toml` (`cargo gen-fixtures`, `withdep`) and
/// source, which the stored repack carries at another version.
const WITHDEP_MANIFEST: &str = r#"[package]
name = "smoke/withdep"
version = "0.2.0"
license = "MIT OR Apache-2.0"
cxx-standard = "c++20"

[dependencies]
"smoke/nodep" = "^0.1"

[target.withdep]
type = "library"
sources = ["src/withdep.cc"]
interface-cxx-standard = "c++17"
links = "withdep-native"
"#;
const WITHDEP_SOURCE: &[u8] = b"void withdep() {}\n";

/// The hidden first argument that makes the smoke binary the
/// verification driver `driver_pass` spawns: the driver's aborts
/// exit the process, so it cannot run inside this one.
pub const DRIVER: &str = "__verify-driver";

/// The bearer the GitHub mock's Actions mint requires
/// (`assets/github-mock.js`).
const ACTIONS_REQUEST_TOKEN: &str = "smoke-actions-request";

/// Five more verified versions with distinct bytes, never downloaded,
/// so the concurrency leg has distinct uncached artifacts to charge.
const LOAD_VERSIONS: [&str; 5] = ["0.7.0", "0.8.0", "0.9.0", "0.10.0", "0.11.0"];

/// The ordinary-read operations the load leg leaves the pool.
const HEADROOM: i64 = 3;

/// What the earlier phases built that this one reads: the run's
/// fixtures, the two paths its own publishes reuse, and the credentials
/// the load threads carry.  Everything is borrowed - the tail owns
/// nothing the run started with.
pub struct FinaleInputs<'a> {
    /// The three children, for the two mid-run restarts and for the
    /// `.dev.vars` mutations between them.
    pub servers: &'a mut DevServers,
    /// The run's scratch directory.
    pub work: &'a Path,
    /// The verifier binary, next to this tool's own binary.
    pub verifier_bin: &'a Path,
    /// The fixture's package.
    pub scope: &'a str,
    pub name: &'a str,
    /// The frozen fixture metadata document, verbatim.
    pub fixture_metadata: &'a [u8],
    /// The fixture archive's SHA-256, the digest every derived document
    /// is retargeted away from.
    pub blob_hash: &'a str,
    /// The main fixture's artifact route, whose cached copy must keep
    /// serving under an exhausted pool.
    pub artifact_path: &'a str,
    /// The original publish's path and body, replayed as an idempotent
    /// no-op.
    pub publish_path: &'a str,
    pub publish_body: &'a [u8],
    /// The minted session cookie, for the source-viewer read.
    pub session_cookie: &'a str,
    /// The publisher's bearer token, which the load threads
    /// carry directly (they do not go through [`Smoke`]).
    pub token: &'a str,
}

/// The zips this phase publishes and later downloads: their bytes are
/// what the artifact routes are derived from, so they are kept rather
/// than re-read.
struct Fixtures {
    iso_pending: Vec<u8>,
    iso_verified: Vec<u8>,
    load: Vec<(&'static str, Arc<Vec<u8>>)>,
}

/// The run's tail.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, inputs: &mut FinaleInputs<'_>) -> Result<()> {
    breaker_cron(smoke, inputs)?;
    alert_mail(smoke)?;
    allowance_alert(smoke)?;
    admin_cron(smoke)?;
    upload_limits(smoke, inputs)?;
    crate::legs::download_logs::run(smoke, inputs.scope, inputs.name, inputs.work)?;
    zip_profile(smoke, inputs)?;
    driver_pass(smoke, inputs)?;
    let fixtures = governor_fixtures(smoke, inputs)?;
    tiny_pools(smoke, inputs, &fixtures)?;
    admin_governor(smoke)?;
    concurrency(smoke, inputs, &fixtures)?;
    crate::legs::notifications::nothing_else_is_owed(inputs.scope)?;
    // Dead last on purpose: the exhaustion burst empties the shared
    // local admission bucket (crate::legs::admission), and any OIDC
    // request after it would answer the 429 for up to a minute.
    crate::legs::admission::run(smoke)?;
    println!("smoke OK");
    Ok(())
}

/// The `/__scheduled` route invokes the cron handler; the breaker
/// expression runs the budget evaluation and the upload lifecycle pass,
/// and the usage log line proves the governor answered.  The wait is a
/// line watermark over a log a live child is still appending to: an
/// earlier leg's pass logged the same line.
fn breaker_cron(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("the breaker cron pass reads the governor's usage");
    let mark = line_mark(inputs.servers.dev_log())?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !await_log(inputs.servers.dev_log(), mark, "governor usage:")? {
        bail!("the breaker cron pass never logged the governor usage snapshot");
    }
    Ok(())
}

/// The cron mails the operator when the service mode leaves normal,
/// and once when it returns. A row reads as mailed once the local
/// `NOTIFY_EMAIL` binding has accepted the pass's mail, which it does
/// from the binding's sender and no other, to the run's
/// `ALERT_RECIPIENT_EMAIL` (`.dev.vars`);
/// the binding keeps no copy to read, so what the mail says is
/// `alerts::digest`'s own test. Without the analytics token the cron
/// leaves a stored `writes_blocked` as it is, so the forced mode stays
/// for its pass.
fn alert_mail(smoke: &mut Smoke) -> Result<()> {
    step("the cron mails a blocked service mode, then its recovery");
    let mailed = |levels: &str| -> Result<bool> {
        let rows = crate::servers::d1_rows(&format!(
            "SELECT 1 FROM alerts WHERE kind = 'service_mode' AND attempts = 0 AND {levels}"
        ))?;
        Ok(!rows.is_empty())
    };
    d1("UPDATE meta SET value = 'writes_blocked' WHERE key = 'service_mode';")?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !mailed("level = 2 AND notified_level = 2")? {
        bail!("the cron did not mail the blocked service mode");
    }
    d1("UPDATE meta SET value = 'normal' WHERE key = 'service_mode';")?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !mailed("level = 0 AND notified_level = 0")? {
        bail!("the cron did not mail the service mode's recovery");
    }
    Ok(())
}

/// The cron mails each class's shared daily upload allowance at its
/// steps (`api/src/quota.rs`: 500 uploads for the default class, 1,000
/// for the operator's), and starts a kind over with its day: a row
/// firing since an earlier day is mailed as a first firing, and fires
/// since this pass. The uploads added here are taken back and the
/// alerts' rows removed, so the legs after this one find the allowances
/// and the alerts as the run left them.
fn allowance_alert(smoke: &mut Smoke) -> Result<()> {
    step("the cron mails a shared daily allowance at its steps and starts it over with its day");
    let add = |class: &str, uploads: u32| {
        d1(&format!(
            "INSERT INTO shared_daily_upload_usage (usage_date, quota_class, bytes, uploads) \
             VALUES (date('now'), '{class}', 0, {uploads}) \
             ON CONFLICT (usage_date, quota_class) DO UPDATE SET uploads = uploads + {uploads};"
        ))
    };
    let mailed = |kind: &str, state: &str| -> Result<bool> {
        let rows = crate::servers::d1_rows(&format!(
            "SELECT 1 FROM alerts WHERE kind = '{kind}' AND attempts = 0 AND {state}"
        ))?;
        Ok(!rows.is_empty())
    };
    add("default", 250)?;
    add("operator", 800)?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !mailed(
        "shared_uploads",
        "level = 1 AND notified_level = 1 AND instr(summary, 'has reached 50% (') > 0 \
         AND instr(summary, ' of 500 uploads, ') > 0",
    )? {
        bail!("the cron did not mail the default class's shared allowance at half");
    }
    if !mailed(
        "operator_uploads",
        "level = 2 AND notified_level = 2 AND instr(summary, 'has reached 80% (') > 0 \
         AND instr(summary, ' of 1000 uploads, ') > 0",
    )? {
        bail!("the cron did not mail the operator class's shared allowance at four fifths");
    }
    d1("UPDATE alerts SET since = '2026-01-01T00:00:00.000Z' WHERE kind = 'shared_uploads';")?;
    add("default", 250)?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !mailed(
        "shared_uploads",
        "level = 3 AND notified_level = 3 AND since > '2026-01-02' \
         AND instr(summary, 'is used up (') > 0",
    )? {
        bail!("the cron did not start the shared allowance's day over and mail its limit");
    }
    // Only a row every addition for its class went to holds that many.
    d1(
        "UPDATE shared_daily_upload_usage SET uploads = uploads - 500 \
         WHERE quota_class = 'default' AND uploads >= 500; \
         UPDATE shared_daily_upload_usage SET uploads = uploads - 800 \
         WHERE quota_class = 'operator' AND uploads >= 800; \
         DELETE FROM alerts WHERE kind IN ('shared_uploads', 'operator_uploads');",
    )
}

/// The versions the upload-limit leg publishes, and one that is only
/// ever refused.  The first is the day's thirtieth upload; the rest
/// fill the publisher's pending slots.
const LIMIT_VERSIONS: [&str; 3] = ["0.14.0", "0.15.0", "0.16.0"];
const REFUSED_VERSION: &str = "0.17.0";

/// Today's allowance rows of every user and class, in one value to
/// compare.
const DAY_ROWS: &str = "SELECT \
    (SELECT json_group_array(json_array(user_id, bytes, uploads)) \
     FROM user_daily_upload_usage WHERE usage_date = date('now')) AS user_rows, \
    (SELECT json_group_array(json_array(quota_class, bytes, uploads)) \
     FROM shared_daily_upload_usage WHERE usage_date = date('now')) AS shared_rows";

/// The upload allowances through the running Worker: each is seeded
/// at its boundary by SQL, so no large body is ever sent.  The day's
/// two allowance rows are put back as they were at the end, and so are
/// the mails the yank cycle owes, which `nothing_else_is_owed` counts.
#[allow(clippy::too_many_lines)] // one seeded sequence, a step per allowance
fn upload_limits(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    let one = |sql: &str| -> Result<Option<(u64, u64)>> {
        Ok(d1_rows(sql)?.first().map(|row| {
            (
                row["bytes"].as_u64().unwrap_or_default(),
                row["uploads"].as_u64().unwrap_or_default(),
            )
        }))
    };
    let user_today = "SELECT bytes, uploads FROM user_daily_upload_usage \
         WHERE usage_date = date('now') AND user_id = 2";
    let original_user = one(user_today)?;
    let original_shared = one("SELECT bytes, uploads FROM shared_daily_upload_usage \
         WHERE usage_date = date('now') AND quota_class = 'default'")?;
    // Each puts today's row of the seeded publisher, or of the default
    // class, as given; `None` leaves none.
    let user_row = |row: Option<(u64, u64)>| {
        let insert = row.map_or_else(String::new, |(bytes, uploads)| {
            format!(
                "INSERT INTO user_daily_upload_usage VALUES (date('now'), 2, {bytes}, {uploads});"
            )
        });
        format!(
            "DELETE FROM user_daily_upload_usage WHERE usage_date = date('now') AND user_id = 2; {insert}"
        )
    };
    let shared_row = |row: Option<(u64, u64)>| {
        let insert = row.map_or_else(String::new, |(bytes, uploads)| {
            format!("INSERT INTO shared_daily_upload_usage VALUES (date('now'), 'default', {bytes}, {uploads});")
        });
        format!(
            "DELETE FROM shared_daily_upload_usage \
             WHERE usage_date = date('now') AND quota_class = 'default'; {insert}"
        )
    };
    let pending_count = || -> Result<u64> {
        let rows = d1_rows(
            "SELECT COUNT(*) AS n FROM revisions WHERE published_by = 2 AND verification = 'pending'",
        )?;
        rows[0]["n"].as_u64().context("the pending count")
    };
    let held = pending_count()?;
    // Two free slots: with one, the bodyless preflight of the byte
    // step would answer for the pending limit first.
    if held > 1 {
        bail!(
            "the publisher already holds {held} pending uploads, the upload-limit leg needs two free slots"
        );
    }
    let publish = |smoke: &mut Smoke, version: &str, expected: u16| -> Result<()> {
        d1_quiet(BURST_RESET)?;
        let zip = min_zip(&format!("limit{version}"));
        let body = publish_payload(inputs, version, &zip);
        smoke.publish(&publish_route(inputs, version), &body, &[expected])?;
        Ok(())
    };

    step(
        "the per-user daily allowance counts an upload through either credential: the thirtieth lands on it, the next is refused",
    );
    // One upload left of 30: admitted through the trusted-publishing
    // token, refused through the session token of the same user.
    d1(&user_row(Some((1024, 29))))?;
    // Refused before admission: a malformed frame draws nothing.
    let rows = d1_rows(DAY_ROWS)?;
    d1_quiet(BURST_RESET)?;
    smoke.publish(
        &publish_route(inputs, REFUSED_VERSION),
        b"not a frame",
        &[400],
    )?;
    if d1_rows(DAY_ROWS)? != rows {
        bail!("a malformed publish changed an allowance row");
    }
    smoke.as_ci_publisher();
    publish(smoke, LIMIT_VERSIONS[0], 201)?;
    smoke.expect_body(r#""verification":"pending""#)?;
    smoke.as_publisher();
    let charged = one(user_today)?;
    if charged.map(|(_, uploads)| uploads) != Some(30) {
        bail!("the trusted-publishing upload was not charged to the user's row: {charged:?}");
    }
    let rows = d1_rows(DAY_ROWS)?;
    publish(smoke, REFUSED_VERSION, 429)?;
    daily_refusal(smoke, "user_count", 30, 30)?;
    if d1_rows(DAY_ROWS)? != rows {
        bail!("a daily-limit refusal changed an allowance row");
    }

    step("the per-user daily byte allowance refuses an archive that no longer fits");
    d1(&user_row(Some((256 * 1024 * 1024 - 1, 0))))?;
    let rows = d1_rows(DAY_ROWS)?;
    publish(smoke, REFUSED_VERSION, 429)?;
    daily_refusal(
        smoke,
        "user_bytes",
        256 * 1024 * 1024,
        256 * 1024 * 1024 - 1,
    )?;
    if d1_rows(DAY_ROWS)? != rows {
        bail!("a byte-limit refusal changed an allowance row");
    }

    step("the ordinary users' shared daily allowance refuses a new upload once spent");
    d1(&format!(
        "{} {}",
        user_row(None),
        shared_row(Some((0, 500)))
    ))?;
    let rows = d1_rows(DAY_ROWS)?;
    publish(smoke, REFUSED_VERSION, 429)?;
    daily_refusal(smoke, "shared_count", 500, 500)?;
    if d1_rows(DAY_ROWS)? != rows {
        bail!("a shared-limit refusal changed an allowance row");
    }

    step("the fourth unverified upload is refused with the pending code and no Retry-After");
    d1(&format!(
        "{} {}",
        user_row(None),
        shared_row(original_shared)
    ))?;
    let mut next = 1;
    while pending_count()? < 3 {
        publish(smoke, LIMIT_VERSIONS[next], 201)?;
        next += 1;
    }
    let rows = d1_rows(DAY_ROWS)?;
    publish(smoke, REFUSED_VERSION, 429)?;
    let error = limit_error(smoke, "pending_limit_exceeded")?;
    if error["limit"] != 3 || error["used"] != 3 || error.get("reset_at").is_some() {
        bail!("the pending refusal's fields are wrong: {error}");
    }
    if !grep_lines(&text(&smoke.headers), "retry-after:").is_empty() {
        bail!(
            "the pending refusal carries a Retry-After: {}",
            capture(&smoke.headers)
        );
    }
    if d1_rows(DAY_ROWS)? != rows {
        bail!("a pending-limit refusal changed an allowance row");
    }

    step("accepted work continues while the shared daily allowance refuses new uploads");
    d1(&shared_row(Some((0, 500))))?;
    let rows = d1_rows(DAY_ROWS)?;
    smoke.as_verifier();
    smoke.wcheck(PENDING_LISTING, &[200])?;
    let listing = smoke.body.clone();
    for version in &LIMIT_VERSIONS[..next] {
        verify_version(
            smoke,
            inputs,
            &listing,
            version,
            &format!("entry-limit-{version}.json"),
        )?;
    }
    smoke.as_publisher();
    let left = pending_count()?;
    if left != held {
        bail!("the verdicts left the publisher's pending uploads at {left}, expected {held}");
    }
    let version = LIMIT_VERSIONS[0];
    let zip = min_zip(&format!("limit{version}"));
    smoke.check(
        &format!("/packages/{}/{}.json", inputs.scope, inputs.name),
        &[200],
    )?;
    smoke.expect_body(&format!(r#""{version}":{{"#))?;
    smoke.check(&artifact_route(inputs, version, &zip), &[200])?;
    if smoke.body != zip {
        bail!("the accepted upload's download differs from what was published");
    }
    let yank = format!("{}/yank", publish_route(inputs, version));
    smoke.wrequest("PATCH", &yank, br#"{"yanked":true}"#, &[200])?;
    smoke.expect_body(r#""yanked":true"#)?;
    smoke.wrequest("PATCH", &yank, br#"{"yanked":false}"#, &[200])?;
    smoke.expect_body(r#""yanked":false"#)?;
    publish(smoke, REFUSED_VERSION, 429)?;
    daily_refusal(smoke, "shared_count", 500, 500)?;
    if d1_rows(DAY_ROWS)? != rows {
        bail!("accepted work, or the refusal after it, changed an allowance row");
    }

    // Put back what the leg seeded and the mails it caused.
    d1(&format!(
        "DELETE FROM notifications WHERE kind IN ('version_yanked', 'version_unyanked') \
           AND detail ->> '$.version' = '{version}'; {} {}",
        user_row(original_user),
        shared_row(original_shared),
    ))
}

/// A refusal's one error object, which must carry `code`.
fn limit_error(smoke: &Smoke, code: &str) -> Result<Value> {
    let body: Value = serde_json::from_slice(&smoke.body).context("parse the refusal")?;
    let error = body["errors"][0].clone();
    if error["code"] != code {
        bail!("the refusal's code is not {code}: {body}");
    }
    Ok(error)
}

/// A `daily_upload_limit_exceeded` refusal of `kind`, whose
/// `Retry-After` counts down to the `reset_at` it names, the next UTC
/// midnight.
fn daily_refusal(smoke: &Smoke, kind: &str, limit: u64, used: u64) -> Result<()> {
    // Read before the slow D1 call below, so the comparison with
    // `Retry-After` spans the request alone.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let error = limit_error(smoke, "daily_upload_limit_exceeded")?;
    if error["limit_kind"] != kind || error["limit"] != limit || error["used"] != used {
        bail!("the daily refusal's fields are not {kind} {used}/{limit}: {error}");
    }
    let midnight = d1_rows("SELECT date('now', '+1 day') || 'T00:00:00Z' AS at")?;
    if error["reset_at"] != midnight[0]["at"] {
        bail!(
            "reset_at is not the next UTC midnight {}: {error}",
            midnight[0]["at"]
        );
    }
    let block = text(&smoke.headers);
    let [line] = grep_lines(&block, "retry-after:")[..] else {
        bail!(
            "the daily refusal has no single Retry-After: {}",
            capture(&smoke.headers)
        );
    };
    let secs: u64 = line
        .split_once(':')
        .map_or("", |(_, value)| value.trim())
        .parse()
        .with_context(|| format!("Retry-After is not a number of seconds: {line}"))?;
    let until = 86_400 - now % 86_400;
    if secs.abs_diff(until) > 5 {
        bail!(
            "Retry-After {secs} is not the {until} seconds to {}",
            midnight[0]["at"]
        );
    }
    println!(
        "    {kind} {used}/{limit}: Retry-After {secs}s, reset_at {}",
        error["reset_at"]
    );
    Ok(())
}

/// The cron's state reads with the verify scope alone, and shows the
/// pass `allowance_alert` just ran: recent, no mail of its alerts failing,
/// and the time of a mail the run's passes sent.
fn admin_cron(smoke: &mut Smoke) -> Result<()> {
    step("the admin cron endpoint answers the pass that just ran, to the verify scope alone");
    smoke.as_ci_publisher();
    smoke.wcheck(CRON_STATE, &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.as_verifier();
    smoke.wcheck(CRON_STATE, &[200])?;
    let state: Value = serde_json::from_slice(&smoke.body).context("parse the cron's state")?;
    let read = |key| state.get(key).and_then(Value::as_i64);
    let in_order = read("alerts_checked_seconds").is_some_and(|seconds| (0..60).contains(&seconds))
        && read("alert_mail_attempts") == Some(0)
        && read("notification_sent_seconds").is_some_and(|seconds| seconds >= 0);
    if !in_order {
        bail!("the cron's state is not that of the pass that just ran: {state}");
    }
    smoke.as_publisher();
    Ok(())
}

/// The strict zip container profile, at publish and at verification.
/// Both publishes charge the publish bucket, so the leg opens with its
/// own burst.
fn zip_profile(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    d1(BURST_RESET)?;
    non_zip_body(smoke, inputs)?;
    traversal_archive(smoke, inputs)
}

/// Canonical metadata for a fresh version with an archive part that is
/// plainly not a zip: the fixed-offset container gate rejects it ahead
/// of the checksum and immutability checks.
fn non_zip_body(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("the publish path fast-fails a non-zip body before hashing it");
    let body = publish_payload(inputs, PROFILE_VERSION, b"not a zip archive");
    smoke.publish(&publish_route(inputs, PROFILE_VERSION), &body, &[400])?;
    smoke.expect_body("archive is not a zip container")
}

/// A single stored zero-length entry named `../evil`: the EOCD
/// arithmetic is exact, so it clears the worker's container gate, but
/// the strict profile fails it on path traversal.
fn traversal_archive(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("a profile-violating archive publishes pending, then the verifier rejects it");
    let evil = min_zip("../evil");
    let artifact = artifact_route(inputs, PROFILE_VERSION, &evil);
    let body = publish_payload(inputs, PROFILE_VERSION, &evil);
    smoke.publish(&publish_route(inputs, PROFILE_VERSION), &body, &[201])?;
    smoke.expect_body(r#""verification":"pending""#)?;

    smoke.as_verifier();
    smoke.wcheck(PENDING_LISTING, &[200])?;
    let pending = smoke.body.clone();
    let downloaded = download(smoke, &artifact)?;
    if downloaded != evil {
        bail!("the pending profile-violation download differs from what was published");
    }
    // The verifier binary opens its two inputs by path, so this leg's
    // files are the ones that really have to be written.
    let archive = inputs.work.join("evil-download.zip");
    write(&archive, &downloaded)?;
    let entry_path = inputs.work.join("entry-profile.json");
    session::listing_entry(&pending, &package(inputs), PROFILE_VERSION, &entry_path)?;
    let verdict_path = inputs.work.join("verdict-profile.json");
    session::run_verifier(inputs.verifier_bin, &archive, &entry_path, &verdict_path)?;
    let verdict = read(&verdict_path)?;
    let rendered = String::from_utf8_lossy(&verdict).into_owned();
    if !rendered.contains(r#""verdict":"rejected""#) {
        bail!("the verifier did not reject the traversal archive: {rendered}");
    }
    if !rendered.contains("path_traversal") {
        bail!("the rejection is not path_traversal: {rendered}");
    }
    // The reason comes from the binary; the checksum and published_at
    // bind the verdict to what the listing reported.
    let parsed: Value = serde_json::from_slice(&verdict).context("parse the verifier verdict")?;
    let entry = parse_entry(&entry_path)?;
    let mut patch = Map::new();
    patch.insert("verdict".to_owned(), Value::String("rejected".to_owned()));
    copy_field(&mut patch, "reason", parsed.pointer("/reasons/0"));
    copy_field(&mut patch, "checksum", entry.get("checksum"));
    copy_field(&mut patch, "published_at", entry.get("published_at"));
    let patch = json_bytes(&Value::Object(patch))?;
    smoke.verdict_patch(&verdict_route(inputs, PROFILE_VERSION), &patch, &[200])?;
    smoke.expect_body(r#""verification":"rejected""#)?;
    smoke.expect_body(r#""changed":true"#)?;
    smoke.as_publisher();
    smoke.check(&artifact, &[404])
}

/// The verification driver (`xtask_registry_admin::verify`) end to
/// end against this Worker, where the legs above hand-build their
/// verdicts: an inspector that cannot run leaves every pending version
/// pending and fails the pass, and a working one verifies the clean
/// repack and rejects the traversal archive.  Before the governor
/// fixtures, whose pending version must stay pending: the driver
/// judges every pending version it lists, the publish leg's twin
/// included.
fn driver_pass(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("the verification driver leaves everything pending when its inspector cannot run");
    d1_quiet(BURST_RESET)?;
    let manifest = WITHDEP_MANIFEST.replace(FIXTURE_VERSION, DRIVER_VERIFIED_VERSION);
    let clean = stored_zip(&[
        ("cabin.toml", manifest.as_bytes()),
        ("src/withdep.cc", WITHDEP_SOURCE),
    ]);
    let evil = min_zip("../driver");
    publish_min_version(smoke, inputs, DRIVER_VERIFIED_VERSION, &clean)?;
    publish_min_version(smoke, inputs, DRIVER_REJECTED_VERSION, &evil)?;
    let ours = [
        format!("{}@{DRIVER_VERIFIED_VERSION}", package(inputs)),
        format!("{}@{DRIVER_REJECTED_VERSION}", package(inputs)),
    ];
    smoke.as_verifier();
    let before = pending(smoke)?;
    if !ours.iter().all(|version| before.contains(version)) {
        bail!("the fresh versions are not pending: {before:?}");
    }

    let (passed, output) = spawn_driver(smoke, &inputs.work.join("no-such-inspector"))?;
    if passed || !output.contains("hit operational failures and stay pending") {
        bail!("the driver did not fail the pass on an inspector that cannot run");
    }
    if pending(smoke)? != before {
        bail!("an operational failure changed a verdict");
    }

    step(
        "the verification driver verifies the clean repack and rejects the traversal, and fails \
         its pass on a cron that stopped and mail that is stuck",
    );
    d1(CRON_TROUBLE)?;
    let (passed, output) = spawn_driver(smoke, inputs.verifier_bin)?;
    d1(CRON_TROUBLE_OVER)?;
    for trouble in [
        "cron: the alert pass last recorded a run ",
        "cron: an alert's mail has failed 4 passes running",
        "cron: a notification has waited ",
        "cron: a notification is unsent after 4 tries",
    ] {
        if passed || !output.contains(trouble) {
            bail!("the driver did not fail its pass on: {trouble}");
        }
    }
    let after = pending(smoke)?;
    if ours.iter().any(|version| after.contains(version)) {
        bail!("the driver left a fresh version pending: {after:?}");
    }
    // The publish leg's `with-dep` twin: its name advisory abstains.
    let twin = format!("{}/with-dep@{FIXTURE_VERSION}", inputs.scope);
    if !after.contains(&twin) {
        bail!("the driver judged {twin}, whose confusable name must leave it pending");
    }
    smoke.wcheck("/api/v1/admin/versions?status=rejected", &[200])?;
    smoke.expect_body(&format!(r#""version":"{DRIVER_REJECTED_VERSION}""#))?;
    smoke.as_ci_publisher();
    let served = artifact_route(inputs, DRIVER_VERIFIED_VERSION, &clean);
    smoke.check(&served, &[200])?;
    if smoke.body != clean {
        bail!("the verified repack serves other bytes than were published");
    }
    smoke.as_publisher();
    smoke.check(
        &artifact_route(inputs, DRIVER_REJECTED_VERSION, &evil),
        &[404],
    )?;

    step("the verification driver passes once the cron is in order");
    let (passed, output) = spawn_driver(smoke, inputs.verifier_bin)?;
    if !passed || !output.contains(CRON_IN_ORDER) {
        bail!("the verification driver failed its pass, or did not check the cron");
    }
    Ok(())
}

/// `name@version` of every pending version, sorted.
fn pending(smoke: &mut Smoke) -> Result<Vec<String>> {
    smoke.wcheck(PENDING_LISTING, &[200])?;
    let listing: Value =
        serde_json::from_slice(&smoke.body).context("parse the pending listing")?;
    let versions = listing
        .get("versions")
        .and_then(Value::as_array)
        .context("the pending listing has no versions")?;
    let mut pending: Vec<String> = versions
        .iter()
        .map(|entry| {
            let field = |key| entry.get(key).and_then(Value::as_str).unwrap_or_default();
            format!("{}@{}", field("name"), field("version"))
        })
        .collect();
    pending.sort();
    Ok(pending)
}

/// One driver pass in a child process: whether it succeeded, and its
/// output, which is also echoed into the run's log.
fn spawn_driver(smoke: &Smoke, inspector: &Path) -> Result<(bool, String)> {
    let output = Command::new(std::env::current_exe().context("locating the smoke binary")?)
        .arg(DRIVER)
        .arg(smoke.url(Base::Registry, ""))
        .arg(smoke.web_origin())
        .arg(smoke.url(Base::Web, ""))
        .arg(smoke.actions_mint_url())
        .arg(inspector)
        .output()
        .context("spawning the verification driver")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for line in text.lines() {
        println!("    driver: {line}");
    }
    Ok((output.status.success(), text))
}

/// The child `spawn_driver` starts:
/// `DRIVER <registry> <expected-api> <api> <mint-url> <inspector>`.
///
/// # Errors
///
/// A wrong argument count, or whatever the pass returns.
pub fn driver(args: &[String]) -> Result<()> {
    let [registry, expected_api, api, mint, inspector] = args else {
        bail!("{DRIVER} takes five arguments");
    };
    run_with(Endpoints {
        registry_origin: registry.clone(),
        expected_api_origin: expected_api.clone(),
        api_base: api.clone(),
        mint_url: mint.clone(),
        mint_token: ACTIONS_REQUEST_TOKEN.to_owned(),
        verifier: PathBuf::from(inspector),
    })
}

/// The isolation fixtures publish while the pools are still large: a
/// pending version (never verified, never downloaded) and a verified
/// one (never downloaded), each with distinct bytes, then five more for
/// the load leg.  Each of those is verified before the next is
/// published: the default class holds three pending uploads at a time
/// (`api/src/quota.rs`), and two are already held.
fn governor_fixtures(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<Fixtures> {
    step("seeding isolation fixtures for the governor legs");
    d1_quiet(BURST_RESET)?;
    let iso_pending = min_zip("isopending");
    let iso_verified = min_zip("isoverified");
    publish_min_version(smoke, inputs, ISO_PENDING_VERSION, &iso_pending)?;
    publish_min_version(smoke, inputs, ISO_VERIFIED_VERSION, &iso_verified)?;
    smoke.as_verifier();
    smoke.wcheck(PENDING_LISTING, &[200])?;
    let pending = smoke.body.clone();
    verify_version(
        smoke,
        inputs,
        &pending,
        ISO_VERIFIED_VERSION,
        "entry-iso.json",
    )?;
    smoke.as_publisher();

    let mut load = Vec::new();
    for version in LOAD_VERSIONS {
        // The publish token bucket is reset per publish, as above.
        d1_quiet(BURST_RESET)?;
        let zip = min_zip(&format!("load{version}"));
        publish_min_version(smoke, inputs, version, &zip)?;
        load.push((version, Arc::new(zip)));
        smoke.as_verifier();
        smoke.wcheck(PENDING_LISTING, &[200])?;
        let pending = smoke.body.clone();
        verify_version(
            smoke,
            inputs,
            &pending,
            version,
            &format!("entry-load-{version}.json"),
        )?;
        smoke.as_publisher();
    }
    Ok(Fixtures {
        iso_pending,
        iso_verified,
        load,
    })
}

/// Tiny pools via a restart: the operation windows persist in the
/// local Durable Object state, so the main run's consumption already
/// exceeds these limits and every fresh billable call must refuse -
/// while the edge cache, filled before the restart, keeps serving.
fn tiny_pools(smoke: &mut Smoke, inputs: &mut FinaleInputs<'_>, fixtures: &Fixtures) -> Result<()> {
    restart_on_tiny_pools(inputs)?;
    // Both routes are derived after the restart: the revision is the
    // archive's, not the running server's.
    let verified = artifact_route(inputs, ISO_VERIFIED_VERSION, &fixtures.iso_verified);
    let pending = artifact_route(inputs, ISO_PENDING_VERSION, &fixtures.iso_pending);
    cached_downloads(smoke, inputs)?;
    uncached_refusal(smoke, &verified)?;
    verifier_isolation(smoke, &pending)?;
    blocked_publish(smoke, inputs)?;
    idempotent_republish(smoke, inputs)?;
    source_reads(smoke, inputs)
}

fn restart_on_tiny_pools(inputs: &mut FinaleInputs<'_>) -> Result<()> {
    step("restarting wrangler dev with tiny governor pools");
    inputs.servers.stop_dev_servers();
    inputs.servers.append_dev_vars(
        r#"GOVERNOR_R2_CLASS_B_ORDINARY_MONTH="1"
GOVERNOR_R2_CLASS_A_PUBLISH_MONTH="1"
GOVERNOR_R2_CLASS_B_SOURCE_MONTH="0"
"#,
    )?;
    inputs.servers.start_registry_dev()?;
    inputs.servers.start_web_dev()
}

/// A cache HIT cannot have consumed the pool, and it thaws to the same
/// outward `no-store`: the stored copy's public header never escapes.
fn cached_downloads(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("cached verified downloads keep serving under an exhausted read pool");
    smoke.check(inputs.artifact_path, &[200])?;
    header_block(smoke, inputs.artifact_path)?;
    let block = String::from_utf8_lossy(&smoke.headers).into_owned();
    if grep_lines(&block, "cache-control: no-store").is_empty() {
        bail!(
            "a cache-hit artifact is missing the outward no-store: {}",
            grep_anywhere(&block, "cache-control").join("\n")
        );
    }
    Ok(())
}

/// Anonymous callers never reach the charged path at all: the
/// private-alpha 401 answers them before the governor is asked, so
/// they neither spend nor learn about the exhausted pool.
fn uncached_refusal(smoke: &mut Smoke, artifact: &str) -> Result<()> {
    step("an uncached verified download is refused with the budget envelope");
    smoke.check(artifact, &[503])?;
    smoke.expect_body("registry_over_budget")?;
    uniform_401_with(smoke, Base::Registry, "GET", artifact, &[], None)
}

fn verifier_isolation(smoke: &mut Smoke, pending: &str) -> Result<()> {
    step("the verifier pool is isolated from the exhausted ordinary pool");
    smoke.as_verifier();
    smoke.check(pending, &[200])?;
    smoke.as_publisher();
    Ok(())
}

fn blocked_publish(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("a fresh publish is refused before any r2 write when the publish pool is exhausted");
    d1_quiet(BURST_RESET)?;
    let blocked = min_zip("isoblocked");
    let body = publish_payload(inputs, BLOCKED_VERSION, &blocked);
    let upload_id = smoke.publish(&publish_route(inputs, BLOCKED_VERSION), &body, &[503])?;
    smoke.expect_body("registry_over_budget")?;
    if stored_upload(&upload_id, inputs.work)?.is_some() {
        bail!("a refused publish still wrote its archive to R2");
    }
    Ok(())
}

fn idempotent_republish(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("an idempotent re-publish stays a 200 no-op under an exhausted publish pool");
    smoke.publish(inputs.publish_path, inputs.publish_body, &[200])?;
    smoke.expect_body(r#""no_op":true"#)
}

fn source_reads(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("source-viewer reads fail closed on an exhausted source pool");
    let source = format!(
        "/api/v1/user/source/{}/{}/{ISO_VERIFIED_VERSION}",
        inputs.scope, inputs.name
    );
    session::session_request(
        smoke,
        inputs.session_cookie,
        "GET",
        &source,
        503,
        &[("Range".to_owned(), "bytes=-22".to_owned())],
        None,
    )?;
    smoke.expect_body("registry_over_budget")
}

/// The endpoint is the usage snapshot only: any other method is a 405
/// for the credential the GET admits.
fn admin_governor(smoke: &mut Smoke) -> Result<()> {
    step("the admin governor endpoint reports usage and takes no actions");
    smoke.as_ci_publisher();
    smoke.wcheck(GOVERNOR, &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.as_verifier();
    smoke.wcheck(GOVERNOR, &[200])?;
    smoke.expect_body(r#""ops""#)?;
    smoke.wrequest("POST", GOVERNOR, br#"{"wipe":true}"#, &[405])?;
    smoke.as_publisher();
    Ok(())
}

/// The ordinary-read pool gets exactly three fresh operations of
/// headroom, then two barrier-released waves race for them through the
/// serialized Durable Object, then a sequential retry pass.  The
/// invariant: distinct successes <= ledger delta <= headroom, and the
/// ledger never crosses the configured limit.
fn concurrency(
    smoke: &mut Smoke,
    inputs: &mut FinaleInputs<'_>,
    fixtures: &Fixtures,
) -> Result<()> {
    step("restarting wrangler dev with exact ordinary-read headroom");
    smoke.as_verifier();
    smoke.wcheck(GOVERNOR, &[200])?;
    let (used_before, window_before) = ordinary_usage(&smoke.body)?;
    smoke.as_publisher();
    let load_limit = used_before + HEADROOM;
    inputs.servers.stop_dev_servers();
    inputs.servers.rewrite_dev_vars_key(
        "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=",
        &format!("GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"{load_limit}\"\n"),
    )?;
    inputs.servers.start_registry_dev()?;
    inputs.servers.start_web_dev()?;

    step("concurrent downloads with retries never take the pool past its limit");
    d1_quiet(BURST_RESET)?;
    let wave = Wave {
        base: smoke.url(Base::Registry, ""),
        token: inputs.token.to_owned(),
        prefix: format!(
            "/archives/{}/{}/{}-{}-",
            inputs.scope, inputs.name, inputs.scope, inputs.name
        ),
    };
    // Wave 1: one request per artifact, all distinct, all uncached -
    // the only way any of them answers 200 is one fresh charged read,
    // and no request path charges more than once per attempt, so the
    // ledger delta must equal the success count exactly.  (The snapshot
    // is aggregate, so this pins totals, not per-request attribution.)
    let mut attempts = wave.fire(&fixtures.load, 1, 1)?;
    let wave1_successes = attempts
        .iter()
        .filter(|attempt| attempt.status == 200)
        .count();
    smoke.as_verifier();
    smoke.wcheck(GOVERNOR, &[200])?;
    let wave1_used = ordinary_usage(&smoke.body)?.0;
    smoke.as_publisher();
    let wave1_delta = wave1_used - used_before;
    if wave1_delta != i64::try_from(wave1_successes).unwrap_or(i64::MAX) {
        bail!(
            "wave 1 served {wave1_successes} distinct uncached artifacts but charged \
             {wave1_delta} operations (must be one-to-one)"
        );
    }
    // Wave 2: two simultaneous attempts per artifact - same-key races,
    // cache hits for wave 1's winners, refusals once the pool is dry.
    // The spans must overlap: the governor object serializes by design,
    // so "concurrent admission" means simultaneous arrival, which is
    // exactly what the spans witness.
    let wave2 = wave.fire(&fixtures.load, 2, 2)?;
    if !overlapping(&wave2) {
        bail!("no two wave-2 requests overlapped; the wave serialized");
    }
    attempts.extend(wave2);
    retry_pass(&wave, fixtures, &mut attempts)?;
    let refusals = sweep(&mut attempts)?;
    let successes = LOAD_VERSIONS
        .iter()
        .filter(|version| served(&attempts, version))
        .count();

    smoke.as_verifier();
    smoke.wcheck(GOVERNOR, &[200])?;
    let (used_after, window_after) = ordinary_usage(&smoke.body)?;
    smoke.as_publisher();
    if window_after != window_before {
        bail!(
            "the UTC month rolled over mid-leg ({window_before} -> {window_after}); \
             rerun the smoke test"
        );
    }
    let delta = used_after - used_before;
    println!(
        "    {successes}/5 distinct artifacts served, {refusals} refusal(s); \
         used {used_before} -> {used_after} (limit {load_limit})"
    );
    verdicts(used_after, load_limit, delta, successes, refusals)
}

/// Every one of these is about the same ledger, so they are read as
/// one block.
fn verdicts(
    used_after: i64,
    load_limit: i64,
    delta: i64,
    successes: usize,
    refusals: usize,
) -> Result<()> {
    let successes = i64::try_from(successes).unwrap_or(i64::MAX);
    if used_after > load_limit {
        bail!("the ledger crossed its configured limit: {used_after} > {load_limit}");
    }
    if delta > HEADROOM {
        bail!("the concurrent wave consumed {delta} operations against {HEADROOM} of headroom");
    }
    if successes > delta {
        bail!(
            "{successes} distinct successes but only {delta} charged operations - an uncharged serve"
        );
    }
    if successes < 1 {
        bail!("no download won any of the admitted operations");
    }
    if refusals < 1 {
        bail!("attempts exceeded the budget yet nothing answered the budget envelope");
    }
    Ok(())
}

/// One sequential retry per artifact that saw no success in either
/// wave: a retry must not mint allowance either.
fn retry_pass(wave: &Wave, fixtures: &Fixtures, attempts: &mut Vec<Attempt>) -> Result<()> {
    for (version, zip) in &fixtures.load {
        if served(attempts, version) {
            continue;
        }
        attempts.push(wave.request(version, zip, "retry", 1)?);
    }
    Ok(())
}

/// Every attempt must resolve to a success or the budget refusal: any
/// other status (a 500, a rate limit) would make the accounting
/// assertions vacuous for that attempt.  The sweep runs in label order,
/// so the same attempt decides the message.
fn sweep(attempts: &mut [Attempt]) -> Result<usize> {
    attempts.sort_by(|left, right| left.label.cmp(&right.label));
    let mut refusals = 0;
    for attempt in attempts.iter() {
        match attempt.status {
            200 => {}
            503 => {
                if !contains(&attempt.body, b"registry_over_budget") {
                    bail!(
                        "a load-test 503 was not the budget envelope: {}",
                        capture(&attempt.body)
                    );
                }
                refusals += 1;
            }
            other => bail!("a load-test attempt answered {other} (expected 200 or the budget 503)"),
        }
    }
    Ok(refusals)
}

/// Whether any of one artifact's attempts answered 200.
fn served(attempts: &[Attempt], version: &str) -> bool {
    attempts
        .iter()
        .any(|attempt| attempt.version == version && attempt.status == 200)
}

/// One recorded request.
struct Attempt {
    /// `status-<wave>-<version>-<attempt>`, which is what the sweep
    /// sorts on.
    label: String,
    version: String,
    status: u16,
    body: Vec<u8>,
    start: Instant,
    end: Instant,
}

/// What every request of a wave shares.  Held apart from the fixtures
/// so a thread can take an owned copy of only what it sends.
struct Wave {
    base: String,
    token: String,
    prefix: String,
}

impl Wave {
    /// Every request is parked on the barrier and released together, so
    /// the wave really arrives simultaneously instead of serializing on
    /// spawn order.
    fn fire(
        &self,
        artifacts: &[(&'static str, Arc<Vec<u8>>)],
        wave: u32,
        attempts: u32,
    ) -> Result<Vec<Attempt>> {
        let width = artifacts.len() * attempts as usize;
        let barrier = Arc::new(Barrier::new(width));
        let mut threads = Vec::with_capacity(width);
        // Spawned with the teardown signals blocked, which every thread
        // inherits: one that took a teardown signal while `DevServers`
        // reaps its children could signal a recycled pid.
        xtask_ci::masked(|| {
            for (version, zip) in artifacts {
                for attempt in 1..=attempts {
                    let barrier = Arc::clone(&barrier);
                    let zip = Arc::clone(zip);
                    let request = Request {
                        base: self.base.clone(),
                        token: self.token.clone(),
                        prefix: self.prefix.clone(),
                        version: (*version).to_owned(),
                        label: format!("status-{wave}-{version}-{attempt}"),
                    };
                    threads.push(std::thread::spawn(move || {
                        barrier.wait();
                        request.send(&zip)
                    }));
                }
            }
        });
        threads
            .into_iter()
            .map(|thread| match thread.join() {
                Ok(Ok(attempt)) => Ok(attempt),
                // Whatever the thread's own error, the wave fails with
                // this one message.
                Ok(Err(_)) | Err(_) => bail!("a load-test download process failed outright"),
            })
            .collect()
    }

    /// One request outside any wave (the sequential retry pass).
    fn request(&self, version: &str, zip: &[u8], wave: &str, attempt: u32) -> Result<Attempt> {
        Request {
            base: self.base.clone(),
            token: self.token.clone(),
            prefix: self.prefix.clone(),
            version: version.to_owned(),
            label: format!("status-{wave}-{version}-{attempt}"),
        }
        .send(zip)
    }
}

/// One thread's whole world: everything it needs is owned before the
/// barrier, and nothing it does touches the run's shared state.
struct Request {
    base: String,
    token: String,
    prefix: String,
    version: String,
    label: String,
}

impl Request {
    /// The span covers the artifact route's derivation as well as the
    /// request.
    fn send(self, zip: &[u8]) -> Result<Attempt> {
        let start = Instant::now();
        let url = format!(
            "{}{}{}-{}.zip",
            self.base,
            self.prefix,
            self.version,
            revision_of(zip)
        );
        // A per-thread agent, so no connection pool can serialize the
        // wave the way a shared one would.
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let sent = agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call();
        let response = match sent {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(error).with_context(|| format!("GET {url} failed")),
        };
        let status = response.status();
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .with_context(|| format!("reading the body of GET {url}"))?;
        Ok(Attempt {
            label: self.label,
            version: self.version,
            status,
            body,
            start,
            end: Instant::now(),
        })
    }
}

/// At least two spans of the wave overlap.
fn overlapping(attempts: &[Attempt]) -> bool {
    attempts.len() >= 2
        && attempts.iter().enumerate().any(|(index, left)| {
            attempts.iter().enumerate().any(|(other, right)| {
                index != other && left.start < right.end && right.start < left.end
            })
        })
}

fn publish_min_version(
    smoke: &mut Smoke,
    inputs: &FinaleInputs<'_>,
    version: &str,
    zip: &[u8],
) -> Result<()> {
    let body = publish_payload(inputs, version, zip);
    smoke.publish(&publish_route(inputs, version), &body, &[201])?;
    smoke.expect_body(r#""verification":"pending""#)
}

/// The verified verdict a listing entry binds: the checksum and
/// `published_at` the listing reported.
fn verify_version(
    smoke: &mut Smoke,
    inputs: &FinaleInputs<'_>,
    pending: &[u8],
    version: &str,
    entry_file: &str,
) -> Result<()> {
    let entry_path = inputs.work.join(entry_file);
    session::listing_entry(pending, &package(inputs), version, &entry_path)?;
    let entry = parse_entry(&entry_path)?;
    let mut verdict = Map::new();
    verdict.insert("verdict".to_owned(), Value::String("verified".to_owned()));
    copy_field(&mut verdict, "checksum", entry.get("checksum"));
    copy_field(&mut verdict, "published_at", entry.get("published_at"));
    let body = json_bytes(&Value::Object(verdict))?;
    smoke.verdict_patch(&verdict_route(inputs, version), &body, &[200])?;
    smoke.expect_body(r#""verification":"verified""#)
}

/// The listing entry, read back from the file `listing_entry` just
/// wrote.
fn parse_entry(path: &Path) -> Result<Value> {
    serde_json::from_slice(&read(path)?).with_context(|| format!("parse {}", path.display()))
}

/// The download is the subject and neither shared buffer is written, so
/// an assertion further down still reads what the last checked request
/// left behind.
fn download(smoke: &mut Smoke, path: &str) -> Result<Vec<u8>> {
    let url = smoke.url(Base::Registry, path);
    let auth = smoke.auth.clone();
    let body = std::mem::take(&mut smoke.body);
    let headers = std::mem::take(&mut smoke.headers);
    smoke.http("GET", &url, &auth, None)?;
    smoke.headers = headers;
    Ok(std::mem::replace(&mut smoke.body, body))
}

/// The header block is the whole subject, and [`Smoke::body`] keeps
/// whatever the previous request left in it.
fn header_block(smoke: &mut Smoke, path: &str) -> Result<()> {
    let url = smoke.url(Base::Registry, path);
    let auth = smoke.auth.clone();
    let body = std::mem::take(&mut smoke.body);
    smoke.http("GET", &url, &auth, None)?;
    smoke.body = body;
    Ok(())
}

fn package(inputs: &FinaleInputs<'_>) -> String {
    format!("{}/{}", inputs.scope, inputs.name)
}

fn publish_route(inputs: &FinaleInputs<'_>, version: &str) -> String {
    format!(
        "/api/v1/packages/{}/{}/{version}",
        inputs.scope, inputs.name
    )
}

fn verdict_route(inputs: &FinaleInputs<'_>, version: &str) -> String {
    format!(
        "/api/v1/admin/versions/{}/{}/{version}",
        inputs.scope, inputs.name
    )
}

fn artifact_route(inputs: &FinaleInputs<'_>, version: &str, archive: &[u8]) -> String {
    format!(
        "/archives/{}/{}/{}-{}-{version}-{}.zip",
        inputs.scope,
        inputs.name,
        inputs.scope,
        inputs.name,
        revision_of(archive)
    )
}

/// The version replace and `retarget_hash`, then `frame`: textual
/// substitutions over the frozen document's raw bytes, never a JSON
/// edit, so every other byte stays as frozen.
fn publish_payload(inputs: &FinaleInputs<'_>, version: &str, archive: &[u8]) -> Vec<u8> {
    let metadata = replace_all(
        inputs.fixture_metadata,
        FIXTURE_VERSION.as_bytes(),
        version.as_bytes(),
    );
    let metadata = retarget_hash(&metadata, inputs.blob_hash, &sha256_hex(archive));
    frame(&metadata, archive)
}

/// A hand-packed container: one stored, zero-length entry, exact EOCD
/// arithmetic, no zip crate.
fn min_zip(entry: &str) -> Vec<u8> {
    stored_zip(&[(entry, b"")])
}

/// The container [`min_zip`] packs, over any stored entries.
fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut cd = Vec::new();
    for (entry, data) in entries {
        let name = entry.as_bytes();
        let length = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let crc = crc32fast::hash(data);
        let bytes = size(data.len());
        let offset = size(local.len());
        // "<IHHHHHIIIHH": signature, version 2.0, flags, method, mtime,
        // mdate, crc32, compressed size, uncompressed size, name length,
        // extra length.
        local.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        local.extend_from_slice(&20_u16.to_le_bytes());
        local.extend_from_slice(&[0; 8]);
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&bytes.to_le_bytes());
        local.extend_from_slice(&bytes.to_le_bytes());
        local.extend_from_slice(&length.to_le_bytes());
        local.extend_from_slice(&[0; 2]);
        local.extend_from_slice(name);
        local.extend_from_slice(data);
        // "<IHHHHHHIIIHHHHHII": signature, version made by, version
        // needed, then the same header, then name/extra/comment
        // lengths, disk number, internal and external attributes, and
        // the local header's offset.
        cd.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        cd.extend_from_slice(&20_u16.to_le_bytes());
        cd.extend_from_slice(&20_u16.to_le_bytes());
        cd.extend_from_slice(&[0; 8]);
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&bytes.to_le_bytes());
        cd.extend_from_slice(&bytes.to_le_bytes());
        cd.extend_from_slice(&length.to_le_bytes());
        cd.extend_from_slice(&[0; 8]);
        cd.extend_from_slice(&[0; 4]);
        cd.extend_from_slice(&offset.to_le_bytes());
        cd.extend_from_slice(name);
    }
    // "<IHHHHIIH": signature, this disk, the directory's disk, entries
    // here, entries total, directory size, directory offset, comment.
    let count = u16::try_from(entries.len()).unwrap_or(u16::MAX);
    let mut eocd = Vec::with_capacity(22);
    eocd.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    eocd.extend_from_slice(&[0; 4]);
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&size(cd.len()).to_le_bytes());
    eocd.extend_from_slice(&size(local.len()).to_le_bytes());
    eocd.extend_from_slice(&[0; 2]);

    let mut zip = local;
    zip.extend_from_slice(&cd);
    zip.extend_from_slice(&eocd);
    zip
}

fn size(bytes: usize) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

/// The `b_ordinary` pool's used count and window, or `0` and `-` when
/// the pool has no row yet.
fn ordinary_usage(body: &[u8]) -> Result<(i64, String)> {
    let snapshot: Value =
        serde_json::from_slice(body).context("parse the governor usage snapshot")?;
    let Some(row) = rows(&snapshot, "ops").find(|row| pool(row) == Some("b_ordinary")) else {
        return Ok((0, "-".to_owned()));
    };
    let used = row.get("used").map(display).unwrap_or_default();
    let window = row.get("window").map(display).unwrap_or_default();
    let used = used
        .parse::<i64>()
        .with_context(|| format!("the b_ordinary pool reports a non-integer used count: {used}"))?;
    Ok((used, window))
}

fn rows<'a>(snapshot: &'a Value, field: &str) -> impl Iterator<Item = &'a Value> {
    snapshot
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn pool(row: &Value) -> Option<&str> {
    row.get("pool").and_then(Value::as_str)
}

/// A field the source document does not carry stays absent, not null;
/// an explicit `null` is copied.
fn copy_field(into: &mut Map<String, Value>, field: &str, value: Option<&Value>) {
    if let Some(value) = value {
        into.insert(field.to_owned(), value.clone());
    }
}

fn json_bytes(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("serialize a request body")
}

/// `wc -l <log>` + 1: the line the next `tail -n +N` starts at, which
/// is the count of newline-separated segments - one more than the
/// newlines `wc -l` counts.
fn line_mark(log: &Path) -> Result<usize> {
    Ok(read(log)?.split(|byte| *byte == b'\n').count())
}

/// 20 × 0.5 s of `tail -n +<from> <log> | grep -q <needle>`, over a
/// file the dev server is still appending to.
fn await_log(log: &Path, from: usize, needle: &str) -> Result<bool> {
    for _ in 0..POLLS {
        if tail(log, from)?.contains(needle) {
            return Ok(true);
        }
        std::thread::sleep(HALF_SECOND);
    }
    Ok(false)
}

/// `tail -n +<from>`: the log from that line on, lossily decoded - a
/// wrangler log carries whatever the Worker printed.
fn tail(log: &Path, from: usize) -> Result<String> {
    let text = String::from_utf8_lossy(&read(log)?).into_owned();
    let mut rest = text.as_str();
    for _ in 1..from {
        match rest.find('\n') {
            Some(at) => rest = &rest[at + 1..],
            None => return Ok(String::new()),
        }
    }
    Ok(rest.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    const EVIL_ZIP: &str = "504b030414000000000000000000000000000000000000000000070000002e2e2f6576696c504b01021400140000000000000000000000000000000000000000000700000000000000000000000000000000002e2e2f6576696c504b0506000000000100010035000000250000000000";
    const MIN_ZIP: &str = "504b0304140000000000000000000000000000000000000000000a00000069736f70656e64696e67504b01021400140000000000000000000000000000000000000000000a000000000000000000000000000000000069736f70656e64696e67504b0506000000000100010038000000280000000000";

    /// Pinned bytes for both entry names, because the name length rides
    /// both headers' name fields and moves the EOCD's directory size and
    /// offset.
    #[test]
    fn the_min_zip_matches_the_pinned_bytes() {
        assert_eq!(hex(&min_zip("../evil")), EVIL_ZIP);
        assert_eq!(hex(&min_zip("isopending")), MIN_ZIP);
    }

    /// The container is what the worker's fixed-offset gate reads: 30
    /// bytes of local header, 46 of central directory, 22 of EOCD, and
    /// the EOCD's offset field pointing at the directory.
    #[test]
    fn the_min_zip_arithmetic_is_exact() {
        let zip = min_zip("isoverified");
        let name = "isoverified".len();
        assert_eq!(zip.len(), 30 + name + 46 + name + 22);
        assert_eq!(&zip[..4], b"PK\x03\x04");
        assert_eq!(&zip[30 + name..30 + name + 4], b"PK\x01\x02");
        assert_eq!(&zip[zip.len() - 22..zip.len() - 18], b"PK\x05\x06");
        let directory_size =
            u32::from_le_bytes(zip[zip.len() - 10..zip.len() - 6].try_into().unwrap());
        let directory_at =
            u32::from_le_bytes(zip[zip.len() - 6..zip.len() - 2].try_into().unwrap());
        assert_eq!(directory_size as usize, 46 + name);
        assert_eq!(directory_at as usize, 30 + name);
    }

    #[test]
    fn the_tail_starts_at_the_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("dev.log");
        fs::write(&log, "one\ntwo\n").unwrap();
        let mark = line_mark(&log).unwrap();
        assert_eq!(mark, 3);
        // Nothing appended yet: the tail is empty, so an earlier
        // occurrence of the needle cannot satisfy the wait.
        assert_eq!(tail(&log, mark).unwrap(), "");
        fs::write(&log, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(&log, mark).unwrap(), "three\n");
        assert_eq!(tail(&log, 1).unwrap(), "one\ntwo\nthree\n");
    }

    fn attempt(label: &str, version: &str, status: u16) -> Attempt {
        let start = Instant::now();
        Attempt {
            label: label.to_owned(),
            version: version.to_owned(),
            status,
            body: b"registry_over_budget".to_vec(),
            start,
            end: start,
        }
    }

    /// The sweep must visit the attempts in label order, so the same
    /// one decides the failure message.
    #[test]
    fn the_sweep_visits_attempts_in_label_order() {
        let mut attempts = vec![
            attempt("status-retry-0.7.0-1", "0.7.0", 500),
            attempt("status-2-0.7.0-1", "0.7.0", 404),
            attempt("status-1-0.10.0-1", "0.10.0", 200),
        ];
        assert_eq!(
            sweep(&mut attempts).unwrap_err().to_string(),
            "a load-test attempt answered 404 (expected 200 or the budget 503)"
        );
        assert_eq!(
            attempts
                .iter()
                .map(|a| a.label.as_str())
                .collect::<Vec<_>>(),
            [
                "status-1-0.10.0-1",
                "status-2-0.7.0-1",
                "status-retry-0.7.0-1"
            ]
        );
    }

    #[test]
    fn a_non_budget_503_is_not_a_refusal() {
        let mut refusal = vec![attempt("status-1-0.7.0-1", "0.7.0", 503)];
        assert_eq!(sweep(&mut refusal).unwrap(), 1);
        let mut other = vec![attempt("status-1-0.7.0-1", "0.7.0", 503)];
        other[0].body = b"{\"errors\":[{\"detail\":\"nope\"}]}".to_vec();
        assert_eq!(
            sweep(&mut other).unwrap_err().to_string(),
            "a load-test 503 was not the budget envelope: {\"errors\":[{\"detail\":\"nope\"}]}"
        );
    }

    #[test]
    fn overlap_needs_two_spans_that_really_overlap() {
        let base = Instant::now();
        let span = |start: u64, end: u64| Attempt {
            label: String::new(),
            version: String::new(),
            status: 200,
            body: Vec::new(),
            start: base + Duration::from_millis(start),
            end: base + Duration::from_millis(end),
        };
        assert!(overlapping(&[span(0, 10), span(5, 15)]));
        assert!(!overlapping(&[span(0, 10), span(10, 20)]));
        assert!(!overlapping(&[span(0, 10)]));
        assert!(overlapping(&[span(0, 1), span(50, 60), span(55, 65)]));
    }

    const SNAPSHOT: &str = r#"{
      "storage": [],
      "ops": [{"pool": "b_ordinary", "used": 7, "window": "2026-08"}]
    }"#;

    #[test]
    fn the_usage_reading_falls_back_to_a_missing_row() {
        assert_eq!(
            ordinary_usage(SNAPSHOT.as_bytes()).unwrap(),
            (7, "2026-08".to_owned())
        );
        assert_eq!(
            ordinary_usage(br#"{"ops":[{"pool":"b_source","used":3}]}"#).unwrap(),
            (0, "-".to_owned())
        );
        assert_eq!(
            ordinary_usage(br#"{"ops":[]}"#).unwrap(),
            (0, "-".to_owned())
        );
    }

    #[test]
    fn the_load_verdicts_reject_every_broken_invariant() {
        assert_eq!(
            verdicts(5, 4, 1, 1, 1).unwrap_err().to_string(),
            "the ledger crossed its configured limit: 5 > 4"
        );
        assert_eq!(
            verdicts(4, 4, 4, 1, 1).unwrap_err().to_string(),
            "the concurrent wave consumed 4 operations against 3 of headroom"
        );
        assert_eq!(
            verdicts(4, 4, 1, 2, 1).unwrap_err().to_string(),
            "2 distinct successes but only 1 charged operations - an uncharged serve"
        );
        assert_eq!(
            verdicts(4, 4, 1, 0, 1).unwrap_err().to_string(),
            "no download won any of the admitted operations"
        );
        assert_eq!(
            verdicts(4, 4, 1, 1, 0).unwrap_err().to_string(),
            "attempts exceeded the budget yet nothing answered the budget envelope"
        );
        verdicts(4, 4, 3, 3, 1).unwrap();
    }
}
