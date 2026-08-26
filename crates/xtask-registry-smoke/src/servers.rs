//! The run's local state and its four long-running children, ported
//! from `registry/scripts/smoke.sh`: the migrations and token seeding
//! (L199-257), the export-API and GitHub mocks (L259-401), the
//! `.dev.vars` the worker reads (L405-434), and the two `wrangler dev`
//! instances (L438-484) - plus the `trap cleanup EXIT` (L85-102) they
//! all hang off.
//!
//! Signal-path ceiling.  The shell's trap fired on SIGINT too, where
//! `Drop` does not, so [`DevServers`] splits the teardown:
//! `xtask_ci::arm_teardown` kills the process groups and restores
//! `.dev.vars` before `_exit`, but the D1 `service_mode` reset is
//! deliberately *not* on that path - spawning wrangler from a signal
//! handler is not async-signal-safe.  A run killed inside a breaker
//! leg therefore leaves the pinned mode behind in the local database,
//! and [`seed_tokens`] normalizes it on the next tokened run.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use tempfile::{NamedTempFile, TempPath};
use xtask_registry_admin::{output, registry_dir, results, status, wrangler};

use crate::bytes::sha256_hex;
use crate::step;

/// The mock servers, kept as the JavaScript the shell wrote from a
/// heredoc: they are test doubles of the D1 export API and of GitHub,
/// not repository automation, and rewriting their wire behavior in
/// Rust is not something a one-to-one port may do.  One deliberate
/// divergence from the heredoc bytes: the export mock's 404
/// diagnostic gained a `text/plain` content type (`CodeQL`
/// `js/reflected-xss` - the echoed URL became visible to scanning once
/// it left the heredoc); no smoke leg reads that body.
const MOCK_JS: &str = include_str!("../assets/mock.js");
const GITHUB_MOCK_JS: &str = include_str!("../assets/github-mock.js");

/// Every count and duration below is the shell's literally (plan
/// §7.9): in-process HTTP is faster than forking `curl`, so a poll
/// budget trimmed to "what it takes locally" turns a real negative
/// assertion into a race that passes for the wrong reason.
const DEV_POLLS: u32 = 300;
const DEV_INTERVAL: Duration = Duration::from_secs(1);
const MOCK_POLLS: u32 = 20;
const PORT_FREE_POLLS: u32 = 30;
const HALF_SECOND: Duration = Duration::from_millis(500);

/// The four ports, as strings.  The shell interpolated `$SMOKE_PORT`
/// and friends straight into URLs and into `.dev.vars` without ever
/// reading them as numbers, so a junk value reached wrangler and
/// failed there; parsing here would fail earlier and differently.
#[derive(Clone)]
pub struct Ports {
    pub registry: String,
    pub web: String,
    pub mock: String,
    pub github: String,
}

impl Ports {
    /// L59-62.
    pub fn from_env() -> Self {
        Self {
            registry: var("SMOKE_PORT", "8787"),
            web: var("SMOKE_WEB_PORT", "8789"),
            mock: var("SMOKE_MOCK_PORT", "8788"),
            github: var("SMOKE_GITHUB_PORT", "8790"),
        }
    }

    /// L63.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.registry)
    }

    /// L70: the website role, a second `wrangler dev` emulating the
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

/// L199-200.  Wrangler's output stays on the operator's terminal, as
/// it did in the shell - the first run applies every migration and the
/// list is the only progress the run shows for a while.
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
/// own result table left on the operator's terminal; the sites the
/// shell redirected or captured use the quiet and JSON variants below.
pub(crate) fn d1(sql: &str) -> Result<()> {
    status(&mut d1_execute(&["--command", sql]))
}

/// The same with `>/dev/null`: capturing stdout is how it is
/// swallowed.
pub(crate) fn d1_quiet(sql: &str) -> Result<()> {
    output(&mut d1_execute(&["--command", sql]))?;
    Ok(())
}

/// `wrangler d1 execute DB --local --json --command <sql>`, as the
/// shell piped it into `node`.
pub(crate) fn d1_json(sql: &str) -> Result<String> {
    output(&mut d1_execute(&["--json", "--command", sql]))
}

/// The `results` rows of a `--json` read, which the `node` programs
/// indexed into as `out[0].results`.
pub(crate) fn d1_rows(sql: &str) -> Result<Vec<Map<String, Value>>> {
    results(&d1_json(sql)?)
}

fn d1_execute(tail: &[&str]) -> Command {
    let mut arguments = vec!["d1", "execute", "DB", "--local"];
    arguments.extend_from_slice(tail);
    wrangler(&arguments)
}

/// L203-248.  The fixture rows are cleared so re-runs still see a
/// first publish and a first claim; the content-addressed R2 blob may
/// survive, which the publish path skips.
///
/// # Errors
///
/// If wrangler cannot be run, or the statements fail.
pub fn seed_tokens(token: &str, verify_token: &str, noverify_token: &str) -> Result<()> {
    step("seeding the smoke tokens and fixtures into the local database");
    let sql = seed_sql(
        &sha256_hex(token.as_bytes()),
        &sha256_hex(verify_token.as_bytes()),
        &sha256_hex(noverify_token.as_bytes()),
    );
    d1(&sql)
}

/// The seeded identities mirror first sign-ins: GitHub account 0 is
/// the claiming user (registry user 1), account 2 ('friend', registry
/// user 2) exists so membership management has an account to add.  The
/// scopes user 1 works with ('smoke', 'smokeorg', 'denyorg') are
/// claimed through the real flow against the GitHub mock - only
/// 'foreign' stays a seeded fixture, because it must belong to
/// somebody else (user 2): publishing there must be exactly as
/// forbidden as the unclaimed 'ghost'.
fn seed_sql(hash: &str, verify_hash: &str, noverify_hash: &str) -> String {
    format!(
        "
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (1, '1970-01-01T00:00:00.000Z');
    INSERT OR IGNORE INTO users (id, created_at)
      VALUES (2, '1970-01-01T00:00:00.000Z');
    INSERT OR IGNORE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '0', 'smoke', 1);
    INSERT OR IGNORE INTO identities (provider, provider_account_id, login_snapshot, user_id)
      VALUES ('github', '2', 'friend', 2);
    INSERT OR IGNORE INTO scopes (name, proof_provider, proof_account_id, claimed_at)
      VALUES ('foreign', 'github', '2', '1970-01-01T00:00:00.000Z');
    INSERT OR IGNORE INTO scope_members (scope_name, user_id, role)
      VALUES ('foreign', 2, 'owner');
    -- The three credentials wear the only shapes the schema still
    -- admits, timestamped at seeding so every run stays inside the
    -- one-day expiry ceiling: the publisher is a login-session row
    -- (the full human scope set), the verifier a trustpub verify-arm
    -- row, and the no-verify probe subject a trustpub publish-arm row
    -- confined to the 'smoke' scope - the shape a CI workflow holds,
    -- which must never see the admin plane.
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind)
      VALUES ('smoke', 1, 'smoke', '{hash}', 'publish,yank,verify',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'session');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind)
      VALUES ('smoke-verify', 1, 'smoke-verify', '{verify_hash}', 'verify',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'trustpub');
    INSERT OR REPLACE INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                   expires_at, kind, scope_limit, quota_class)
      VALUES ('smoke-noverify', 1, 'smoke-noverify', '{noverify_hash}', 'publish',
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+12 hours'), 'trustpub',
              'smoke', 'default');
    DELETE FROM revisions WHERE scope = 'smoke';
    DELETE FROM versions WHERE scope = 'smoke';
    DELETE FROM packages WHERE scope = 'smoke';
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

/// L254-257.  The backup-cron leg drives the worker's dump job against
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

/// The four servers, their logs, and the `.dev.vars` they are
/// configured by - one owner, because the cleanup order across them is
/// what the shell's single `cleanup` defined.
pub struct DevServers {
    ports: Ports,
    /// `None` once killed and reaped: a reaped pid is recyclable, so
    /// nothing may signal it again (the guarantee bash's job table
    /// gave the trap's kill loop).
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
}

impl DevServers {
    /// Starts both mocks, writes `.dev.vars`, refuses a stale server on
    /// either dev port, then starts both `wrangler dev` instances
    /// (L259-484).  The step banners are emitted here, in the shell's
    /// places.
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
        };
        // Every `?` from here drops `servers`, which is the trap.
        servers.start_mocks(mock_dir)?;
        servers.write_dev_vars()?;
        servers.refuse_stale_ports()?;
        step(&format!(
            "starting wrangler dev on port {} (first build takes a while)",
            servers.ports.registry
        ));
        servers.start_registry_dev()?;
        // Started second so the first instance's build is already
        // cached.
        step(&format!(
            "starting the website-role wrangler dev on port {}",
            servers.ports.web
        ));
        servers.start_web_dev()?;
        Ok(servers)
    }

    /// The registry-role instance (L446-463).  `spawn_tracked` is the
    /// shell's `set -m`: the tree gets its own process group, so the
    /// teardown kills npx, wrangler and workerd together instead of
    /// orphaning them on a bound port.
    ///
    /// # Errors
    ///
    /// If the child cannot be spawned, exits early, or never answers
    /// `/healthz`.
    pub fn start_registry_dev(&mut self) -> Result<()> {
        let mut command = wrangler(&["dev", "--port", &self.ports.registry, "--test-scheduled"]);
        log_to(&mut command, self.dev_log.path())?;
        self.registry = Some(xtask_ci::spawn_tracked(&mut command).context("start wrangler dev")?);
        let url = format!("{}/healthz", self.ports.base());
        let log = self.dev_log.path().to_path_buf();
        await_dev(
            &mut self.registry,
            &log,
            "wrangler dev exited early",
            "wrangler dev never answered /healthz",
            || alive(&url),
        )
    }

    /// The website-role instance (L465-477): same code, same local
    /// state, but wrangler pins its emulated Host header to
    /// cabinpkg.com, which is what flips the Worker's role dispatch.
    /// `/healthz` only exists on the registry role, so any HTTP status
    /// at all (the website role answers it 401/404) proves the
    /// instance is up.
    ///
    /// # Errors
    ///
    /// If the child cannot be spawned, exits early, or never answers.
    pub fn start_web_dev(&mut self) -> Result<()> {
        let mut command = wrangler(&["dev", "--port", &self.ports.web, "--host", "cabinpkg.com"]);
        log_to(&mut command, self.web_log.path())?;
        self.web = Some(
            xtask_ci::spawn_tracked(&mut command).context("start the website-role wrangler dev")?,
        );
        let url = format!("{}/healthz", self.ports.web_base());
        let log = self.web_log.path().to_path_buf();
        await_dev(
            &mut self.web,
            &log,
            "the website-role wrangler dev exited early",
            "the website-role wrangler dev never answered",
            || probe(&url).is_some(),
        )
    }

    /// Both dev servers down and reaped, then up to 30 × 0.5 s until
    /// `/healthz` stops answering (L1973-1979, L2158-2165).  The
    /// caller mutates `.dev.vars` between this and the restarts, which
    /// is the whole point of the two mid-run restarts.
    pub fn stop_dev_servers(&mut self) {
        let mut registry = self.registry.take();
        let mut web = self.web.take();
        for child in [registry.as_mut(), web.as_mut()].into_iter().flatten() {
            xtask_ci::kill_group(child);
        }
        // Reaped before the port poll, and before the pid could be
        // recycled: a later teardown must not signal an unrelated
        // group. Errors are the shell's `|| true`.
        for child in [registry.as_mut(), web.as_mut()].into_iter().flatten() {
            let _ = xtask_ci::reap(child);
        }
        let url = format!("{}/healthz", self.ports.base());
        for _ in 0..PORT_FREE_POLLS {
            if !alive(&url) {
                break;
            }
            std::thread::sleep(HALF_SECOND);
        }
    }

    /// L1980-1984: `cat >> .dev.vars`.
    ///
    /// # Errors
    ///
    /// If the file cannot be appended to.
    pub fn append_dev_vars(&self, extra: &str) -> Result<()> {
        append(&self.dev_vars, extra)
    }

    /// L2166-2172: replace a key rather than append a second copy.
    /// Wrangler takes the last value of a duplicated key, so the
    /// `grep -v` is load-bearing - without it the earlier
    /// `GOVERNOR_R2_CLASS_B_ORDINARY_MONTH="1"` would be the one that
    /// stuck and every load assertion below it would go vacuous - and
    /// a stale duplicate is a foot-gun for the next reader either way.
    /// `drop_prefix` is matched anchored, as `grep -v '^KEY='` was.
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

    /// L405-434.  Wrangler reads `.dev.vars` for `wrangler dev`; an
    /// existing file is saved and restored.
    ///
    /// `SESSION_SECRET` is pinned so the session-plane leg can mint a
    /// valid session cookie for the seeded user (github id 0) without
    /// a GitHub round trip; `ALLOWED_GITHUB_IDS` admits that id plus id
    /// 1, whose identity row deliberately does not exist (the
    /// post-wipe ghost-session case), and ids 3 and 4, the sign-in
    /// leg's fresh accounts (too young to sign up, and old enough -
    /// only id 4 ever gets an identity row).  `SERVICE_MODE_TTL_SECS=0`
    /// disables the service-mode cache so the breaker leg observes a
    /// flipped mode immediately (the deployed worker uses the in-code
    /// 60 s TTL), and `STATS_CACHE_TTL_SECS=0` disables the stats edge
    /// cache so the download-count leg observes a fresh count
    /// (deployed: 300 s), with `DOWNLOAD_FLUSH_INTERVAL_MS=0` flushing
    /// every buffered download count immediately for the same reason
    /// (deployed: 30 s batches).  The `GITHUB_*` entries point the claim
    /// flow's server-side calls and the verdict endpoint's JWKS fetch
    /// at the GitHub mock (the client secret only has to exist for the
    /// mock exchange).  `VERIFIER_BACKING_ACCOUNT_ID` overrides the
    /// deployed operator id with the seeded account 0, so the
    /// exchange's verifier arm mints against registry user 1.
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

    /// L438-442.  A stale server on either port would silently answer
    /// the checks below in place of the instances started here.  The
    /// mock ports deliberately get no such guard.
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
    /// L85-102, in order.
    fn drop(&mut self) {
        // A failure inside a breaker leg would otherwise leave the
        // pinned mode behind in the local D1 state, blocking unrelated
        // local work until the next run's seeding normalizes it.  Best
        // effort, both streams discarded, exactly as the trap's
        // `|| true` was.
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
        // would orphan npx/wrangler/workerd and leave the port bound.
        // The mocks are single-process groups, which kill identically.
        for slot in [
            &mut self.registry,
            &mut self.web,
            &mut self.mock,
            &mut self.github,
        ] {
            if let Some(mut child) = slot.take() {
                xtask_ci::kill_group(&mut child);
                let _ = xtask_ci::reap(&mut child);
            }
        }
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

/// L422-433, in the file's order.
fn render_dev_vars(ports: &Ports) -> String {
    let Ports { mock, github, .. } = ports;
    format!(
        "CF_API_BASE=\"http://127.0.0.1:{mock}\"
D1_EXPORT_API_TOKEN=\"smoke-placeholder\"
SESSION_SECRET=\"smoke-session-secret-not-for-production\"
ALLOWED_GITHUB_IDS=\"0,1,3,4\"
SERVICE_MODE_TTL_SECS=\"0\"
STATS_CACHE_TTL_SECS=\"0\"
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
    // `$dev_vars.next`, not a `.next` extension: the file is
    // `.dev.vars`, whose extension is `vars`.
    let mut next = path.as_os_str().to_owned();
    next.push(".next");
    let next = PathBuf::from(next);
    fs::write(&next, kept).with_context(|| format!("write {}", next.display()))?;
    fs::rename(&next, path).with_context(|| format!("replace {}", path.display()))?;
    append(path, extra)
}

/// `>"$log" 2>&1`: a fresh, truncated log per start - the mid-run
/// restarts reuse the same path, and the legs that watermark it with
/// `wc -l` count from the restart's first line.
///
/// stdin is `/dev/null` where the shell's background job inherited the
/// terminal: nothing here ever writes to a child's stdin, and a
/// backgrounded wrangler that decides it owns a TTY would be stopped
/// by SIGTTIN in its own process group.
fn log_to(command: &mut Command, log: &Path) -> Result<()> {
    let file = File::create(log).with_context(|| format!("create {}", log.display()))?;
    let errors = file.try_clone().context("redirect stderr to the log")?;
    command.stdout(file).stderr(errors).stdin(Stdio::null());
    Ok(())
}

/// One `curl` invocation's verdict: the HTTP status if the server
/// answered at all, `None` for what `curl` reported as `000`.  A fresh
/// agent per probe because `curl` was a fresh process per probe -
/// a pooled connection across a mid-run restart would answer for a
/// server that is gone.  No redirect following, as `curl` had no `-L`.
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

/// `curl -fsS`: `-f` fails on 400 and up, and there is no `-L`, so a
/// redirect is a success.
fn alive(url: &str) -> bool {
    matches!(probe(url), Some(status) if status < 400)
}

/// `kill -0`, done with a peek that never reaps: `try_wait` would
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
        // A failed probe WAS the shell's death signal (`kill -0`'s
        // nonzero, whatever errno says), so it takes the same path:
        // the caller dumps the log and issues the server-named
        // diagnostic, never a generic error.
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

/// `cat "$log" >&2`: the whole log, not a tail - the reason a dev
/// server exited during its first build is usually its first lines.
fn dump(log: &Path) {
    if let Ok(text) = fs::read(log) {
        let _ = std::io::stderr().write_all(&text);
    }
}

/// The dev-server readiness loop (L449-462, L471-476): 300 × 1 s, with
/// the liveness check before every probe, so a child that died fails
/// with its log instead of after five minutes of polling a dead port.
fn await_dev(
    child: &mut Option<std::process::Child>,
    log: &Path,
    exited_early: &str,
    never_answered: &str,
    mut ready: impl FnMut() -> bool,
) -> Result<()> {
    for _ in 0..DEV_POLLS {
        if exited(child) {
            dump(log);
            bail!("{exited_early}");
        }
        if ready() {
            return Ok(());
        }
        std::thread::sleep(DEV_INTERVAL);
    }
    bail!("{never_answered}")
}

/// The mock readiness loop (L281-288, L394-401): 20 × 0.5 s, and
/// deliberately no timeout arm - the shell's loop simply fell through,
/// leaving an unready mock to fail at the first leg that needs it.
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
        // shasum -a 256 of "cabin_smoke", "cabin_smoke-verify", and
        // "cabin_smoke-noverify".
        let sql = seed_sql(
            &sha256_hex(b"cabin_smoke"),
            &sha256_hex(b"cabin_smoke-verify"),
            &sha256_hex(b"cabin_smoke-noverify"),
        );
        assert!(sql.starts_with("\n    INSERT OR IGNORE INTO users (id, created_at)\n"));
        assert!(sql.contains(&format!(
            "VALUES ('smoke', 1, 'smoke', '{}', 'publish,yank,verify',",
            sha256_hex(b"cabin_smoke")
        )));
        assert!(sql.contains(&format!(
            "VALUES ('smoke-verify', 1, 'smoke-verify', '{}', 'verify',",
            sha256_hex(b"cabin_smoke-verify")
        )));
        assert!(sql.contains(&format!(
            "VALUES ('smoke-noverify', 1, 'smoke-noverify', '{}', 'publish',",
            sha256_hex(b"cabin_smoke-noverify")
        )));
        assert!(
            sql.ends_with("\n    UPDATE meta SET value = 'normal' WHERE key = 'service_mode';")
        );
    }
}
