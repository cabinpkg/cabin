//! The run's tail: the backup and breaker cron legs, the strict
//! zip-container profile, the governor's hard limits across two mid-run
//! dev-server restarts, the admin governor endpoint, the real
//! verification driver's pass over two fresh versions (`driver_pass`),
//! the bounded-concurrency leg, and the admission leg, after which the
//! run closes with `smoke OK`.
//!
//! The concurrency leg runs its waves on threads released by a
//! [`Barrier`], not as `xtask_ci::spawn_tracked` children: that table is
//! for children the teardown must kill, it holds eight, and wave two
//! alone wants ten.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use xtask_registry_admin::verify::{Endpoints, run_with};
use xtask_registry_admin::{BACKUP_BUCKET, BLOBS_BUCKET, display, wrangler};

use crate::bytes::{frame, replace_all, retarget_hash, revision_of, sha256_hex};
use crate::context::{Base, Smoke};
use crate::legs::anonymous::uniform_401_with;
use crate::legs::session;
use crate::servers::{DevServers, d1, d1_quiet, d1_rows};
use crate::step;
use crate::text::{capture, contains, grep_anywhere, grep_lines, read, write};

const POLLS: u32 = 20;
const HALF_SECOND: Duration = Duration::from_millis(500);

/// The two cron expressions `wrangler dev --test-scheduled` routes.
/// The `+` are the expression's spaces and must reach the query string
/// as `+`: percent-encoding them to `%2B` names a different cron.
const DUMP_CRON: &str = "/__scheduled?cron=0+3+*+*+*";
const BREAKER_CRON: &str = "/__scheduled?cron=*/15+*+*+*+*";

const PENDING_LISTING: &str = "/api/v1/admin/versions?status=pending";
const GOVERNOR: &str = "/api/v1/admin/governor";

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
const WITHDEP_MANIFEST: &str = "\
[package]
name = \"smoke/withdep\"
version = \"0.2.0\"
cxx-standard = \"c++20\"

[dependencies]
\"smoke/nodep\" = \"^0.1\"

[target.withdep]
type = \"library\"
sources = [\"src/withdep.cc\"]
interface-cxx-standard = \"c++17\"
links = \"withdep-native\"
";
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
    /// The four children, for the two mid-run restarts and for the
    /// `.dev.vars` mutations between them.
    pub servers: &'a mut DevServers,
    /// The run's scratch directory.
    pub work: &'a Path,
    /// The mock directory, whose `dump.sql` the stored backup must
    /// equal.
    pub mock_dir: &'a Path,
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
    let today = utc_today()?;
    let dump_key = format!("d1/{today}.sql");
    backup_cron(smoke, inputs, &dump_key)?;
    dump_sidecar(inputs, &today, &dump_key)?;
    let last_backup_at = meta_records_backup(&dump_key)?;
    same_day_rerun(smoke, &last_backup_at)?;
    breaker_cron(smoke, inputs)?;
    zip_profile(smoke, inputs)?;
    driver_pass(smoke, inputs)?;
    let fixtures = governor_fixtures(smoke, inputs)?;
    tiny_pools(smoke, inputs, &fixtures)?;
    admin_governor(smoke)?;
    admin_reconcile(smoke)?;
    cron_reconcile(smoke, inputs)?;
    concurrency(smoke, inputs, &fixtures)?;
    // Dead last on purpose: the exhaustion burst empties the shared
    // local admission bucket (crate::legs::admission), and any OIDC
    // request after it would answer the 429 for up to a minute.
    crate::legs::admission::run(smoke)?;
    println!("smoke OK");
    Ok(())
}

/// The `/__scheduled` route invokes the cron handler; any non-breaker
/// expression routes to the dump job, which talks to the export-API
/// mock.
fn backup_cron(smoke: &mut Smoke, inputs: &FinaleInputs<'_>, dump_key: &str) -> Result<()> {
    step("the backup cron stores a validated dump in the BACKUP bucket");
    smoke.check(DUMP_CRON, &[200])?;
    let stored = inputs.work.join("stored-dump.sql");
    let mut appeared = false;
    for _ in 0..POLLS {
        if r2_get(&format!("{BACKUP_BUCKET}/{dump_key}"), &stored)? {
            appeared = true;
            break;
        }
        std::thread::sleep(HALF_SECOND);
    }
    if !appeared {
        bail!("dump {dump_key} never appeared in the BACKUP bucket");
    }
    if read(&stored)? != read(&inputs.mock_dir.join("dump.sql"))? {
        bail!("stored dump differs from the mock's exported dump");
    }
    Ok(())
}

/// The sidecar names the dump by its date, so the copy it is checked
/// against has to carry that name.
fn dump_sidecar(inputs: &FinaleInputs<'_>, today: &str, dump_key: &str) -> Result<()> {
    step("the dump's sha256 sidecar verifies with shasum -c");
    let sidecar = inputs.work.join(format!("{today}.sql.sha256"));
    if !r2_get(&format!("{BACKUP_BUCKET}/{dump_key}.sha256"), &sidecar)? {
        bail!("sidecar {dump_key}.sha256 is missing");
    }
    let dump = inputs.work.join("stored-dump.sql");
    fs::copy(&dump, inputs.work.join(format!("{today}.sql")))
        .with_context(|| format!("copy {}", dump.display()))?;
    let listing = String::from_utf8_lossy(&read(&sidecar)?).into_owned();
    if !checksums_verify(inputs.work, &listing) {
        bail!(
            "shasum -c rejected the sidecar: {}",
            listing.trim_end_matches('\n')
        );
    }
    Ok(())
}

fn meta_records_backup(dump_key: &str) -> Result<String> {
    step("meta records the backup");
    let rows =
        d1_rows("SELECT key, value FROM meta WHERE key IN ('last_backup_at', 'last_backup_key')")?;
    let recorded = key_values(&rows);
    let last_backup_at = recorded.get("last_backup_at").cloned().unwrap_or_default();
    if recorded.get("last_backup_key").map(String::as_str) != Some(dump_key)
        || !is_timestamp(&last_backup_at)
    {
        bail!("meta.last_backup_at / last_backup_key not recorded");
    }
    println!("    last_backup_at = {last_backup_at}");
    Ok(last_backup_at)
}

/// One validated dump per date: a same-day re-run must skip instead of
/// re-exporting, because a failed re-export would overwrite the
/// verified copy.
fn same_day_rerun(smoke: &mut Smoke, last_backup_at: &str) -> Result<()> {
    step("a same-day re-run of the dump job is a no-op");
    smoke.check(DUMP_CRON, &[200])?;
    let rows = d1_rows("SELECT value FROM meta WHERE key = 'last_backup_at'")?;
    let rerun_at = rows
        .first()
        .and_then(|row| row.get("value"))
        .map(display)
        .context("meta lost last_backup_at")?;
    if rerun_at != last_backup_at {
        bail!("same-day re-run rewrote last_backup_at: {rerun_at} (was {last_backup_at})");
    }
    Ok(())
}

/// The breaker expression additionally runs the governor
/// reconciliation, and the usage log line proves the ledger answered.
fn breaker_cron(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("the breaker cron reconciles the governor ledger");
    smoke.check(BREAKER_CRON, &[200])?;
    if !await_log(inputs.servers.dev_log(), 1, "governor usage:")? {
        bail!("the breaker cron pass never logged the governor usage snapshot");
    }
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
    smoke.wrequest(
        "PUT",
        &publish_route(inputs, PROFILE_VERSION),
        &body,
        &[400],
    )?;
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
    smoke.wrequest(
        "PUT",
        &publish_route(inputs, PROFILE_VERSION),
        &body,
        &[201],
    )?;
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
/// judges every pending version it lists, the two earlier legs left
/// behind included, and nothing after this reads those two.
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

    step("the verification driver verifies the clean repack and rejects the traversal");
    let (passed, _) = spawn_driver(smoke, inputs.verifier_bin)?;
    if !passed {
        bail!("the verification driver failed its pass");
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
    )
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
/// one (never downloaded), each with distinct content-addressed bytes,
/// then five more for the load leg.
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
    }
    smoke.as_verifier();
    smoke.wcheck(PENDING_LISTING, &[200])?;
    let pending = smoke.body.clone();
    for version in LOAD_VERSIONS {
        verify_version(
            smoke,
            inputs,
            &pending,
            version,
            &format!("entry-load-{version}.json"),
        )?;
    }
    smoke.as_publisher();
    Ok(Fixtures {
        iso_pending,
        iso_verified,
        load,
    })
}

/// Tiny pools via a restart: the ledger and windows persist in the
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
        "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"1\"\n\
         GOVERNOR_STORAGE_PRIMARY_BYTES=\"1\"\n\
         GOVERNOR_R2_CLASS_B_SOURCE_MONTH=\"0\"\n",
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
    step("a fresh publish is refused before any r2 write when storage is exhausted");
    d1_quiet(BURST_RESET)?;
    let blocked = min_zip("isoblocked");
    let blocked_hash = sha256_hex(&blocked);
    let body = publish_payload(inputs, BLOCKED_VERSION, &blocked);
    smoke.wrequest(
        "PUT",
        &publish_route(inputs, BLOCKED_VERSION),
        &body,
        &[503],
    )?;
    smoke.expect_body("registry_over_budget")?;
    // The blob bucket a refused publish must not have written to.
    if r2_get(
        &format!("{BLOBS_BUCKET}/blobs/sha256/{blocked_hash}"),
        Path::new("/dev/null"),
    )? {
        bail!("a refused publish still wrote its blob to R2");
    }
    Ok(())
}

fn idempotent_republish(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("an idempotent re-publish stays a 200 no-op under a full storage pool");
    smoke.wrequest("PUT", inputs.publish_path, inputs.publish_body, &[200])?;
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

/// The pre-launch ledger wipe clears the storage rows of every pool -
/// the registry wipe empties both buckets - while the monthly op
/// windows survive: they mirror R2 operations Cloudflare already
/// metered this month, so zeroing them would re-mint a month of
/// allowance for spend that already happened.
fn admin_governor(smoke: &mut Smoke) -> Result<()> {
    step("the admin governor endpoint reports usage and takes operator actions");
    smoke.as_ci_publisher();
    smoke.wcheck(GOVERNOR, &[403])?;
    smoke.expect_body("verify scope")?;
    smoke.as_verifier();
    smoke.wcheck(GOVERNOR, &[200])?;
    smoke.expect_body(r#""storage""#)?;
    // An idempotent release of an unknown key answers ok.
    let release = br#"{"release":{"pool":"primary","key":"blobs/sha256/none"}}"#;
    smoke.wrequest("POST", GOVERNOR, release, &[200])?;
    smoke.expect_body(r#""ok":true"#)?;
    smoke.wrequest("POST", GOVERNOR, br#"{"wipe":true}"#, &[200])?;
    smoke.expect_body(r#""ok":true"#)?;
    smoke.wcheck(GOVERNOR, &[200])?;
    if !wiped_the_right_rows(&smoke.body) {
        bail!(
            "the governor wipe cleared the op windows or left the wrong rows: {}",
            capture(&smoke.body)
        );
    }
    Ok(())
}

/// The on-demand reconcile is the operator's recovery path after a
/// ledger wipe or a Durable Object storage loss: the same increase-only
/// primary rebuild the cron runs, answering with the report instead of
/// waiting up to 15 minutes.
fn admin_reconcile(smoke: &mut Smoke) -> Result<()> {
    step("an admin reconcile rebuilds the wiped primary ledger on demand");
    smoke.wrequest("POST", GOVERNOR, br#"{"reconcile":true}"#, &[200])?;
    if !added_anything(&smoke.body) {
        bail!(
            "the on-demand reconcile recorded nothing after the wipe: {}",
            capture(&smoke.body)
        );
    }
    smoke.wcheck(GOVERNOR, &[200])?;
    if !has_committed_primary(&smoke.body) {
        bail!(
            "the on-demand reconcile left no committed primary rows: {}",
            capture(&smoke.body)
        );
    }
    // The body names exactly one action, so `reconcile` with `wipe` is a
    // 400.
    smoke.wrequest(
        "POST",
        GOVERNOR,
        br#"{"reconcile":true,"wipe":true}"#,
        &[400],
    )?;
    // A second wipe re-empties the storage rows so the cron leg below
    // still proves the scheduled pass rebuilds them on its own.
    smoke.wrequest("POST", GOVERNOR, br#"{"wipe":true}"#, &[200])?;
    smoke.expect_body(r#""ok":true"#)
}

/// The next breaker pass commits every live checksum back into the
/// ledger and logs how many it recorded; the pass after that logs the
/// rebuilt rows in its usage snapshot, because each pass logs usage
/// before it reconciles.  Both waits are line watermarks over a log a
/// live child is still appending to.
fn cron_reconcile(smoke: &mut Smoke, inputs: &FinaleInputs<'_>) -> Result<()> {
    step("reconciliation rebuilds the wiped primary ledger from d1");
    let log = inputs.servers.dev_log().to_path_buf();
    let mark = line_mark(&log)?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !await_log(&log, mark, "previously unledgered blob")? {
        bail!("the post-wipe cron pass never re-committed the live primary set");
    }
    let mark = line_mark(&log)?;
    smoke.check(BREAKER_CRON, &[200])?;
    if !await_log(&log, mark, "primary/committed=")? {
        bail!("the rebuilt primary ledger never appeared in the usage snapshot");
    }
    // The wipe refuses while the registry is launched (the wipe guard).
    d1_quiet("UPDATE meta SET value = 'true' WHERE key = 'launched';")?;
    smoke.wrequest("POST", GOVERNOR, br#"{"wipe":true}"#, &[403])?;
    smoke.expect_body("launched")?;
    d1_quiet("UPDATE meta SET value = 'false' WHERE key = 'launched';")?;
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
            "/artifacts/{}/{}/{}-{}-",
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
    smoke.wrequest("PUT", &publish_route(inputs, version), &body, &[201])?;
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
        "/artifacts/{}/{}/{}-{}-{version}-{}.zip",
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

/// The three governor-snapshot readings.  A body that is not a
/// snapshot fails the assertion, so a parse failure answers `false`
/// here rather than raising its own error.
fn wiped_the_right_rows(body: &[u8]) -> bool {
    let Ok(snapshot) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    if snapshot
        .get("storage")
        .and_then(Value::as_array)
        .is_none_or(|storage| !storage.is_empty())
    {
        return false;
    }
    // The exhausted ordinary-read counter must survive the wipe: the
    // ops Cloudflare already metered this month are not re-minted.
    rows(&snapshot, "ops")
        .find(|row| pool(row) == Some("b_ordinary"))
        .is_some_and(|row| row.get("used").and_then(Value::as_i64) != Some(0))
}

fn added_anything(body: &[u8]) -> bool {
    let Ok(report) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    report
        .get("added")
        .and_then(Value::as_array)
        .is_some_and(|added| !added.is_empty())
}

fn has_committed_primary(body: &[u8]) -> bool {
    let Ok(snapshot) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    rows(&snapshot, "storage").any(|row| {
        pool(row) == Some("primary")
            && row.get("state").and_then(Value::as_str) == Some("committed")
    })
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

/// The rows of a key/value read as a map: a later row for the same key
/// wins.
fn key_values(rows: &[Map<String, Value>]) -> std::collections::HashMap<String, String> {
    rows.iter()
        .filter_map(|row| Some((display(row.get("key")?), display(row.get("value")?))))
        .collect()
}

/// `wrangler r2 object get … --file <path> --local`, both streams
/// discarded: every caller words its own miss, and the backup poll and
/// the refused-publish check expect one.
fn r2_get(key: &str, file: &Path) -> Result<bool> {
    let path = file.to_str().context("an R2 destination is not UTF-8")?;
    let status = wrangler(&["r2", "object", "get", key, "--file", path, "--local"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run wrangler r2 object get")?;
    Ok(status.success())
}

/// `/^\d{4}-\d{2}-\d{2}T/`.
fn is_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 11
        && bytes[..11]
            .iter()
            .zip("dddd-dd-ddT".bytes())
            .all(|(byte, shape)| match shape {
                b'd' => byte.is_ascii_digit(),
                other => *byte == other,
            })
}

/// `shasum -a 256 -c <listing>` run from `dir`: every listed file must
/// hash to the digest beside it, and a listing with no usable line is a
/// failure, as `shasum` reports one.
///
/// Only the two formats `shasum` writes are read (`<hex>  <name>`
/// and the binary-mode `<hex> *<name>`); its BSD `SHA256 (name) = hex`
/// tag form is not, and the worker writes the first of the two
/// (`registry/src/backup_glue.rs`).
fn checksums_verify(dir: &Path, listing: &str) -> bool {
    let mut checked = 0;
    for line in listing.lines().filter(|line| !line.trim().is_empty()) {
        let Some((digest, rest)) = line.split_once(' ') else {
            return false;
        };
        let name = rest.strip_prefix([' ', '*']).unwrap_or(rest);
        let Ok(contents) = fs::read(dir.join(name)) else {
            return false;
        };
        if !sha256_hex(&contents).eq_ignore_ascii_case(digest) {
            return false;
        }
        checked += 1;
    }
    checked > 0
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

/// `date -u +%F`.  UTC, not local: the backup key and the sidecar's
/// name read this, and a local date crosses midnight at the wrong
/// moment.
fn utc_today() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("the system clock is before the epoch")?
        .as_secs();
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(days);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// Days since 1970-01-01 to a proleptic Gregorian date, by the shift-
/// the-era method: March-based years make the leap day the last day of
/// the year, so the month-length table becomes arithmetic.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
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

    /// Every derived fixture is content-addressed, so two entry names
    /// must not collide - the load leg needs five distinct uncached
    /// artifacts.
    #[test]
    fn distinct_entry_names_give_distinct_bytes() {
        let mut revisions: Vec<String> = LOAD_VERSIONS
            .iter()
            .map(|version| revision_of(&min_zip(&format!("load{version}"))))
            .collect();
        revisions.sort();
        revisions.dedup();
        assert_eq!(revisions.len(), LOAD_VERSIONS.len());
    }

    #[test]
    fn the_sidecar_is_checked_against_the_named_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("2026-08-05.sql"), b"dump\n").unwrap();
        let digest = sha256_hex(b"dump\n");

        let listing = format!("{digest}  2026-08-05.sql\n");
        assert!(checksums_verify(dir.path(), &listing));
        // Binary mode, which shasum also writes and also accepts.
        assert!(checksums_verify(
            dir.path(),
            &format!("{digest} *2026-08-05.sql\n")
        ));
        // A digest that does not match, a file that is not there, and a
        // listing with nothing in it all fail.
        assert!(!checksums_verify(
            dir.path(),
            &listing.replace(&digest[..1], "0")
        ));
        assert!(!checksums_verify(
            dir.path(),
            &format!("{digest}  absent.sql\n")
        ));
        assert!(!checksums_verify(dir.path(), "\n"));
    }

    #[test]
    fn the_timestamp_shape_is_the_iso_date_prefix() {
        assert!(is_timestamp("2026-08-05T03:00:00Z"));
        assert!(!is_timestamp("2026-08-05 03:00:00Z"));
        assert!(!is_timestamp("2026-8-05T03:00:00Z"));
        assert!(!is_timestamp(""));
    }

    #[test]
    fn the_utc_date_reads_the_civil_calendar() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // A leap day, and the century that is not one.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
        assert_eq!(civil_from_days(20_634), (2026, 6, 30));
        assert_eq!(civil_from_days(20_670), (2026, 8, 5));
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
      "storage": [{"pool": "primary", "state": "committed"}, {"pool": "backup"}],
      "ops": [{"pool": "b_ordinary", "used": 7, "window": "2026-08"}]
    }"#;

    #[test]
    fn the_wipe_snapshot_is_empty_storage_and_kept_op_windows() {
        // Before the wipe: the primary and backup rows are still there.
        assert!(!wiped_the_right_rows(SNAPSHOT.as_bytes()));
        let wiped = SNAPSHOT.replace(
            r#"[{"pool": "primary", "state": "committed"}, {"pool": "backup"}]"#,
            "[]",
        );
        assert!(wiped_the_right_rows(wiped.as_bytes()));
        // A wipe that left any pool's rows - the backup ones included -
        // fails, and so does one that zeroed the metered month.
        assert!(!wiped_the_right_rows(
            SNAPSHOT
                .replace(r#"{"pool": "primary", "state": "committed"}, "#, "")
                .as_bytes()
        ));
        assert!(!wiped_the_right_rows(
            wiped.replace("\"used\": 7", "\"used\": 0").as_bytes()
        ));
        // A body that is not a snapshot at all is the same failure.
        assert!(!wiped_the_right_rows(b"<html>"));
        assert!(!wiped_the_right_rows(br#"{"ops": []}"#));
    }

    #[test]
    fn the_reconcile_report_and_the_rebuilt_rows_are_detected() {
        assert!(added_anything(br#"{"added":["blobs/sha256/aa"]}"#));
        assert!(!added_anything(br#"{"added":[]}"#));
        assert!(!added_anything(br#"{"ok":true}"#));
        assert!(has_committed_primary(SNAPSHOT.as_bytes()));
        assert!(!has_committed_primary(
            SNAPSHOT.replace("committed", "pending").as_bytes()
        ));
    }

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

    #[test]
    fn the_key_value_rows_collapse_to_the_recorded_pairs() {
        let rows = vec![
            serde_json::from_str(r#"{"key":"last_backup_key","value":"d1/2026-08-05.sql"}"#)
                .unwrap(),
            serde_json::from_str(r#"{"key":"last_backup_at","value":"2026-08-05T03:00:00Z"}"#)
                .unwrap(),
        ];
        let recorded = key_values(&rows);
        assert_eq!(
            recorded.get("last_backup_key").map(String::as_str),
            Some("d1/2026-08-05.sql")
        );
        assert!(is_timestamp(recorded.get("last_backup_at").unwrap()));
    }
}
