//! The run's local state and its four long-running children: the
//! migrations and token seeding, the export-API and GitHub mocks, the
//! `.dev.vars` the worker reads, and the two `wrangler dev`
//! instances - plus the teardown they all hang off.
//!
//! Signal path.  `Drop` does not run on SIGINT, so [`DevServers`]
//! splits the teardown: `xtask_ci::arm_teardown` kills the process
//! groups and restores `.dev.vars` before `_exit`, but the D1
//! `service_mode` reset is not on that path: spawning wrangler from a
//! signal handler is not async-signal-safe.  A run
//! killed inside a breaker leg therefore leaves the pinned mode behind
//! in the local database, and [`seed_tokens`] normalizes it on the next
//! tokened run.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};
use tempfile::{NamedTempFile, TempDir, TempPath};
use xtask_registry_admin::{output, registry_dir, results, status, wrangler};

use crate::bytes::sha256_hex;
use crate::step;

/// The mock servers, in JavaScript: they are test doubles of the D1
/// export API and of GitHub, not repository automation.
const MOCK_JS: &str = include_str!("../assets/mock.js");
const GITHUB_MOCK_JS: &str = include_str!("../assets/github-mock.js");

const DEV_POLLS: u32 = 300;
const DEV_INTERVAL: Duration = Duration::from_secs(1);
const MOCK_POLLS: u32 = 20;
const PORT_FREE_POLLS: u32 = 30;
const HALF_SECOND: Duration = Duration::from_millis(500);
/// Wrangler relays the Worker's console output on its own channel, so
/// the line explaining a refused request lands behind the response
/// (`await_log` polls for the same reason).
const LOG_SETTLE: Duration = Duration::from_secs(2);

/// The four ports, as strings: they go into URLs and into `.dev.vars`
/// unparsed, so a junk value reaches wrangler and fails there.
#[derive(Clone)]
pub struct Ports {
    pub registry: String,
    pub web: String,
    pub mock: String,
    pub github: String,
}

impl Ports {
    pub fn from_env() -> Self {
        Self {
            registry: var("SMOKE_PORT", "8787"),
            web: var("SMOKE_WEB_PORT", "8789"),
            mock: var("SMOKE_MOCK_PORT", "8788"),
            github: var("SMOKE_GITHUB_PORT", "8790"),
        }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.registry)
    }

    /// The website role, a second `wrangler dev` emulating the
    /// website origin's hostname over the same local D1/R2 state.
    pub fn web_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.web)
    }
}

/// `${NAME:-default}`: an empty value takes the default too.
fn var(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// Wrangler's output stays on the operator's terminal: the first run
/// applies every migration and the list is the only progress the run
/// shows for a while.
///
/// # Errors
///
/// If wrangler cannot be run, or refuses the migrations.
pub fn apply_migrations() -> Result<()> {
    step("applying migrations to the local database");
    status(&mut wrangler(&[
        "d1",
        "migrations",
        "apply",
        "DB",
        "--local",
    ]))
}

/// `wrangler d1 execute DB --local --command <sql>`, with wrangler's
/// own result table left on the operator's terminal; the quiet and
/// JSON variants below capture it instead.
pub(crate) fn d1(sql: &str) -> Result<()> {
    status(&mut d1_execute(&["--command", sql]))
}

/// The same with stdout captured and dropped.
pub(crate) fn d1_quiet(sql: &str) -> Result<()> {
    output(&mut d1_execute(&["--command", sql]))?;
    Ok(())
}

/// `wrangler d1 execute DB --local --json --command <sql>`, stdout
/// captured.
pub(crate) fn d1_json(sql: &str) -> Result<String> {
    output(&mut d1_execute(&["--json", "--command", sql]))
}

/// The `results` rows of a `--json` read.
pub(crate) fn d1_rows(sql: &str) -> Result<Vec<Map<String, Value>>> {
    results(&d1_json(sql)?)
}

fn d1_execute(tail: &[&str]) -> Command {
    let mut arguments = vec!["d1", "execute", "DB", "--local"];
    arguments.extend_from_slice(tail);
    wrangler(&arguments)
}

/// The fixture rows are cleared so re-runs still see a first publish
/// and a first claim; the content-addressed R2 blob may survive, which
/// the publish path skips.
///
/// # Errors
///
/// If wrangler cannot be run, or the statements fail.
pub fn seed_tokens(
    token: &str,
    verify_token: &str,
    noverify_token: &str,
    outsider_token: &str,
) -> Result<()> {
    step("seeding the smoke tokens and fixtures into the local database");
    let sql = seed_sql(
        &sha256_hex(token.as_bytes()),
        &sha256_hex(verify_token.as_bytes()),
        &sha256_hex(noverify_token.as_bytes()),
        &sha256_hex(outsider_token.as_bytes()),
    );
    d1(&sql)
}

/// The seeded identities mirror first sign-ins after the migration's
/// own seed (registry user 1 is the pre-promoted operator, which the
/// fixtures must stay off so the smoke user keeps the default quota
/// class): GitHub account 0 is the claiming user (registry user 2),
/// account 2 ('friend', registry user 3) exists so membership
/// management has an account to add.  The scopes the claiming user
/// works with ('smoke', 'smokeorg', 'denyorg') are claimed through the
/// real flow against the GitHub mock - only 'foreign' stays a seeded
/// fixture, because it must belong to somebody else (user 3):
/// publishing there must be exactly as forbidden as the unclaimed
/// 'ghost'.
fn seed_sql(hash: &str, verify_hash: &str, noverify_hash: &str, outsider_hash: &str) -> String {
    format!(
        "
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (2, '1970-01-01T00:00:00.000Z');
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (3, '1970-01-01T00:00:00.000Z');
    -- The publish bucket lives on the user row OR IGNORE keeps; clear it
    -- so a rerun after an aborted run starts on a full burst.
    UPDATE users SET rl_tokens = NULL, rl_updated_at = NULL WHERE id = 2;
    -- OR REPLACE, and the 'foreign' membership cleared first: local
    -- state seeded before the migration's operator seed bound these
    -- accounts to users 1/2, and a rerun must rebind them, not keep
    -- the fossil rows beside the shifted ids.
    INSERT OR REPLACE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '0', 'smoke', 2);
    INSERT OR REPLACE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '2', 'friend', 3);
    INSERT OR IGNORE INTO scopes (name, proof_provider, proof_account_id, claimed_at)
      VALUES ('foreign', 'github', '2', '1970-01-01T00:00:00.000Z');
    DELETE FROM scope_members WHERE scope_name = 'foreign';
    INSERT INTO scope_members (scope_name, user_id, role)
      VALUES ('foreign', 3, 'owner');
    -- The first three credentials wear the only shapes the schema still
    -- admits, timestamped at seeding so every run stays inside the
    -- one-day expiry ceiling: the publisher is a login-session row
    -- seeded with the operator's scope set (the mint itself gives user
    -- 2 no verify), the verifier a trustpub verify-arm
    -- row, and the no-verify probe subject a trustpub publish-arm row
    -- confined to the 'smoke' scope - the shape a CI workflow holds,
    -- which must never see the admin plane.
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind)
      VALUES ('smoke', 2, 'smoke', '{hash}', 'publish,yank,verify',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'session');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind)
      VALUES ('smoke-verify', 2, 'smoke-verify', '{verify_hash}', 'verify',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'trustpub');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind, scope_limit, quota_class)
      VALUES ('smoke-noverify', 2, 'smoke-noverify', '{noverify_hash}', 'publish',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'trustpub',
              'smoke', 'default');
    -- A live session for 'friend' (user 3, github id 2), whom
    -- ALLOWED_GITHUB_IDS does not admit: the private alpha must refuse
    -- a valid token held by an account that is not on the allowlist.
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind)
      VALUES ('smoke-outsider', 3, 'smoke-outsider', '{outsider_hash}', 'publish,yank',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'session');
    -- The browser sessions the run hand-seals cookies for: github id
    -- 0's main one every session leg rides and the one the logout leg
    -- revokes (OR REPLACE revives it on a rerun), and the ghost - a live
    -- session for id 1, which has no identity row.
    INSERT OR REPLACE INTO browser_sessions (id, provider_account_id, expires_at)
      VALUES ('smoke', '0', CAST(strftime('%s', 'now') AS INTEGER) + 3600),
             ('smoke-logout', '0', CAST(strftime('%s', 'now') AS INTEGER) + 3600),
             ('ghost', '1', CAST(strftime('%s', 'now') AS INTEGER) + 3600);
    DELETE FROM revisions WHERE scope = 'smoke';
    DELETE FROM versions WHERE scope = 'smoke';
    DELETE FROM packages WHERE scope = 'smoke';
    -- The deletes above bypass the publish batches that keep the quota
    -- tallies exact, so recompute them (registry/docs/runbook.md, \"Quota tallies\").
    UPDATE users SET
      package_count = (SELECT COUNT(*) FROM packages WHERE created_by = users.id),
      stored_bytes = (SELECT COALESCE(SUM(archive_size), 0) FROM revisions
                      WHERE published_by = users.id AND verification != 'rejected');
    DELETE FROM scope_members WHERE scope_name IN
      ('smoke', 'smokeorg', 'denyorg', 'imposterorg', 'swaporg', 'statedrift',
       'core', 'sm0keorg');
    DELETE FROM scopes WHERE name IN
      ('smoke', 'smokeorg', 'denyorg', 'imposterorg', 'swaporg', 'statedrift',
       'core', 'sm0keorg');
    -- Fixture reset only: in production the claim history is
    -- append-only, so a released scope never restores claim capacity.
    -- Without this, run 2's re-claims would trip the lifetime limit.
    DELETE FROM scope_claims WHERE scope_name IN
      ('smoke', 'smokeorg', 'denyorg', 'imposterorg', 'swaporg', 'statedrift',
       'core', 'sm0keorg');
    DELETE FROM meta WHERE key IN ('last_backup_at', 'last_backup_key');
    DELETE FROM backup_pending;
    -- A prior run that failed inside a breaker leg leaves its pinned
    -- mode behind; normalize so re-runs never start blocked.
    UPDATE meta SET value = 'normal' WHERE key = 'service_mode';"
    )
}

/// The backup-cron leg drives the worker's dump job against
/// a local mock of the D1 export API serving a dump exported from the
/// local database right here, so the job's polling, streaming,
/// validation and bookkeeping all run for real without touching
/// Cloudflare.
///
/// # Errors
///
/// If wrangler cannot be run, or the export fails.
pub fn export_dump(mock_dir: &Path) -> Result<()> {
    step("exporting a local dump for the export-API mock");
    let dump = mock_dir.join("dump.sql");
    let dump = dump.to_str().context("the mock directory is not UTF-8")?;
    status(&mut wrangler(&[
        "d1", "export", "DB", "--local", "--output", dump,
    ]))
}

/// Restores that dump into a scratch database and requires the schema
/// and every table's rows to match the database it was exported from:
/// the backup cron leg proves the stored backup is this dump, and this
/// proves the dump restores.  Nothing writes in between - the servers
/// are not up yet.  Rows compare as wrangler's JSON renders them, which
/// cannot tell a BLOB from the TEXT spelling its bytes, or integers past
/// 2^53 apart; the registry stores neither.
///
/// # Errors
///
/// If wrangler cannot be run, or the restored database differs.
pub fn rehearse_restore(mock_dir: &Path) -> Result<()> {
    step("restoring the exported dump into a scratch database");
    let scratch = tempfile::tempdir().context("creating the scratch database directory")?;
    let persist = scratch
        .path()
        .to_str()
        .context("the scratch directory is not UTF-8")?;
    let dump = mock_dir.join("dump.sql");
    let dump = dump.to_str().context("the mock directory is not UTF-8")?;
    output(&mut d1_execute(&["--persist-to", persist, "--file", dump]))?;

    // `sqlite_sequence` included: the export restores its rows too.
    let tables = d1_rows(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT GLOB '_cf_*' ORDER BY name",
    )?;
    let tables = tables
        .iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .context("a table row has no name")
        })
        .collect::<Result<Vec<_>>>()?;
    // SQLite's own objects and D1's are rebuilt, not restored.
    let mut statements = vec![
        "SELECT type, name, sql FROM sqlite_master \
         WHERE name NOT GLOB 'sqlite_*' AND name NOT GLOB '_cf_*' ORDER BY type, name"
            .to_owned(),
    ];
    statements.extend(
        tables
            .iter()
            .map(|table| format!("SELECT * FROM \"{table}\"")),
    );
    let query = statements.join("; ");
    let live = result_sets(&d1_json(&query)?)?;
    let restored = result_sets(&output(&mut d1_execute(&[
        "--persist-to",
        persist,
        "--json",
        "--command",
        &query,
    ]))?)?;
    ensure!(
        live.len() == statements.len() && restored.len() == statements.len(),
        "wrangler answered {} and {} of {} statements",
        live.len(),
        restored.len(),
        statements.len(),
    );
    let labels = std::iter::once("the schema").chain(tables.iter().copied());
    for (label, (live, restored)) in labels.zip(live.iter().zip(&restored)) {
        ensure!(
            live == restored,
            "the restored dump differs from its database in {label}"
        );
    }
    Ok(())
}

/// Every statement's `results`, where [`results`] reads the first.
fn result_sets(json: &str) -> Result<Vec<Value>> {
    let parsed: Vec<Value> = serde_json::from_str(json).context("parse wrangler output")?;
    parsed
        .into_iter()
        .map(|set| {
            set.get("results")
                .cloned()
                .context("wrangler output has no results array")
        })
        .collect()
}

/// The website role's worker.  One deployed Worker serves both
/// hostnames, so this one only hands each request, Host header and all,
/// to the registry role's worker through wrangler's dev registry: one
/// workerd then opens the governor's SQLite, D1 and R2, where two over
/// the same files fail writes with `database is locked` whenever they
/// overlap.
const WEB_WORKER: &str =
    "export default { fetch: (request, env) => env.REGISTRY.fetch(request) };\n";

/// [`WEB_WORKER`]'s config; the service is `wrangler.jsonc`'s worker.
const WEB_CONFIG: &str = r#"{
  "name": "cabin-registry-smoke-web",
  "main": "index.js",
  "compatibility_date": "2026-07-09",
  "services": [{ "binding": "REGISTRY", "service": "cabin-registry" }]
}
"#;

fn write_web_worker(dir: &Path) -> Result<()> {
    fs::write(dir.join("index.js"), WEB_WORKER).context("write the website-role worker")?;
    fs::write(dir.join("wrangler.json"), WEB_CONFIG).context("write the website-role config")
}

/// The four servers, their logs, and the `.dev.vars` they are
/// configured by - one owner, so one teardown orders the cleanup
/// across them.
pub struct DevServers {
    ports: Ports,
    /// `None` once killed and reaped: a reaped pid is recyclable, so
    /// nothing may signal it again.
    registry: Option<std::process::Child>,
    web: Option<std::process::Child>,
    mock: Option<std::process::Child>,
    github: Option<std::process::Child>,
    /// Real files, not pipes: later legs take `wc -l` watermarks of
    /// these and `tail -n +N` them while wrangler is still appending.
    dev_log: NamedTempFile,
    web_log: NamedTempFile,
    dev_vars: PathBuf,
    dev_vars_created: bool,
    /// Kept beside `.dev.vars` rather than in `$TMPDIR`: the signal
    /// path restores it with `rename(2)`, which cannot cross a
    /// filesystem (`registry/.gitignore` covers `/.dev.vars*`).
    dev_vars_backup: Option<TempPath>,
    /// The website role's forwarding worker (`WEB_WORKER`).
    web_worker: TempDir,
    /// This run's wrangler dev registry, through which the website
    /// role reaches the registry role's worker; per run, so no other
    /// local `wrangler dev` answers for that worker.
    dev_registry: TempDir,
}

impl DevServers {
    /// Starts both mocks, writes `.dev.vars`, refuses a stale server on
    /// either dev port, then starts both `wrangler dev` instances.
    ///
    /// # Errors
    ///
    /// If a child cannot be spawned, exits during its readiness poll,
    /// never answers, if `.dev.vars` cannot be written, or if
    /// something already serves `/healthz` on a dev port.  A partial
    /// start tears down what it did start, through [`Drop`].
    pub fn start(ports: &Ports, mock_dir: &Path) -> Result<Self> {
        // Idempotent, and `spawn_tracked` requires it: arming here
        // rather than trusting the caller keeps the very first child
        // covered.
        xtask_ci::arm_teardown();
        let mut servers = Self {
            ports: ports.clone(),
            registry: None,
            web: None,
            mock: None,
            github: None,
            dev_log: NamedTempFile::new().context("create the dev-server log")?,
            web_log: NamedTempFile::new().context("create the website-role log")?,
            dev_vars: registry_dir().join(".dev.vars"),
            dev_vars_created: false,
            dev_vars_backup: None,
            web_worker: TempDir::new().context("create the website-role worker directory")?,
            dev_registry: TempDir::new().context("create the wrangler dev registry")?,
        };
        // Every `?` from here drops `servers`, which is the teardown.
        servers.start_mocks(mock_dir)?;
        servers.write_dev_vars()?;
        write_web_worker(servers.web_worker.path())?;
        servers.refuse_stale_ports()?;
        servers
            .start_dev_servers()
            .inspect_err(|_| servers.dump_log_tails())?;
        Ok(servers)
    }

    fn start_dev_servers(&mut self) -> Result<()> {
        step(&format!(
            "starting wrangler dev on port {} (first build takes a while)",
            self.ports.registry
        ));
        self.start_registry_dev()?;
        step(&format!(
            "starting the website-role wrangler dev on port {}",
            self.ports.web
        ));
        self.start_web_dev()
    }

    /// The registry-role instance.  `spawn_tracked` gives the tree its
    /// own process group, so the teardown kills wrangler and workerd
    /// together instead of orphaning them on a bound port.
    ///
    /// # Errors
    ///
    /// If the child cannot be spawned, exits early, or never answers
    /// `/healthz`.
    pub fn start_registry_dev(&mut self) -> Result<()> {
        let mut command = wrangler(&["dev", "--port", &self.ports.registry, "--test-scheduled"]);
        command.env("WRANGLER_REGISTRY_PATH", self.dev_registry.path());
        log_to(&mut command, self.dev_log.path())?;
        self.registry = Some(xtask_ci::spawn_tracked(&mut command).context("start wrangler dev")?);
        let url = format!("{}/healthz", self.ports.base());
        await_dev(
            &mut self.registry,
            "wrangler dev exited early",
            "wrangler dev never answered /healthz",
            || alive(&url),
        )
    }

    /// The website-role instance: a forwarder into the registry role's
    /// worker (`WEB_WORKER`), with wrangler pinning its emulated Host
    /// header to cabinpkg.com, which is what flips the Worker's role
    /// dispatch.
    /// `/healthz` only exists on the registry role, so any HTTP status
    /// at all (the website role answers it 401/404) proves the
    /// instance is up.
    ///
    /// # Errors
    ///
    /// If the child cannot be spawned, exits early, or never answers.
    pub fn start_web_dev(&mut self) -> Result<()> {
        let config = self.web_worker.path().join("wrangler.json");
        let config = config
            .to_str()
            .context("the scratch directory is not UTF-8")?;
        let mut command = wrangler(&[
            "dev",
            "--config",
            config,
            "--port",
            &self.ports.web,
            "--host",
            "cabinpkg.com",
        ]);
        command.env("WRANGLER_REGISTRY_PATH", self.dev_registry.path());
        log_to(&mut command, self.web_log.path())?;
        self.web = Some(
            xtask_ci::spawn_tracked(&mut command).context("start the website-role wrangler dev")?,
        );
        let url = format!("{}/healthz", self.ports.web_base());
        await_dev(
            &mut self.web,
            "the website-role wrangler dev exited early",
            "the website-role wrangler dev never answered",
            || probe(&url).is_some(),
        )
    }

    /// Both dev servers down and reaped, then up to 30 × 0.5 s until
    /// `/healthz` stops answering.  The caller mutates `.dev.vars`
    /// between this and the restarts, which is the whole point of the
    /// two mid-run restarts.
    pub fn stop_dev_servers(&mut self) {
        let mut registry = self.registry.take();
        let mut web = self.web.take();
        // Reaped before the port poll, and before the pid could be
        // recycled: a later teardown must not signal an unrelated
        // group.
        xtask_ci::terminate([registry.as_mut(), web.as_mut()].into_iter().flatten());
        let url = format!("{}/healthz", self.ports.base());
        for _ in 0..PORT_FREE_POLLS {
            if !alive(&url) {
                break;
            }
            std::thread::sleep(HALF_SECOND);
        }
    }

    /// Appends to `.dev.vars`.
    ///
    /// # Errors
    ///
    /// If the file cannot be appended to.
    pub fn append_dev_vars(&self, extra: &str) -> Result<()> {
        append(&self.dev_vars, extra)
    }

    /// Replace a key rather than append a second copy: a stale duplicate
    /// is a foot-gun for the next reader.
    /// `drop_prefix` is matched anchored at the start of a line.
    ///
    /// # Errors
    ///
    /// If the file cannot be read, rewritten or appended to.
    pub fn rewrite_dev_vars_key(&self, drop_prefix: &str, extra: &str) -> Result<()> {
        rewrite_key(&self.dev_vars, drop_prefix, extra)
    }

    /// The registry-role log, for the legs that watermark it with
    /// `wc -l` and tail it from there.
    pub fn dev_log(&self) -> &Path {
        self.dev_log.path()
    }

    /// The website-role log.
    pub fn web_log(&self) -> &Path {
        self.web_log.path()
    }

    /// Both dev-server log tails on stderr, for a wrangler start or a
    /// leg that failed: the Worker's reason is only ever in here.
    pub fn dump_log_tails(&self) {
        std::thread::sleep(LOG_SETTLE);
        for (role, log) in [("registry", self.dev_log()), ("website", self.web_log())] {
            eprintln!("--- {role}-role wrangler dev log, last {LOG_TAIL} bytes ---");
            dump(log);
        }
    }

    fn start_mocks(&mut self, mock_dir: &Path) -> Result<()> {
        step(&format!(
            "starting the export-API mock on port {}",
            self.ports.mock
        ));
        let script = mock_dir.join("mock.js");
        fs::write(&script, MOCK_JS).context("write the export-API mock")?;
        let log = mock_dir.join("mock.log");
        let mut command = Command::new("node");
        command
            .arg(&script)
            .arg(&self.ports.mock)
            .arg(mock_dir.join("dump.sql"));
        log_to(&mut command, &log)?;
        self.mock = Some(xtask_ci::spawn_tracked(&mut command).context("start the export mock")?);
        let url = format!("http://127.0.0.1:{}/dump.sql", self.ports.mock);
        await_mock(
            &mut self.mock,
            &log,
            "the export-API mock exited early",
            || alive(&url),
        )?;

        step(&format!(
            "starting the GitHub mock on port {}",
            self.ports.github
        ));
        let script = mock_dir.join("github-mock.js");
        fs::write(&script, GITHUB_MOCK_JS).context("write the GitHub mock")?;
        let log = mock_dir.join("github-mock.log");
        let mut command = Command::new("node");
        command.arg(&script).arg(&self.ports.github);
        log_to(&mut command, &log)?;
        self.github = Some(xtask_ci::spawn_tracked(&mut command).context("start the GitHub mock")?);
        // API reads without a bearer token answer 401 like GitHub, so
        // the refusal is the readiness signal.
        let url = format!("http://127.0.0.1:{}/user", self.ports.github);
        await_mock(
            &mut self.github,
            &log,
            "the GitHub mock exited early",
            || probe(&url) == Some(401),
        )
    }

    /// Wrangler reads `.dev.vars` for `wrangler dev`; an existing file
    /// is saved and restored.
    ///
    /// `SESSION_SECRET` is pinned so the session-plane leg can mint
    /// valid session cookies for the seeded `browser_sessions` rows
    /// without a GitHub round trip; `ALLOWED_GITHUB_IDS` admits the
    /// seeded user (github id 0) plus id 1, whose session row exists but
    /// whose identity row deliberately does not (the post-wipe
    /// ghost-session case), and ids 3 and 4, the sign-in
    /// leg's fresh accounts (too young to sign up, and old enough -
    /// only id 4 ever gets an identity row).  `SERVICE_MODE_TTL_SECS=0`
    /// disables the service-mode cache so the breaker leg observes a
    /// flipped mode immediately (the deployed worker uses the in-code
    /// 60 s TTL), and `STATS_CACHE_TTL_SECS=0` disables the stats edge
    /// cache so the download-count leg observes a fresh count
    /// (deployed: 300 s), with `DOWNLOAD_FLUSH_INTERVAL_MS=0` flushing
    /// every buffered download count immediately for the same reason
    /// (deployed: 30 s batches).  `PACKAGE_CACHE_TTL_SECS=0` disables
    /// the package-document edge cache so a verdict or yank shows in
    /// the next read (deployed: 60 s).  The `GITHUB_*` entries point the claim
    /// flow's server-side calls and the verdict endpoint's JWKS fetch
    /// at the GitHub mock (the client secret only has to exist for the
    /// mock exchange).  `VERIFIER_BACKING_ACCOUNT_ID` overrides the
    /// deployed operator id with the seeded account 0, so the
    /// exchange's verifier arm mints against registry user 2.
    fn write_dev_vars(&mut self) -> Result<()> {
        if self.dev_vars.exists() {
            let backup = tempfile::Builder::new()
                .prefix(".dev.vars.backup")
                .tempfile_in(registry_dir())
                .context("back up .dev.vars")?
                .into_temp_path();
            fs::copy(&self.dev_vars, &backup).context("back up .dev.vars")?;
            // Armed before the write, so a signal in between still
            // puts the operator's file back.
            xtask_ci::restore_on_teardown(Some(&backup), &self.dev_vars);
            self.dev_vars_backup = Some(backup);
        } else {
            xtask_ci::restore_on_teardown(None, &self.dev_vars);
        }
        fs::write(&self.dev_vars, render_dev_vars(&self.ports)).context("write .dev.vars")?;
        self.dev_vars_created = true;
        Ok(())
    }

    /// A stale server on either port would silently answer the checks
    /// below in place of the instances started here.  The mock ports
    /// deliberately get no such guard.
    fn refuse_stale_ports(&self) -> Result<()> {
        for port in [&self.ports.registry, &self.ports.web] {
            if alive(&format!("http://127.0.0.1:{port}/healthz")) {
                bail!("something is already serving on port {port}; kill it first");
            }
        }
        Ok(())
    }
}

impl Drop for DevServers {
    fn drop(&mut self) {
        // A failure inside a breaker leg would otherwise leave the
        // pinned mode behind in the local D1 state, blocking unrelated
        // local work until the next run's seeding normalizes it.  Best
        // effort, both streams discarded.
        let _ = wrangler(&[
            "d1",
            "execute",
            "DB",
            "--local",
            "--command",
            "UPDATE meta SET value = 'normal' WHERE key = 'service_mode';
     UPDATE meta SET value = '' WHERE key = 'service_mode_reason';",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
        // The whole process group: killing the direct child alone
        // would orphan wrangler/workerd and leave the port bound.
        // The mocks are single-process groups, which kill identically.
        xtask_ci::terminate(
            [
                &mut self.registry,
                &mut self.web,
                &mut self.mock,
                &mut self.github,
            ]
            .into_iter()
            .filter_map(Option::as_mut),
        );
        if self.dev_vars_created {
            let _ = fs::remove_file(&self.dev_vars);
        }
        if let Some(backup) = self.dev_vars_backup.take() {
            let _ = fs::rename(&backup, &self.dev_vars);
        }
        // The file is back; the signal path must not restore it again
        // over whatever comes next.
        xtask_ci::teardown_restore_done();
    }
}

fn render_dev_vars(ports: &Ports) -> String {
    let Ports { mock, github, .. } = ports;
    format!(
        "CF_API_BASE=\"http://127.0.0.1:{mock}\"
D1_EXPORT_API_TOKEN=\"smoke-placeholder\"
SESSION_SECRET=\"smoke-session-secret-not-for-production\"
ALLOWED_GITHUB_IDS=\"0,1,3,4\"
SERVICE_MODE_TTL_SECS=\"0\"
STATS_CACHE_TTL_SECS=\"0\"
PACKAGE_CACHE_TTL_SECS=\"0\"
DOWNLOAD_FLUSH_INTERVAL_MS=\"0\"
GITHUB_OAUTH_BASE=\"http://127.0.0.1:{github}\"
GITHUB_API_BASE=\"http://127.0.0.1:{github}\"
GITHUB_JWKS_URL=\"http://127.0.0.1:{github}/.well-known/jwks\"
GITHUB_CLIENT_SECRET=\"smoke-client-secret\"
VERIFIER_BACKING_ACCOUNT_ID=\"0\"
"
    )
}

fn append(path: &Path, extra: &str) -> Result<()> {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("append to {}", path.display()))?
        .write_all(extra.as_bytes())
        .with_context(|| format!("append to {}", path.display()))
}

fn rewrite_key(path: &Path, drop_prefix: &str, extra: &str) -> Result<()> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let kept: String = contents
        .lines()
        .filter(|line| !line.starts_with(drop_prefix))
        .fold(String::with_capacity(contents.len()), |mut kept, line| {
            kept.push_str(line);
            kept.push('\n');
            kept
        });
    // `.dev.vars.next`, not a `.next` extension: the file is
    // `.dev.vars`, whose extension is `vars`.
    let mut next = path.as_os_str().to_owned();
    next.push(".next");
    let next = PathBuf::from(next);
    fs::write(&next, kept).with_context(|| format!("write {}", next.display()))?;
    fs::rename(&next, path).with_context(|| format!("replace {}", path.display()))?;
    append(path, extra)
}

/// Both output streams into a fresh, truncated log per start - the
/// mid-run restarts reuse the same path, and the legs that watermark it
/// with `wc -l` count from the restart's first line.
///
/// stdin is `/dev/null`: nothing here ever writes to a child's stdin,
/// and a backgrounded wrangler that decides it owns a TTY would be
/// stopped by SIGTTIN in its own process group.
fn log_to(command: &mut Command, log: &Path) -> Result<()> {
    let file = File::create(log).with_context(|| format!("create {}", log.display()))?;
    let errors = file.try_clone().context("redirect stderr to the log")?;
    command.stdout(file).stderr(errors).stdin(Stdio::null());
    Ok(())
}

/// One probe's verdict: the HTTP status if the server answered at
/// all, `None` if it did not.  A fresh agent per probe.
fn probe(url: &str) -> Option<u16> {
    match ureq::AgentBuilder::new()
        .redirects(0)
        .build()
        .get(url)
        .call()
    {
        Ok(response) => Some(response.status()),
        Err(ureq::Error::Status(status, _)) => Some(status),
        Err(ureq::Error::Transport(_)) => None,
    }
}

/// Any status below 400, so an unfollowed redirect counts as alive.
fn alive(url: &str) -> bool {
    matches!(probe(url), Some(status) if status < 400)
}

/// A liveness check done with a peek that never reaps: `try_wait` would
/// reap the child and leave its recyclable pid in the teardown table
/// until the `xtask_ci::reap` below cleared it - the exact window the
/// `WNOWAIT` design exists to close.  `waitid(WNOWAIT | WNOHANG)`
/// observes an exit while the child stays a zombie, whose pid the
/// kernel cannot recycle; the real reap then runs table-safely.
fn exited(child: &mut Option<std::process::Child>) -> bool {
    let Some(running) = child.as_mut() else {
        return true;
    };
    if !exited_no_reap(running) {
        return false;
    }
    if let Some(mut dead) = child.take() {
        let _ = xtask_ci::reap(&mut dead);
    }
    true
}

#[cfg(unix)]
fn exited_no_reap(child: &mut std::process::Child) -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let found = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &raw mut info,
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    };
    if found != 0 {
        // A failed probe reads as an exit, whatever errno says: the
        // caller dumps the log and issues the server-named diagnostic,
        // never a generic error.
        return true;
    }
    // With WNOHANG, a zero si_pid means "still running".
    (unsafe { info.si_pid() }) != 0
}

/// Off Unix there is no teardown table for a reap to race, so the
/// reaping probe is harmless.
#[cfg(not(unix))]
fn exited_no_reap(child: &mut std::process::Child) -> bool {
    // An errored probe reads as an exit, as on Unix.
    child.try_wait().map_or(true, |state| state.is_some())
}

/// The log on stderr, bounded: a boot log fits whole, a full run's
/// request lines would swamp a CI log, and the exit or the refused
/// request is at the end either way.
const LOG_TAIL: usize = 64 * 1024;

fn dump(log: &Path) {
    if let Ok(text) = fs::read(log) {
        let _ = std::io::stderr().write_all(&text[text.len().saturating_sub(LOG_TAIL)..]);
    }
}

/// The dev-server readiness loop: 300 × 1 s, with the liveness check
/// before every probe, so a child that died fails at once instead of
/// after five minutes of polling a dead port.  The logs are printed by
/// whoever owns the failure (`start`, `run`).
fn await_dev(
    child: &mut Option<std::process::Child>,
    exited_early: &str,
    never_answered: &str,
    mut ready: impl FnMut() -> bool,
) -> Result<()> {
    for _ in 0..DEV_POLLS {
        if exited(child) {
            bail!("{exited_early}");
        }
        if ready() {
            return Ok(());
        }
        std::thread::sleep(DEV_INTERVAL);
    }
    bail!("{never_answered}")
}

/// The mock readiness loop: 20 × 0.5 s, and deliberately no timeout
/// arm - an unready mock fails at the first leg that needs it.
fn await_mock(
    child: &mut Option<std::process::Child>,
    log: &Path,
    exited_early: &str,
    mut ready: impl FnMut() -> bool,
) -> Result<()> {
    for _ in 0..MOCK_POLLS {
        if exited(child) {
            dump(log);
            bail!("{exited_early}");
        }
        if ready() {
            return Ok(());
        }
        std::thread::sleep(HALF_SECOND);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports() -> Ports {
        Ports {
            registry: "8787".into(),
            web: "8789".into(),
            mock: "8788".into(),
            github: "8790".into(),
        }
    }

    /// The append and the replace-then-append, over the file the run
    /// actually mutates: the second must leave exactly one
    /// `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH`, the last-written one.
    #[test]
    fn the_dev_vars_mutations_leave_one_value_per_key() {
        let dir = tempfile::tempdir().unwrap();
        let vars = dir.path().join(".dev.vars");
        fs::write(&vars, render_dev_vars(&ports())).unwrap();

        append(
            &vars,
            "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"1\"\n\
             GOVERNOR_STORAGE_PRIMARY_BYTES=\"1\"\n\
             GOVERNOR_R2_CLASS_B_SOURCE_MONTH=\"0\"\n",
        )
        .unwrap();
        rewrite_key(
            &vars,
            "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=",
            "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"42\"\n",
        )
        .unwrap();

        let written = fs::read_to_string(&vars).unwrap();
        let governor: Vec<&str> = written
            .lines()
            .filter(|line| line.starts_with("GOVERNOR_R2_CLASS_B_ORDINARY_MONTH="))
            .collect();
        assert_eq!(governor, ["GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"42\""]);
        assert!(written.starts_with("CF_API_BASE=\"http://127.0.0.1:8788\"\n"));
        assert!(written.contains("GOVERNOR_STORAGE_PRIMARY_BYTES=\"1\"\n"));
        assert!(written.ends_with("GOVERNOR_R2_CLASS_B_ORDINARY_MONTH=\"42\"\n"));
        assert!(!dir.path().join(".dev.vars.next").exists());
    }

    #[test]
    fn the_seeding_sql_carries_the_token_hashes() {
        let sql = seed_sql(
            &sha256_hex(b"cabin_smoke"),
            &sha256_hex(b"cabin_smoke-verify"),
            &sha256_hex(b"cabin_smoke-noverify"),
            &sha256_hex(b"cabin_smoke-outsider"),
        );
        assert!(sql.starts_with("\n    INSERT OR IGNORE INTO users (id, created_at)\n"));
        assert!(sql.contains(&format!(
            "VALUES ('smoke', 2, 'smoke', '{}', 'publish,yank,verify',",
            sha256_hex(b"cabin_smoke")
        )));
        assert!(sql.contains(&format!(
            "VALUES ('smoke-verify', 2, 'smoke-verify', '{}', 'verify',",
            sha256_hex(b"cabin_smoke-verify")
        )));
        assert!(sql.contains(&format!(
            "VALUES ('smoke-noverify', 2, 'smoke-noverify', '{}', 'publish',",
            sha256_hex(b"cabin_smoke-noverify")
        )));
        assert!(sql.contains(&format!(
            "VALUES ('smoke-outsider', 3, 'smoke-outsider', '{}', 'publish,yank',",
            sha256_hex(b"cabin_smoke-outsider")
        )));
        assert!(
            sql.ends_with("\n    UPDATE meta SET value = 'normal' WHERE key = 'service_mode';")
        );
    }
}
