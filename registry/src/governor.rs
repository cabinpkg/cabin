//! The cost governor's accounting engine: hard, request-time admission
//! control for every billable R2 resource Cabin can initiate
//! (`docs/architecture.md`, "The cost governor").
//!
//! The engine is pure Rust over a tiny [`Store`] abstraction so the same
//! SQL and the same decision logic run against `rusqlite` in host tests
//! and against the governor Durable Object's `SQLite` storage in the
//! deployed Worker (`src/governor_do.rs`). The Durable Object is a
//! single-threaded serialized authority, so the engine may issue
//! sequential statements without interleaving concerns; crash safety
//! comes from the conservative direction of every step (see the module
//! tests), never from multi-statement atomicity.
//!
//! Invariants the engine maintains:
//!
//! - **Ledger is an upper bound of reality.** Committed plus reserved
//!   usage never exceeds a pool's hard limit at admission time, and
//!   every billable operation is recorded before (ops) or reserved
//!   before / committed after (storage) the R2 call it pays for.
//! - **Uncertainty is conservative.** A reservation is released only on
//!   explicit proof (the guarded write was never initiated, or the
//!   object was deleted); age alone never releases anything, and a
//!   commit for bytes that exist in R2 is recorded even over the limit.
//! - **Windows only roll forward.** A clock that regresses can never
//!   reset an operation window and mint fresh allowance.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// Pools
// ---------------------------------------------------------------------

/// Storage pools: stocks of bytes, one ledger row per R2 object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoragePool {
    /// Primary archive blobs (`BLOBS`, `blobs/sha256/<hex>`).
    Primary,
    /// Verified-artifact backup blobs (`BACKUP`, same keys).
    Backup,
    /// Nightly D1 dumps and their sidecars (`BACKUP`, `d1/...`).
    Dump,
}

impl StoragePool {
    pub fn as_str(self) -> &'static str {
        match self {
            StoragePool::Primary => "primary",
            StoragePool::Backup => "backup",
            StoragePool::Dump => "dump",
        }
    }
}

/// Billable-operation pools: monthly flows, consumed immediately before
/// each billable R2 call. Split so abuse of one channel can exhaust
/// only its own allowance: the publish path, the read plane, the source
/// viewer, the verifier, and the operator-side infrastructure jobs
/// (backup replication, nightly dumps, reconciliation) each draw from
/// their own pool, and nothing reachable with an ordinary credential
/// can touch the infrastructure pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpPool {
    /// Class A (writes/lists) on the publish path: blob puts.
    APublish,
    /// Class A for infrastructure: backup replication puts, dump puts,
    /// retention lists.
    AInfra,
    /// Class B (reads) for ordinary artifact downloads (cache misses).
    BOrdinary,
    /// Class B for the source viewer's ranged reads.
    BSource,
    /// Class B for the verifier's pending-artifact fetches.
    BVerifier,
    /// Class B on the publish path: existence heads.
    BPublish,
    /// Class B for infrastructure: replication source reads, dump
    /// re-read verification.
    BInfra,
}

impl OpPool {
    pub fn as_str(self) -> &'static str {
        match self {
            OpPool::APublish => "a_publish",
            OpPool::AInfra => "a_infra",
            OpPool::BOrdinary => "b_ordinary",
            OpPool::BSource => "b_source",
            OpPool::BVerifier => "b_verifier",
            OpPool::BPublish => "b_publish",
            OpPool::BInfra => "b_infra",
        }
    }
}

// ---------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------

/// Additions stay overflow-free: every limit is clamped here, `used`
/// and `SUM(bytes)` never exceed a limit by more than one commit's
/// bytes, and per-request amounts are validated small.
const LIMIT_CLAMP: u64 = i64::MAX as u64 / 4;

/// Hard per-pool ceilings. Env-overridable (`GOVERNOR_*`; see
/// [`Limits::from_lookup`]) with in-code defaults sized comfortably
/// under the R2 free-tier limits, leaving headroom for orphan drift and
/// out-of-band operator work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub storage_primary_bytes: u64,
    pub storage_backup_bytes: u64,
    pub storage_dump_bytes: u64,
    pub a_publish_month: u64,
    pub a_infra_month: u64,
    pub b_ordinary_month: u64,
    pub b_source_month: u64,
    pub b_verifier_month: u64,
    pub b_publish_month: u64,
    pub b_infra_month: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            storage_primary_bytes: 4 * 1024 * 1024 * 1024,
            storage_backup_bytes: 4 * 1024 * 1024 * 1024,
            storage_dump_bytes: 512 * 1024 * 1024,
            a_publish_month: 200_000,
            a_infra_month: 200_000,
            b_ordinary_month: 4_000_000,
            b_source_month: 500_000,
            b_verifier_month: 500_000,
            b_publish_month: 200_000,
            b_infra_month: 200_000,
        }
    }
}

/// One pool's env var name, for [`Limits::from_lookup`] and the docs.
pub fn storage_env_var(pool: StoragePool) -> &'static str {
    match pool {
        StoragePool::Primary => "GOVERNOR_STORAGE_PRIMARY_BYTES",
        StoragePool::Backup => "GOVERNOR_STORAGE_BACKUP_BYTES",
        StoragePool::Dump => "GOVERNOR_STORAGE_DUMP_BYTES",
    }
}

pub fn op_env_var(pool: OpPool) -> &'static str {
    match pool {
        OpPool::APublish => "GOVERNOR_R2_CLASS_A_PUBLISH_MONTH",
        OpPool::AInfra => "GOVERNOR_R2_CLASS_A_INFRA_MONTH",
        OpPool::BOrdinary => "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH",
        OpPool::BSource => "GOVERNOR_R2_CLASS_B_SOURCE_MONTH",
        OpPool::BVerifier => "GOVERNOR_R2_CLASS_B_VERIFIER_MONTH",
        OpPool::BPublish => "GOVERNOR_R2_CLASS_B_PUBLISH_MONTH",
        OpPool::BInfra => "GOVERNOR_R2_CLASS_B_INFRA_MONTH",
    }
}

impl Limits {
    fn storage_limit(&self, pool: StoragePool) -> u64 {
        match pool {
            StoragePool::Primary => self.storage_primary_bytes,
            StoragePool::Backup => self.storage_backup_bytes,
            StoragePool::Dump => self.storage_dump_bytes,
        }
    }

    fn op_limit(&self, pool: OpPool) -> u64 {
        match pool {
            OpPool::APublish => self.a_publish_month,
            OpPool::AInfra => self.a_infra_month,
            OpPool::BOrdinary => self.b_ordinary_month,
            OpPool::BSource => self.b_source_month,
            OpPool::BVerifier => self.b_verifier_month,
            OpPool::BPublish => self.b_publish_month,
            OpPool::BInfra => self.b_infra_month,
        }
    }

    /// Builds the limits from an env lookup. An unset var keeps the
    /// in-code default; a var that is **set but unparsable fails
    /// closed to zero** - these are hard spending caps, so a typo must
    /// block the pool loudly (the refusal names it) rather than
    /// silently reverting to a default the operator meant to change.
    /// `malformed` receives each offending var name for the caller to
    /// log. Every limit is clamped so window additions cannot overflow.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        mut malformed: impl FnMut(&str),
    ) -> Limits {
        let defaults = Limits::default();
        let mut read = |name: &str, default: u64| -> u64 {
            match lookup(name) {
                None => default,
                Some(value) => value.trim().parse().unwrap_or_else(|_| {
                    malformed(name);
                    0
                }),
            }
            .min(LIMIT_CLAMP)
        };
        Limits {
            storage_primary_bytes: read(
                storage_env_var(StoragePool::Primary),
                defaults.storage_primary_bytes,
            ),
            storage_backup_bytes: read(
                storage_env_var(StoragePool::Backup),
                defaults.storage_backup_bytes,
            ),
            storage_dump_bytes: read(
                storage_env_var(StoragePool::Dump),
                defaults.storage_dump_bytes,
            ),
            a_publish_month: read(op_env_var(OpPool::APublish), defaults.a_publish_month),
            a_infra_month: read(op_env_var(OpPool::AInfra), defaults.a_infra_month),
            b_ordinary_month: read(op_env_var(OpPool::BOrdinary), defaults.b_ordinary_month),
            b_source_month: read(op_env_var(OpPool::BSource), defaults.b_source_month),
            b_verifier_month: read(op_env_var(OpPool::BVerifier), defaults.b_verifier_month),
            b_publish_month: read(op_env_var(OpPool::BPublish), defaults.b_publish_month),
            b_infra_month: read(op_env_var(OpPool::BInfra), defaults.b_infra_month),
        }
    }
}

// ---------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------

/// The largest batched op consumption; real callers consume 1 or 2.
pub const MAX_CONSUME_N: u32 = 1_000;

/// One billable-op consumption. `principal` (with `principal_cap`)
/// additionally charges a per-principal daily fairness window; the cap
/// comes from the caller's quota-class model, so the governor stays
/// policy-free. Fairness never grants allowance - the pool check always
/// runs too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consume {
    pub pool: OpPool,
    pub n: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_cap: Option<u64>,
}

/// One storage reservation: capacity taken before the R2 write, keyed
/// by the object key so retries and concurrent identical writes share
/// one row instead of double-counting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reserve {
    pub pool: StoragePool,
    pub key: String,
    pub bytes: u64,
}

/// One storage commit: the write's outcome is known (the object exists
/// and is referenced), so the reservation becomes durable usage. A
/// commit never refuses - the bytes exist in R2, and refusing to record
/// reality would create unaccounted spend - so it may exceed the limit;
/// admission catches up at the next reservation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub pool: StoragePool,
    pub key: String,
    pub bytes: u64,
}

/// One storage release. Only ever sent with proof: the guarded write
/// was never initiated, or the object was affirmatively deleted from
/// R2. Idempotent; releasing an unknown key is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub pool: StoragePool,
    pub key: String,
}

/// One atomic multi-resource decision. The refusable parts (`consume`
/// and `reserve`) are all-or-nothing: on any refusal the engine
/// compensates what it already applied and reports the refusal.
/// `commit` and `release` always apply (they record reality). A crash
/// between steps can only leave extra consumption or an extra
/// reservation behind - conservative, never unaccounted spend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consume: Vec<Consume>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserve: Vec<Reserve>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit: Vec<Commit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub release: Vec<Release>,
}

/// Why a decision was refused. `PrincipalExhausted` maps to a per-user
/// `429` (with seconds until the UTC day rolls over); the others map to
/// the service-wide `503`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refusal {
    PoolExhausted {
        pool: String,
    },
    PrincipalExhausted {
        pool: String,
        retry_after_secs: u64,
    },
    /// The key is already reserved or committed with different bytes:
    /// a caller bug or corruption, never silently merged.
    KeyConflict {
        pool: String,
        key: String,
    },
    /// A structurally invalid request (bad amounts, missing cap).
    Invalid {
        detail: String,
    },
}

/// A decision's outcome on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
}

impl Outcome {
    fn allowed() -> Outcome {
        Outcome {
            ok: true,
            refusal: None,
        }
    }

    fn refused(refusal: Refusal) -> Outcome {
        Outcome {
            ok: false,
            refusal: Some(refusal),
        }
    }
}

// ---------------------------------------------------------------------
// Store abstraction
// ---------------------------------------------------------------------

/// One SQL bind value; the schema only needs text and integers.
#[derive(Debug, Clone)]
pub enum Value {
    Text(String),
    Int(i64),
}

impl From<&str> for Value {
    fn from(value: &str) -> Value {
        Value::Text(value.to_owned())
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Value {
        Value::Int(value)
    }
}

/// The storage the engine runs against: the Durable Object's `SQLite` in
/// the Worker, `rusqlite` in host tests. Rows come back as JSON objects
/// keyed by column name, which both backends produce naturally.
pub trait Store {
    /// Executes a statement, returning the number of changed rows.
    ///
    /// # Errors
    ///
    /// Any storage-level failure, as a message.
    fn exec(&mut self, sql: &str, params: &[Value]) -> Result<usize, String>;

    /// Runs a query, returning each row as a JSON object.
    ///
    /// # Errors
    ///
    /// Any storage-level failure, as a message.
    fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<serde_json::Value>, String>;
}

// ---------------------------------------------------------------------
// Schema and statements
// ---------------------------------------------------------------------

/// Idempotent schema, applied on every Durable Object start so a fresh
/// or reinitialized object recreates its tables before serving.
pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS objects (
        pool TEXT NOT NULL,
        key TEXT NOT NULL,
        bytes INTEGER NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('reserved', 'committed')),
        updated_at TEXT NOT NULL,
        PRIMARY KEY (pool, key))",
    "CREATE TABLE IF NOT EXISTS op_windows (
        pool TEXT PRIMARY KEY,
        window TEXT NOT NULL,
        used INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS principal_windows (
        pool TEXT NOT NULL,
        principal TEXT NOT NULL,
        window TEXT NOT NULL,
        used INTEGER NOT NULL,
        PRIMARY KEY (pool, principal))",
];

const OBJECT_STATE: &str = "SELECT bytes, state FROM objects WHERE pool = ?1 AND key = ?2";

/// Admission-guarded reservation: inserts only while the pool's total
/// (reserved plus committed) stays within the hard limit.
const RESERVE_OBJECT: &str = "INSERT INTO objects (pool, key, bytes, state, updated_at)
     SELECT ?1, ?2, ?3, 'reserved', ?4
     WHERE (SELECT COALESCE(SUM(bytes), 0) FROM objects WHERE pool = ?1) + ?3 <= ?5";

/// The conservative commit upsert: records reality, never refuses.
/// `MAX` keeps the larger byte count if the stored row disagrees.
const COMMIT_OBJECT: &str = "INSERT INTO objects (pool, key, bytes, state, updated_at)
     VALUES (?1, ?2, ?3, 'committed', ?4)
     ON CONFLICT (pool, key) DO UPDATE SET
     state = 'committed',
     bytes = MAX(objects.bytes, excluded.bytes),
     updated_at = excluded.updated_at";

const RELEASE_OBJECT: &str = "DELETE FROM objects WHERE pool = ?1 AND key = ?2";

/// Forward-only window rollover: a regressed clock can never reset a
/// window and mint fresh allowance.
const ROLL_OP_WINDOW: &str = "INSERT INTO op_windows (pool, window, used) VALUES (?1, ?2, 0)
     ON CONFLICT (pool) DO UPDATE SET window = excluded.window, used = 0
     WHERE excluded.window > op_windows.window";

const CONSUME_OPS: &str =
    "UPDATE op_windows SET used = used + ?2 WHERE pool = ?1 AND used + ?2 <= ?3";

/// Compensation for a refused decision; clamped so a stray double
/// compensation can never mint allowance below zero usage.
const UNCONSUME_OPS: &str = "UPDATE op_windows SET used = MAX(used - ?2, 0) WHERE pool = ?1";

const ROLL_PRINCIPAL_WINDOW: &str =
    "INSERT INTO principal_windows (pool, principal, window, used) VALUES (?1, ?2, ?3, 0)
     ON CONFLICT (pool, principal) DO UPDATE SET window = excluded.window, used = 0
     WHERE excluded.window > principal_windows.window";

const CONSUME_PRINCIPAL: &str = "UPDATE principal_windows SET used = used + ?3
     WHERE pool = ?1 AND principal = ?2 AND used + ?3 <= ?4";

const UNCONSUME_PRINCIPAL: &str = "UPDATE principal_windows SET used = MAX(used - ?3, 0)
     WHERE pool = ?1 AND principal = ?2";

const STORAGE_USAGE: &str = "SELECT pool, state, COALESCE(SUM(bytes), 0) AS bytes,
     COUNT(*) AS objects FROM objects GROUP BY pool, state ORDER BY pool, state";

const OP_USAGE: &str = "SELECT pool, window, used FROM op_windows ORDER BY pool";

const OBJECT_KEYS: &str = "SELECT key, bytes, state FROM objects WHERE pool = ?1 ORDER BY key";

/// Stale fairness rows carry no meaning once their day passed.
const PRUNE_PRINCIPAL_WINDOWS: &str = "DELETE FROM principal_windows WHERE window < ?1";

// The operator-triggered ledger wipe (pre-launch only, guarded by the
// caller on `meta.launched`). Deliberately **primary-storage-only**:
// the registry wipe deletes the primary blobs, so those rows restart
// from zero and reconciliation rebuilds whatever the fresh registry
// accrues - but the BACKUP bucket is never wiped, so its `backup` and
// `dump` rows must survive or the ledger would understate objects that
// keep billing. The monthly `op_windows` are NOT wiped: they count R2
// operations Cloudflare has already metered this month, reconciliation
// cannot rebuild them (R2 exposes no per-pool op counter), and zeroing
// them mid-month would re-mint a full month of Class A/B allowance on
// top of the ops that already ran. Daily `principal_windows` are
// fairness-only (they never grant pool allowance) and roll forward on
// their own, so clearing them is harmless.
const WIPE_PRIMARY_OBJECTS: &str = "DELETE FROM objects WHERE pool = 'primary'";
const WIPE_PRINCIPAL_WINDOWS: &str = "DELETE FROM principal_windows";

// ---------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------

/// The `YYYY-MM` UTC month of an ISO 8601 timestamp. The governor's
/// operation windows are **explicit UTC calendar months**: Cloudflare's
/// actual billing window cannot be inferred reliably, so the budgets
/// are sized with headroom for the skew instead (`docs/runbook.md`).
pub fn utc_month(now_iso: &str) -> Option<&str> {
    let month = now_iso.get(..7)?;
    let bytes = month.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    (digits(0..4) && bytes[4] == b'-' && digits(5..7)).then_some(month)
}

/// The `YYYY-MM-DD` UTC day, for the fairness windows.
pub fn utc_day(now_iso: &str) -> Option<&str> {
    crate::analytics::utc_date(now_iso)
}

/// Seconds until the next UTC midnight, for the fairness refusal's
/// `Retry-After`. Falls back to a full day on a malformed clock.
pub fn secs_to_next_utc_day(now_iso: &str) -> u64 {
    let time = now_iso.get(11..19).unwrap_or_default();
    let mut parts = time.split(':');
    let mut field = || {
        parts
            .next()
            .and_then(|part| part.parse::<u64>().ok())
            .filter(|&n| n < 60)
    };
    match (field(), field(), field()) {
        (Some(h), Some(m), Some(s)) if h < 24 => 86_400 - (h * 3_600 + m * 60 + s),
        _ => 86_400,
    }
}

// ---------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct ObjectRow {
    bytes: i64,
}

fn object_state(
    store: &mut impl Store,
    pool: &str,
    key: &str,
) -> Result<Option<ObjectRow>, String> {
    let rows = store.rows(OBJECT_STATE, &[pool.into(), key.into()])?;
    match rows.into_iter().next() {
        None => Ok(None),
        Some(row) => serde_json::from_value(row)
            .map(Some)
            .map_err(|err| format!("object row did not parse: {err}")),
    }
}

/// What [`decide`] applied before a refusal, for compensation. Only
/// fresh effects are undone: an idempotent reserve that matched an
/// existing row must never delete it.
enum Applied {
    Ops {
        pool: OpPool,
        n: i64,
    },
    Principal {
        pool: OpPool,
        principal: String,
        n: i64,
    },
    Reserved {
        pool: StoragePool,
        key: String,
    },
}

fn compensate(store: &mut impl Store, applied: &[Applied]) -> Result<(), String> {
    for action in applied.iter().rev() {
        match action {
            Applied::Ops { pool, n } => {
                store.exec(UNCONSUME_OPS, &[pool.as_str().into(), (*n).into()])?;
            }
            Applied::Principal { pool, principal, n } => {
                store.exec(
                    UNCONSUME_PRINCIPAL,
                    &[pool.as_str().into(), principal.as_str().into(), (*n).into()],
                )?;
            }
            Applied::Reserved { pool, key } => {
                store.exec(RELEASE_OBJECT, &[pool.as_str().into(), key.as_str().into()])?;
            }
        }
    }
    Ok(())
}

/// Validates request amounts so the SQL arithmetic stays trivially
/// overflow-free; the real budgets are the pool limits.
fn validate(decision: &Decision) -> Result<(), Refusal> {
    let invalid = |detail: &str| Refusal::Invalid {
        detail: detail.to_owned(),
    };
    for consume in &decision.consume {
        if consume.n == 0 || consume.n > MAX_CONSUME_N {
            return Err(invalid("consume n out of range"));
        }
        if consume.principal.is_some() != consume.principal_cap.is_some() {
            return Err(invalid("principal and principal_cap must come together"));
        }
        if consume.principal_cap.is_some_and(|cap| cap > LIMIT_CLAMP) {
            return Err(invalid("principal_cap out of range"));
        }
    }
    for (key, bytes) in decision
        .reserve
        .iter()
        .map(|r| (&r.key, r.bytes))
        .chain(decision.commit.iter().map(|c| (&c.key, c.bytes)))
    {
        if key.is_empty() || key.len() > 512 {
            return Err(invalid("object key length out of range"));
        }
        // Zero is never a real archive, dump, or sidecar: a profile zip
        // is at least its 22-byte end record. Admitting a zero-byte
        // object inserts a phantom row that later `KeyConflict`s a real
        // reserve of the same content-addressed key. The upper bound
        // keeps the admission arithmetic overflow-free (the same clamp
        // the limits get); anything a configured pool could actually
        // admit passes. An artifact-sized cap here once broke nightly
        // dumps the moment the database outgrew it.
        if bytes == 0 || bytes > LIMIT_CLAMP {
            return Err(invalid("object bytes out of range"));
        }
    }
    for release in &decision.release {
        if release.key.is_empty() || release.key.len() > 512 {
            return Err(invalid("object key length out of range"));
        }
    }
    // Each (pool, key) may take part in at most one storage operation
    // per decision. Reserve and commit ADD capacity; release REMOVES
    // it. Mixing them for one key in a single decision has no
    // conservative direction: `decide` applies commits before releases,
    // so a commit+release of one key would record the object as
    // committed and then delete its ledger row while the object still
    // exists in R2 - an under-count that mints free capacity. Real
    // callers always separate these into distinct decisions (reserve
    // before the R2 write, commit/release after), so a collision is a
    // caller bug the engine refuses outright rather than applies.
    let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    for entry in decision
        .reserve
        .iter()
        .map(|r| (r.pool.as_str(), r.key.as_str()))
        .chain(
            decision
                .commit
                .iter()
                .map(|c| (c.pool.as_str(), c.key.as_str())),
        )
        .chain(
            decision
                .release
                .iter()
                .map(|r| (r.pool.as_str(), r.key.as_str())),
        )
    {
        if !seen.insert(entry) {
            return Err(invalid("a storage key appears in more than one operation"));
        }
    }
    Ok(())
}

/// One consume item: the fairness window first (its refusal is
/// per-caller and must not consume pool allowance), then the pool.
/// Fresh effects land in `applied`; `Some` is the refusal to report.
fn apply_consume(
    store: &mut impl Store,
    limits: &Limits,
    month: &str,
    day: &str,
    now_iso: &str,
    consume: &Consume,
    applied: &mut Vec<Applied>,
) -> Result<Option<Refusal>, String> {
    let pool = consume.pool;
    let n = i64::from(consume.n);
    if let (Some(principal), Some(cap)) = (&consume.principal, consume.principal_cap) {
        store.exec(
            ROLL_PRINCIPAL_WINDOW,
            &[pool.as_str().into(), principal.as_str().into(), day.into()],
        )?;
        #[allow(clippy::cast_possible_wrap)] // clamped to LIMIT_CLAMP
        let cap = cap as i64;
        let changed = store.exec(
            CONSUME_PRINCIPAL,
            &[
                pool.as_str().into(),
                principal.as_str().into(),
                n.into(),
                cap.into(),
            ],
        )?;
        if changed == 0 {
            return Ok(Some(Refusal::PrincipalExhausted {
                pool: pool.as_str().to_owned(),
                retry_after_secs: secs_to_next_utc_day(now_iso),
            }));
        }
        applied.push(Applied::Principal {
            pool,
            principal: principal.clone(),
            n,
        });
    }
    store.exec(ROLL_OP_WINDOW, &[pool.as_str().into(), month.into()])?;
    #[allow(clippy::cast_possible_wrap)] // clamped to LIMIT_CLAMP
    let limit = limits.op_limit(pool) as i64;
    let changed = store.exec(CONSUME_OPS, &[pool.as_str().into(), n.into(), limit.into()])?;
    if changed == 0 {
        return Ok(Some(Refusal::PoolExhausted {
            pool: pool.as_str().to_owned(),
        }));
    }
    applied.push(Applied::Ops { pool, n });
    Ok(None)
}

/// One reserve item: idempotent against an existing row with the same
/// bytes (retries and concurrent identical writes share the
/// content-addressed key), a conflict on different bytes, and an
/// admission-guarded insert otherwise.
fn apply_reserve(
    store: &mut impl Store,
    limits: &Limits,
    now_iso: &str,
    reserve: &Reserve,
    applied: &mut Vec<Applied>,
) -> Result<Option<Refusal>, String> {
    let pool = reserve.pool;
    #[allow(clippy::cast_possible_wrap)] // validated <= LIMIT_CLAMP
    let bytes = reserve.bytes as i64;
    match object_state(store, pool.as_str(), &reserve.key)? {
        Some(row) if row.bytes == bytes => return Ok(None),
        Some(_) => {
            return Ok(Some(Refusal::KeyConflict {
                pool: pool.as_str().to_owned(),
                key: reserve.key.clone(),
            }));
        }
        None => {}
    }
    #[allow(clippy::cast_possible_wrap)] // clamped to LIMIT_CLAMP
    let limit = limits.storage_limit(pool) as i64;
    let changed = store.exec(
        RESERVE_OBJECT,
        &[
            pool.as_str().into(),
            reserve.key.as_str().into(),
            bytes.into(),
            now_iso.into(),
            limit.into(),
        ],
    )?;
    if changed == 0 {
        return Ok(Some(Refusal::PoolExhausted {
            pool: pool.as_str().to_owned(),
        }));
    }
    applied.push(Applied::Reserved {
        pool,
        key: reserve.key.clone(),
    });
    Ok(None)
}

/// Applies one decision. Refusable parts (consumes, then reserves) are
/// all-or-nothing: a refusal compensates every fresh effect this call
/// applied and reports the first refusal. Commits and releases always
/// apply, after the refusable parts succeeded.
///
/// # Errors
///
/// Storage-level failure. The caller must treat an error exactly like
/// an unreachable governor: fail closed before initiating any billable
/// call (some consumption may already have applied - conservative).
pub fn decide(
    store: &mut impl Store,
    limits: &Limits,
    now_iso: &str,
    decision: &Decision,
) -> Result<Outcome, String> {
    if let Err(refusal) = validate(decision) {
        return Ok(Outcome::refused(refusal));
    }
    let Some(month) = utc_month(now_iso) else {
        return Err(format!("clock produced a non-ISO timestamp: {now_iso}"));
    };
    let Some(day) = utc_day(now_iso) else {
        return Err(format!("clock produced a non-ISO timestamp: {now_iso}"));
    };

    let mut applied: Vec<Applied> = Vec::new();
    for consume in &decision.consume {
        if let Some(refusal) =
            apply_consume(store, limits, month, day, now_iso, consume, &mut applied)?
        {
            compensate(store, &applied)?;
            return Ok(Outcome::refused(refusal));
        }
    }
    for reserve in &decision.reserve {
        if let Some(refusal) = apply_reserve(store, limits, now_iso, reserve, &mut applied)? {
            compensate(store, &applied)?;
            return Ok(Outcome::refused(refusal));
        }
    }

    for commit in &decision.commit {
        #[allow(clippy::cast_possible_wrap)] // validated <= LIMIT_CLAMP
        let bytes = commit.bytes as i64;
        store.exec(
            COMMIT_OBJECT,
            &[
                commit.pool.as_str().into(),
                commit.key.as_str().into(),
                bytes.into(),
                now_iso.into(),
            ],
        )?;
    }
    for release in &decision.release {
        store.exec(
            RELEASE_OBJECT,
            &[release.pool.as_str().into(), release.key.as_str().into()],
        )?;
    }
    Ok(Outcome::allowed())
}

// ---------------------------------------------------------------------
// Usage and reconciliation
// ---------------------------------------------------------------------

/// One storage pool's usage split by state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageUsage {
    pub pool: String,
    pub state: String,
    pub bytes: u64,
    pub objects: u64,
}

/// One op pool's window and consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpUsage {
    pub pool: String,
    pub window: String,
    pub used: u64,
}

/// The `/usage` snapshot: the ledger as the reconciliation cron and the
/// operator see it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub storage: Vec<StorageUsage>,
    pub ops: Vec<OpUsage>,
}

#[derive(Deserialize)]
struct StorageUsageRow {
    pool: String,
    state: String,
    bytes: i64,
    objects: i64,
}

#[derive(Deserialize)]
struct OpUsageRow {
    pool: String,
    window: String,
    used: i64,
}

/// Reads the usage snapshot.
///
/// # Errors
///
/// Storage-level failure.
pub fn usage(store: &mut impl Store) -> Result<UsageSnapshot, String> {
    let non_negative = |value: i64| u64::try_from(value).unwrap_or(0);
    let storage = store
        .rows(STORAGE_USAGE, &[])?
        .into_iter()
        .map(|row| {
            serde_json::from_value::<StorageUsageRow>(row)
                .map(|row| StorageUsage {
                    pool: row.pool,
                    state: row.state,
                    bytes: non_negative(row.bytes),
                    objects: non_negative(row.objects),
                })
                .map_err(|err| format!("storage usage row did not parse: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ops = store
        .rows(OP_USAGE, &[])?
        .into_iter()
        .map(|row| {
            serde_json::from_value::<OpUsageRow>(row)
                .map(|row| OpUsage {
                    pool: row.pool,
                    window: row.window,
                    used: non_negative(row.used),
                })
                .map_err(|err| format!("op usage row did not parse: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UsageSnapshot { storage, ops })
}

/// One live object per D1's authoritative view, for reconciliation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveObject {
    pub key: String,
    pub bytes: u64,
}

/// A reconciliation request: the authoritative live set for one pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    pub pool: StoragePool,
    pub live: Vec<LiveObject>,
}

/// What reconciliation changed and what it can only report.
/// **Increase-only by design**: every object the authoritative set
/// proves live is committed through the conservative upsert (missing
/// rows appear, reserved rows a lost acknowledgement stranded settle,
/// byte counts only ever grow); ledger entries the set does not name
/// are reported (candidate orphans or leaked reservations), never
/// released - a decrease needs proof the object is gone, which is the
/// operator's explicit release (`docs/runbook.md`, "Governor ledger").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileReport {
    /// Keys newly recorded as committed (ledger was missing them).
    pub added: Vec<String>,
    /// Ledger keys the authoritative set does not name.
    pub unreferenced: Vec<String>,
    /// Keys present in both but with differing byte counts (kept at
    /// the larger of the two; reported for the operator).
    pub mismatched: Vec<String>,
}

#[derive(Deserialize)]
struct ObjectKeyRow {
    key: String,
    bytes: i64,
}

/// Applies one reconciliation pass for a pool.
///
/// # Errors
///
/// Storage-level failure.
pub fn reconcile(
    store: &mut impl Store,
    now_iso: &str,
    request: &ReconcileRequest,
) -> Result<ReconcileReport, String> {
    let pool = request.pool.as_str();
    let ledger = store
        .rows(OBJECT_KEYS, &[pool.into()])?
        .into_iter()
        .map(|row| {
            serde_json::from_value::<ObjectKeyRow>(row)
                .map_err(|err| format!("object row did not parse: {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ledger_bytes: std::collections::HashMap<&str, i64> = ledger
        .iter()
        .map(|row| (row.key.as_str(), row.bytes))
        .collect();

    let mut report = ReconcileReport {
        added: Vec::new(),
        unreferenced: Vec::new(),
        mismatched: Vec::new(),
    };
    // A live object is a real R2 object, and a real archive or dump is
    // never empty; a zero-byte entry is corrupt D1 data, so drop it
    // rather than commit the phantom row `validate` forbids on the
    // request path (a 0-byte committed row would later `KeyConflict` a
    // real reserve of the same content-addressed key). A dropped key
    // that carries a ledger row is reported `unreferenced` for the
    // operator, exactly like any other entry the live set does not name.
    let live: Vec<&LiveObject> = request.live.iter().filter(|obj| obj.bytes > 0).collect();
    for obj in &live {
        // Reality is never clamped downward: the request-path item cap
        // does not apply here, because recording less than an object's
        // true size would make the ledger understate R2.
        let bytes = i64::try_from(obj.bytes).unwrap_or(i64::MAX);
        match ledger_bytes.get(obj.key.as_str()) {
            None => report.added.push(obj.key.clone()),
            Some(&stored) if stored != bytes => report.mismatched.push(obj.key.clone()),
            Some(_) => {}
        }
        // The same conservative commit upsert the settle path uses:
        // records missing objects, settles reserved rows a lost
        // acknowledgement stranded, and only ever grows byte counts.
        store.exec(
            COMMIT_OBJECT,
            &[
                pool.into(),
                obj.key.as_str().into(),
                bytes.into(),
                now_iso.into(),
            ],
        )?;
    }
    let live_keys: std::collections::HashSet<&str> =
        live.iter().map(|obj| obj.key.as_str()).collect();
    for row in &ledger {
        if !live_keys.contains(row.key.as_str()) {
            report.unreferenced.push(row.key.clone());
        }
    }
    // Fairness rows from past days are dead weight; prune them here so
    // the table stays bounded by the active principal count.
    if let Some(day) = utc_day(now_iso) {
        store.exec(PRUNE_PRINCIPAL_WINDOWS, &[day.into()])?;
    }
    Ok(report)
}

/// Clears the primary-pool storage ledger and the daily fairness
/// windows. The caller owns the guard: this is the pre-launch registry
/// wipe's companion and must never run against a launched registry
/// (`docs/runbook.md`, "Wipe procedure"). The `backup` and `dump`
/// storage rows survive on purpose - the wipe never touches the BACKUP
/// bucket, and their objects keep billing. The monthly `op_windows`
/// survive too: they mirror R2 operations Cloudflare already metered
/// this month and are not reconstructable, so zeroing them would mint
/// fresh monthly allowance for spend that already happened.
///
/// # Errors
///
/// Storage-level failure.
pub fn wipe(store: &mut impl Store) -> Result<(), String> {
    store.exec(WIPE_PRIMARY_OBJECTS, &[])?;
    store.exec(WIPE_PRINCIPAL_WINDOWS, &[])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host-test [`Store`]: the same SQL the Durable Object runs,
    /// prepared by `rusqlite` against the same schema.
    struct Sqlite(rusqlite::Connection);

    impl Sqlite {
        fn new() -> Sqlite {
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
            for statement in SCHEMA {
                conn.execute(statement, []).expect("schema applies");
            }
            Sqlite(conn)
        }
    }

    fn bindings(params: &[Value]) -> Vec<rusqlite::types::Value> {
        params
            .iter()
            .map(|value| match value {
                Value::Text(text) => rusqlite::types::Value::Text(text.clone()),
                Value::Int(int) => rusqlite::types::Value::Integer(*int),
            })
            .collect()
    }

    // The adapter bodies live in inherent methods so the fault-
    // injecting [`Flaky`] store below can delegate to them without
    // spelling a dynamic-argument `exec(`/`prepare(` call - the SQL-
    // consolidation scan sanctions exactly the const-exec and
    // `prepare(sql)` shapes in this file, and widening it for a test
    // forwarder would loosen the ceiling for the whole engine.
    impl Sqlite {
        fn run_statement(&mut self, sql: &str, params: &[Value]) -> Result<usize, String> {
            self.0
                .execute(sql, rusqlite::params_from_iter(bindings(params)))
                .map_err(|err| err.to_string())
        }

        fn read_rows(
            &mut self,
            sql: &str,
            params: &[Value],
        ) -> Result<Vec<serde_json::Value>, String> {
            let mut statement = self.0.prepare(sql).map_err(|err| err.to_string())?;
            let names: Vec<String> = statement
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut rows = statement
                .query(rusqlite::params_from_iter(bindings(params)))
                .map_err(|err| err.to_string())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|err| err.to_string())? {
                let mut object = serde_json::Map::new();
                for (index, name) in names.iter().enumerate() {
                    let value: rusqlite::types::Value =
                        row.get(index).map_err(|err| err.to_string())?;
                    let json = match value {
                        rusqlite::types::Value::Integer(int) => serde_json::json!(int),
                        rusqlite::types::Value::Real(real) => serde_json::json!(real),
                        rusqlite::types::Value::Text(text) => serde_json::json!(text),
                        _ => serde_json::Value::Null,
                    };
                    object.insert(name.clone(), json);
                }
                out.push(serde_json::Value::Object(object));
            }
            Ok(out)
        }
    }

    impl Store for Sqlite {
        fn exec(&mut self, sql: &str, params: &[Value]) -> Result<usize, String> {
            self.run_statement(sql, params)
        }

        fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<serde_json::Value>, String> {
            self.read_rows(sql, params)
        }
    }

    const NOW: &str = "2026-07-22T12:00:00.000Z";

    fn small_limits() -> Limits {
        Limits {
            storage_primary_bytes: 100,
            storage_backup_bytes: 50,
            storage_dump_bytes: 30,
            a_publish_month: 5,
            a_infra_month: 5,
            b_ordinary_month: 5,
            b_source_month: 5,
            b_verifier_month: 5,
            b_publish_month: 5,
            b_infra_month: 5,
        }
    }

    fn consume(pool: OpPool, n: u32) -> Decision {
        Decision {
            consume: vec![Consume {
                pool,
                n,
                principal: None,
                principal_cap: None,
            }],
            ..Decision::default()
        }
    }

    fn consume_as(pool: OpPool, n: u32, principal: &str, cap: u64) -> Decision {
        Decision {
            consume: vec![Consume {
                pool,
                n,
                principal: Some(principal.to_owned()),
                principal_cap: Some(cap),
            }],
            ..Decision::default()
        }
    }

    fn reserve(pool: StoragePool, key: &str, bytes: u64) -> Decision {
        Decision {
            reserve: vec![Reserve {
                pool,
                key: key.to_owned(),
                bytes,
            }],
            ..Decision::default()
        }
    }

    fn commit(pool: StoragePool, key: &str, bytes: u64) -> Decision {
        Decision {
            commit: vec![Commit {
                pool,
                key: key.to_owned(),
                bytes,
            }],
            ..Decision::default()
        }
    }

    fn release(pool: StoragePool, key: &str) -> Decision {
        Decision {
            release: vec![Release {
                pool,
                key: key.to_owned(),
            }],
            ..Decision::default()
        }
    }

    fn pool_bytes(store: &mut Sqlite, pool: StoragePool) -> u64 {
        usage(store)
            .expect("usage reads")
            .storage
            .iter()
            .filter(|row| row.pool == pool.as_str())
            .map(|row| row.bytes)
            .sum()
    }

    fn pool_used(store: &mut Sqlite, pool: OpPool) -> u64 {
        usage(store)
            .expect("usage reads")
            .ops
            .iter()
            .find(|row| row.pool == pool.as_str())
            .map_or(0, |row| row.used)
    }

    #[track_caller]
    fn allowed(store: &mut Sqlite, limits: &Limits, decision: &Decision) {
        let outcome = decide(store, limits, NOW, decision).expect("decide runs");
        assert!(outcome.ok, "refused: {:?}", outcome.refusal);
    }

    #[track_caller]
    fn refused(store: &mut Sqlite, limits: &Limits, decision: &Decision) -> Refusal {
        let outcome = decide(store, limits, NOW, decision).expect("decide runs");
        assert!(!outcome.ok, "expected a refusal");
        outcome.refusal.expect("refusal is reported")
    }

    #[test]
    fn storage_admission_is_exact_at_the_limit() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // Landing exactly on the limit is allowed; one byte over is not.
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 60));
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "b", 40));
        let refusal = refused(&mut store, &limits, &reserve(StoragePool::Primary, "c", 1));
        assert_eq!(
            refusal,
            Refusal::PoolExhausted {
                pool: "primary".to_owned()
            }
        );
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 100);
    }

    #[test]
    fn reserved_and_committed_bytes_both_count_against_admission() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 60));
        allowed(&mut store, &limits, &commit(StoragePool::Primary, "a", 60));
        // 60 committed + 41 requested > 100: the committed bytes still
        // gate admission.
        let refusal = refused(&mut store, &limits, &reserve(StoragePool::Primary, "b", 41));
        assert!(matches!(refusal, Refusal::PoolExhausted { .. }));
    }

    #[test]
    fn reservation_is_idempotent_under_the_same_key_and_bytes() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // Two concurrent identical publishes: the content-addressed key
        // makes the second reserve a no-op instead of a double count.
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 90));
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 90));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 90);
        // Both publishes commit; the second commit is a no-op too.
        allowed(&mut store, &limits, &commit(StoragePool::Primary, "a", 90));
        allowed(&mut store, &limits, &commit(StoragePool::Primary, "a", 90));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 90);
    }

    #[test]
    fn conflicting_bytes_under_one_key_are_refused() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 10));
        let refusal = refused(&mut store, &limits, &reserve(StoragePool::Primary, "a", 20));
        assert_eq!(
            refusal,
            Refusal::KeyConflict {
                pool: "primary".to_owned(),
                key: "a".to_owned()
            }
        );
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 10);
    }

    #[test]
    fn a_crashed_publish_keeps_its_reservation_until_explicit_release() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // Reserve + R2 put, then crash before D1: the reservation stays,
        // reducing allowance but never creating unaccounted spend.
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 70));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 70);
        // A retry of the same content heals it into committed state.
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 70));
        allowed(&mut store, &limits, &commit(StoragePool::Primary, "a", 70));
        let snapshot = usage(&mut store).expect("usage reads");
        assert_eq!(snapshot.storage.len(), 1);
        assert_eq!(snapshot.storage[0].state, "committed");
        assert_eq!(snapshot.storage[0].bytes, 70);
    }

    #[test]
    fn release_frees_capacity_and_is_idempotent() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "a", 100),
        );
        refused(&mut store, &limits, &reserve(StoragePool::Primary, "b", 1));
        allowed(&mut store, &limits, &release(StoragePool::Primary, "a"));
        allowed(&mut store, &limits, &release(StoragePool::Primary, "a"));
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "b", 100),
        );
    }

    #[test]
    fn commit_records_reality_even_over_the_limit() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "a", 100),
        );
        // A commit for bytes that exist in R2 must never be refused:
        // refusing to record reality would create unaccounted spend.
        allowed(&mut store, &limits, &commit(StoragePool::Primary, "b", 80));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 180);
        // Admission now refuses everything until usage drops.
        refused(&mut store, &limits, &reserve(StoragePool::Primary, "c", 1));
    }

    #[test]
    fn op_consumption_is_exact_at_the_limit() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &consume(OpPool::BOrdinary, 4));
        allowed(&mut store, &limits, &consume(OpPool::BOrdinary, 1));
        let refusal = refused(&mut store, &limits, &consume(OpPool::BOrdinary, 1));
        assert_eq!(
            refusal,
            Refusal::PoolExhausted {
                pool: "b_ordinary".to_owned()
            }
        );
        assert_eq!(pool_used(&mut store, OpPool::BOrdinary), 5);
    }

    #[test]
    fn pools_are_isolated() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // Exhausting the ordinary read pool leaves the verifier,
        // source, and infrastructure pools untouched.
        allowed(&mut store, &limits, &consume(OpPool::BOrdinary, 5));
        refused(&mut store, &limits, &consume(OpPool::BOrdinary, 1));
        allowed(&mut store, &limits, &consume(OpPool::BVerifier, 5));
        allowed(&mut store, &limits, &consume(OpPool::BSource, 5));
        allowed(&mut store, &limits, &consume(OpPool::BInfra, 5));
        allowed(&mut store, &limits, &consume(OpPool::AInfra, 5));
        // Storage pools are isolated the same way.
        allowed(&mut store, &limits, &reserve(StoragePool::Backup, "k", 50));
        refused(&mut store, &limits, &reserve(StoragePool::Backup, "l", 1));
        allowed(&mut store, &limits, &reserve(StoragePool::Dump, "d", 30));
    }

    #[test]
    fn a_multi_resource_decision_is_all_or_nothing() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &consume(OpPool::APublish, 4));
        // consume would fit, reserve would not: nothing may stick.
        let decision = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "big".to_owned(),
                bytes: 101,
            }],
            ..Decision::default()
        };
        refused(&mut store, &limits, &decision);
        assert_eq!(pool_used(&mut store, OpPool::APublish), 4);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 0);

        // The reverse order of failure: reserve applied, second consume
        // refused - the fresh reservation is compensated away too.
        allowed(&mut store, &limits, &consume(OpPool::BPublish, 5));
        let decision = Decision {
            consume: vec![
                Consume {
                    pool: OpPool::APublish,
                    n: 1,
                    principal: None,
                    principal_cap: None,
                },
                Consume {
                    pool: OpPool::BPublish,
                    n: 1,
                    principal: None,
                    principal_cap: None,
                },
            ],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "ok".to_owned(),
                bytes: 10,
            }],
            ..Decision::default()
        };
        refused(&mut store, &limits, &decision);
        assert_eq!(pool_used(&mut store, OpPool::APublish), 4);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 0);
    }

    #[test]
    fn compensation_never_deletes_a_preexisting_reservation() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 10));
        // An idempotent re-reserve of "a" rides a decision whose second
        // reserve is refused: compensation must delete only the fresh
        // "b" row, never the pre-existing "a".
        let decision = Decision {
            reserve: vec![
                Reserve {
                    pool: StoragePool::Primary,
                    key: "a".to_owned(),
                    bytes: 10,
                },
                Reserve {
                    pool: StoragePool::Primary,
                    key: "b".to_owned(),
                    bytes: 50,
                },
                Reserve {
                    pool: StoragePool::Primary,
                    key: "c".to_owned(),
                    bytes: 41,
                },
            ],
            ..Decision::default()
        };
        refused(&mut store, &limits, &decision);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 10);
        let snapshot = usage(&mut store).expect("usage reads");
        assert_eq!(
            snapshot.storage.len(),
            1,
            "only the pre-existing row remains"
        );
    }

    #[test]
    fn fairness_caps_one_principal_without_touching_the_pool_or_others() {
        let mut store = Sqlite::new();
        let limits = Limits {
            b_ordinary_month: 1_000,
            ..small_limits()
        };
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 2),
        );
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 2),
        );
        let refusal = refused(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 2),
        );
        let Refusal::PrincipalExhausted {
            pool,
            retry_after_secs,
        } = refusal
        else {
            panic!("expected PrincipalExhausted, got {refusal:?}");
        };
        assert_eq!(pool, "b_ordinary");
        // NOW is 12:00:00Z: half a day to the window rollover.
        assert_eq!(retry_after_secs, 43_200);
        // The refused attempt consumed no pool allowance...
        assert_eq!(pool_used(&mut store, OpPool::BOrdinary), 2);
        // ...and another principal (or a raised cap) still proceeds.
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "2", 2),
        );
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 5),
        );
    }

    #[test]
    fn fairness_never_grants_pool_allowance() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &consume(OpPool::BOrdinary, 5));
        // A principal with plenty of personal cap still hits the pool.
        let refusal = refused(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 1_000),
        );
        assert!(matches!(refusal, Refusal::PoolExhausted { .. }));
    }

    #[test]
    fn op_windows_roll_forward_and_never_backward() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &consume(OpPool::BOrdinary, 5));
        refused(&mut store, &limits, &consume(OpPool::BOrdinary, 1));
        // A new month resets the window.
        let outcome = decide(
            &mut store,
            &limits,
            "2026-08-01T00:00:00.000Z",
            &consume(OpPool::BOrdinary, 5),
        )
        .expect("decide runs");
        assert!(outcome.ok);
        // A clock regressing into July must not reset August's window.
        let outcome = decide(
            &mut store,
            &limits,
            "2026-07-31T23:59:59.000Z",
            &consume(OpPool::BOrdinary, 1),
        )
        .expect("decide runs");
        assert!(!outcome.ok, "a regressed clock must not mint allowance");
    }

    #[test]
    fn principal_windows_roll_daily_and_never_backward() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(&mut store, &limits, &consume_as(OpPool::BSource, 2, "1", 2));
        refused(&mut store, &limits, &consume_as(OpPool::BSource, 1, "1", 2));
        // The next UTC day resets the fairness window.
        let outcome = decide(
            &mut store,
            &limits,
            "2026-07-23T00:00:00.000Z",
            &consume_as(OpPool::BSource, 2, "1", 2),
        )
        .expect("decide runs");
        assert!(outcome.ok);
        // Regressing to the previous day must not reset it again.
        let outcome = decide(
            &mut store,
            &limits,
            "2026-07-22T23:00:00.000Z",
            &consume_as(OpPool::BSource, 1, "1", 2),
        )
        .expect("decide runs");
        assert!(!outcome.ok);
    }

    #[test]
    fn reconcile_adds_missing_objects_and_only_reports_the_rest() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "orphan", 10),
        );
        allowed(
            &mut store,
            &limits,
            &commit(StoragePool::Primary, "drifted", 10),
        );
        let report = reconcile(
            &mut store,
            NOW,
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![
                    LiveObject {
                        key: "missing".to_owned(),
                        bytes: 30,
                    },
                    LiveObject {
                        key: "drifted".to_owned(),
                        bytes: 20,
                    },
                ],
            },
        )
        .expect("reconcile runs");
        // The live object the ledger lacked is now committed usage, and
        // the understated byte count grew to the authoritative value:
        // 30 (missing) + 20 (drifted, grown from 10) + 10 (orphan).
        assert_eq!(report.added, vec!["missing".to_owned()]);
        assert_eq!(report.mismatched, vec!["drifted".to_owned()]);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 60);
        // The unreferenced reservation is reported but never released.
        assert_eq!(report.unreferenced, vec!["orphan".to_owned()]);
        let snapshot = usage(&mut store).expect("usage reads");
        assert!(
            snapshot
                .storage
                .iter()
                .any(|row| row.state == "reserved" && row.bytes == 10)
        );
    }

    #[test]
    fn reconcile_settles_a_stranded_reservation_the_live_set_proves() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // A publish whose D1 rows landed but whose governor commit was
        // lost: the live set names the key, so reconcile settles it.
        allowed(&mut store, &limits, &reserve(StoragePool::Primary, "a", 10));
        let report = reconcile(
            &mut store,
            NOW,
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![LiveObject {
                    key: "a".to_owned(),
                    bytes: 10,
                }],
            },
        )
        .expect("reconcile runs");
        assert!(report.added.is_empty());
        assert!(report.unreferenced.is_empty());
        let snapshot = usage(&mut store).expect("usage reads");
        assert_eq!(snapshot.storage.len(), 1);
        assert_eq!(snapshot.storage[0].state, "committed");
        assert_eq!(snapshot.storage[0].bytes, 10);
    }

    #[test]
    fn reconcile_records_reality_beyond_the_request_item_cap() {
        let mut store = Sqlite::new();
        let report = reconcile(
            &mut store,
            NOW,
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![LiveObject {
                    key: "huge".to_owned(),
                    bytes: LIMIT_CLAMP + 1,
                }],
            },
        )
        .expect("reconcile runs");
        assert_eq!(report.added, vec!["huge".to_owned()]);
        assert_eq!(
            pool_bytes(&mut store, StoragePool::Primary),
            LIMIT_CLAMP + 1
        );
    }

    #[test]
    fn reconcile_skips_a_zero_byte_live_object() {
        let mut store = Sqlite::new();
        // A 0-byte "live" object is corrupt D1 data - a real archive is
        // never empty - so reconcile must not record a phantom row for
        // it (that row would later `KeyConflict` a real reserve of the
        // same content-addressed key), the same invariant N2 enforces
        // on the request path.
        let report = reconcile(
            &mut store,
            NOW,
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![
                    LiveObject {
                        key: "real".to_owned(),
                        bytes: 10,
                    },
                    LiveObject {
                        key: "empty".to_owned(),
                        bytes: 0,
                    },
                ],
            },
        )
        .expect("reconcile runs");
        assert_eq!(report.added, vec!["real".to_owned()]);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 10);
        // Exactly one row exists; the empty key recorded nothing.
        let snapshot = usage(&mut store).expect("usage reads");
        assert_eq!(
            snapshot.storage.iter().map(|row| row.objects).sum::<u64>(),
            1
        );
    }

    #[test]
    fn reconcile_prunes_stale_fairness_rows() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 1, "1", 5),
        );
        reconcile(
            &mut store,
            "2026-07-23T00:00:00.000Z",
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![],
            },
        )
        .expect("reconcile runs");
        let rows = store
            .rows("SELECT COUNT(*) AS n FROM principal_windows", &[])
            .expect("count reads");
        assert_eq!(rows[0]["n"], serde_json::json!(0));
    }

    #[test]
    fn wipe_clears_primary_storage_but_keeps_op_windows_backup_and_dump() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "a", 100),
        );
        allowed(&mut store, &limits, &commit(StoragePool::Backup, "k", 50));
        allowed(&mut store, &limits, &commit(StoragePool::Dump, "d", 30));
        allowed(
            &mut store,
            &limits,
            &consume_as(OpPool::BOrdinary, 5, "1", 5),
        );
        refused(&mut store, &limits, &reserve(StoragePool::Primary, "b", 1));
        wipe(&mut store).expect("wipe runs");
        // The primary storage rows are gone - the registry wipe deleted
        // those blobs - so primary capacity is free again.
        let snapshot = usage(&mut store).expect("usage reads");
        assert!(snapshot.storage.iter().all(|row| row.pool != "primary"));
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "b", 100),
        );
        // The backup and dump rows survive - the BACKUP bucket is never
        // wiped, so their objects keep billing.
        assert_eq!(pool_bytes(&mut store, StoragePool::Backup), 50);
        assert_eq!(pool_bytes(&mut store, StoragePool::Dump), 30);
        // The monthly op window survives too: the 5 ordinary reads were
        // real R2 operations Cloudflare already metered this month, and
        // reconciliation cannot rebuild op counters, so the wipe must
        // not re-mint their allowance. The pool stays exhausted until
        // the month rolls forward.
        assert_eq!(pool_used(&mut store, OpPool::BOrdinary), 5);
        refused(&mut store, &limits, &consume(OpPool::BOrdinary, 1));
    }

    #[test]
    fn malformed_limit_vars_fail_closed_to_zero() {
        let mut seen = Vec::new();
        let limits = Limits::from_lookup(
            |name| match name {
                "GOVERNOR_STORAGE_PRIMARY_BYTES" => Some("not-a-number".to_owned()),
                "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH" => Some("123".to_owned()),
                _ => None,
            },
            |name| seen.push(name.to_owned()),
        );
        // Set-but-garbage is a hard-cap typo: block the pool loudly.
        assert_eq!(limits.storage_primary_bytes, 0);
        assert_eq!(seen, vec!["GOVERNOR_STORAGE_PRIMARY_BYTES".to_owned()]);
        // Set-and-valid overrides; unset keeps the default.
        assert_eq!(limits.b_ordinary_month, 123);
        assert_eq!(limits.a_publish_month, Limits::default().a_publish_month);

        // A zero limit refuses everything on that pool.
        let mut store = Sqlite::new();
        refused(&mut store, &limits, &reserve(StoragePool::Primary, "a", 1));
    }

    #[test]
    fn oversized_and_malformed_requests_are_refused_upfront() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        for decision in [
            consume(OpPool::BOrdinary, 0),
            consume(OpPool::BOrdinary, MAX_CONSUME_N + 1),
            reserve(StoragePool::Primary, "", 1),
            reserve(StoragePool::Primary, "k", LIMIT_CLAMP + 1),
            Decision {
                consume: vec![Consume {
                    pool: OpPool::BOrdinary,
                    n: 1,
                    principal: Some("1".to_owned()),
                    principal_cap: None,
                }],
                ..Decision::default()
            },
        ] {
            let refusal = refused(&mut store, &limits, &decision);
            assert!(matches!(refusal, Refusal::Invalid { .. }), "{decision:?}");
        }
        assert_eq!(pool_used(&mut store, OpPool::BOrdinary), 0);
    }

    #[test]
    fn windows_parse_and_midnight_math_holds() {
        assert_eq!(utc_month("2026-07-22T12:00:00.000Z"), Some("2026-07"));
        assert_eq!(utc_month("garbage"), None);
        assert_eq!(utc_month(""), None);
        assert_eq!(utc_day("2026-07-22T12:00:00.000Z"), Some("2026-07-22"));
        assert_eq!(secs_to_next_utc_day("2026-07-22T00:00:00.000Z"), 86_400);
        assert_eq!(secs_to_next_utc_day("2026-07-22T23:59:59.000Z"), 1);
        assert_eq!(secs_to_next_utc_day("garbage"), 86_400);
    }

    #[test]
    fn the_protocol_round_trips_as_json() {
        let decision = Decision {
            consume: vec![Consume {
                pool: OpPool::BOrdinary,
                n: 1,
                principal: Some("7".to_owned()),
                principal_cap: Some(100),
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "blobs/sha256/ab".to_owned(),
                bytes: 42,
            }],
            commit: vec![],
            release: vec![],
        };
        let json = serde_json::to_string(&decision).expect("serializes");
        let back: Decision = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.consume[0].pool, OpPool::BOrdinary);
        assert_eq!(back.reserve[0].key, "blobs/sha256/ab");
        // Unknown fields are refused: the protocol stays narrow.
        assert!(serde_json::from_str::<Decision>(r#"{"steal":true}"#).is_err());
        let outcome = Outcome::refused(Refusal::PrincipalExhausted {
            pool: "b_ordinary".to_owned(),
            retry_after_secs: 60,
        });
        let json = serde_json::to_string(&outcome).expect("serializes");
        let back: Outcome = serde_json::from_str(&json).expect("deserializes");
        assert!(!back.ok);
        assert_eq!(
            back.refusal,
            Some(Refusal::PrincipalExhausted {
                pool: "b_ordinary".to_owned(),
                retry_after_secs: 60
            })
        );
    }

    #[test]
    fn a_key_cannot_be_committed_and_released_in_one_decision() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // commit ADDS the object's bytes, release REMOVES its row, and
        // `decide` runs commits before releases: applied together for
        // one key the net direction is non-conservative - the object
        // stays in R2 while the ledger loses its row, minting free
        // capacity. The engine refuses the ambiguous decision outright.
        let commit_then_release = Decision {
            commit: vec![Commit {
                pool: StoragePool::Primary,
                key: "k".to_owned(),
                bytes: 50,
            }],
            release: vec![Release {
                pool: StoragePool::Primary,
                key: "k".to_owned(),
            }],
            ..Decision::default()
        };
        assert!(matches!(
            refused(&mut store, &limits, &commit_then_release),
            Refusal::Invalid { .. }
        ));
        // Nothing was applied: no committed row, no deletion.
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 0);

        // reserve+release of one key, and a key duplicated in one list,
        // are refused the same way.
        let reserve_then_release = Decision {
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "k".to_owned(),
                bytes: 50,
            }],
            release: vec![Release {
                pool: StoragePool::Primary,
                key: "k".to_owned(),
            }],
            ..Decision::default()
        };
        assert!(matches!(
            refused(&mut store, &limits, &reserve_then_release),
            Refusal::Invalid { .. }
        ));

        // Distinct keys across commit and release stay allowed: a real
        // settle that commits one blob and releases another in one pass.
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "old", 10),
        );
        let distinct = Decision {
            commit: vec![Commit {
                pool: StoragePool::Primary,
                key: "new".to_owned(),
                bytes: 20,
            }],
            release: vec![Release {
                pool: StoragePool::Primary,
                key: "old".to_owned(),
            }],
            ..Decision::default()
        };
        allowed(&mut store, &limits, &distinct);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 20);
        // The same key under DIFFERENT pools is two distinct objects, so
        // it stays allowed WITHIN one decision (the validator keys on
        // pool AND key, not key alone).
        let cross_pool = Decision {
            reserve: vec![
                Reserve {
                    pool: StoragePool::Primary,
                    key: "shared".to_owned(),
                    bytes: 5,
                },
                Reserve {
                    pool: StoragePool::Backup,
                    key: "shared".to_owned(),
                    bytes: 5,
                },
            ],
            ..Decision::default()
        };
        allowed(&mut store, &limits, &cross_pool);
    }

    #[test]
    fn a_zero_byte_object_is_refused() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // No real archive, dump, or sidecar is empty; a 0-byte reserve
        // would insert a phantom row that later conflicts a real
        // reserve of the same content-addressed key.
        assert!(matches!(
            refused(&mut store, &limits, &reserve(StoragePool::Primary, "z", 0)),
            Refusal::Invalid { .. }
        ));
        assert!(matches!(
            refused(&mut store, &limits, &commit(StoragePool::Primary, "z", 0)),
            Refusal::Invalid { .. }
        ));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 0);
    }

    /// A [`Store`] that injects one storage failure on the Nth
    /// statement (0-indexed), to prove the engine surfaces storage
    /// errors - the caller then fails closed - instead of silently
    /// resolving to an allow.
    struct Flaky {
        inner: Sqlite,
        countdown: std::cell::Cell<i64>,
    }

    impl Flaky {
        fn failing_on(nth: i64) -> Flaky {
            Flaky {
                inner: Sqlite::new(),
                countdown: std::cell::Cell::new(nth),
            }
        }

        fn trip(&self) -> bool {
            let n = self.countdown.get();
            self.countdown.set(n - 1);
            n == 0
        }
    }

    impl Store for Flaky {
        fn exec(&mut self, sql: &str, params: &[Value]) -> Result<usize, String> {
            if self.trip() {
                return Err("injected storage failure".to_owned());
            }
            self.inner.run_statement(sql, params)
        }

        fn rows(&mut self, sql: &str, params: &[Value]) -> Result<Vec<serde_json::Value>, String> {
            if self.trip() {
                return Err("injected storage failure".to_owned());
            }
            self.inner.read_rows(sql, params)
        }
    }

    #[test]
    fn a_storage_failure_makes_decide_fail_closed_not_allow() {
        // The whole fail-closed contract: any Store error must surface
        // as Err so the client refuses before initiating a billable R2
        // call - never a silent Ok(allowed).
        let mut store = Flaky::failing_on(0);
        let err = decide(
            &mut store,
            &small_limits(),
            NOW,
            &reserve(StoragePool::Primary, "a", 10),
        )
        .expect_err("a storage failure must not resolve to an allow");
        assert!(err.contains("injected"));
    }

    #[test]
    fn a_failure_at_any_step_never_resolves_to_an_allow() {
        // A consume+reserve issues four statements (roll, consume,
        // state lookup, admission insert). Breaking ANY of them - even
        // after the consume already applied - must never yield an
        // allow; the caller treats the resulting error exactly like an
        // unreachable governor and refuses.
        let decision = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "a".to_owned(),
                bytes: 10,
            }],
            ..Decision::default()
        };
        for nth in 0..4 {
            let mut store = Flaky::failing_on(nth);
            let result = decide(&mut store, &small_limits(), NOW, &decision);
            assert!(
                !matches!(&result, Ok(outcome) if outcome.ok),
                "breaking statement {nth} must never resolve to an allow, got {result:?}"
            );
        }
    }

    #[test]
    fn usage_and_reconcile_surface_storage_failure() {
        // The breaker cron and admin route rely on usage()/reconcile()
        // returning Err - not an empty/zero snapshot - during a storage
        // incident, so they fail closed instead of reading a false
        // "healthy zero" that would disable the budget breaker.
        let mut store = Flaky::failing_on(0);
        assert!(usage(&mut store).is_err());
        let mut store = Flaky::failing_on(0);
        assert!(
            reconcile(
                &mut store,
                NOW,
                &ReconcileRequest {
                    pool: StoragePool::Primary,
                    live: vec![],
                },
            )
            .is_err()
        );
    }

    #[test]
    fn releasing_a_committed_object_frees_capacity() {
        let mut store = Sqlite::new();
        let limits = small_limits(); // dump limit 30
        // RELEASE_OBJECT is state-agnostic; the operator's evidence-
        // backed release and the dump self-release both free COMMITTED
        // rows, not just reserved ones. A "safety" refactor to delete
        // only reserved rows would silently leak the dump/backup pools.
        allowed(&mut store, &limits, &commit(StoragePool::Dump, "d/x", 30));
        refused(&mut store, &limits, &reserve(StoragePool::Dump, "d/y", 1));
        allowed(&mut store, &limits, &release(StoragePool::Dump, "d/x"));
        assert_eq!(pool_bytes(&mut store, StoragePool::Dump), 0);
        allowed(&mut store, &limits, &reserve(StoragePool::Dump, "d/y", 30));
    }

    #[test]
    fn reconcile_never_lowers_a_committed_byte_count() {
        let mut store = Sqlite::new();
        // Reconcile is increase-only: an authoritative set reporting a
        // SMALLER size than the ledger keeps the larger value (the MAX
        // upsert) and only reports the mismatch. Lowering it would
        // under-count real R2 usage - the one thing reconcile must
        // never do.
        allowed(
            &mut store,
            &small_limits(),
            &commit(StoragePool::Primary, "k", 40),
        );
        let report = reconcile(
            &mut store,
            NOW,
            &ReconcileRequest {
                pool: StoragePool::Primary,
                live: vec![LiveObject {
                    key: "k".to_owned(),
                    bytes: 10,
                }],
            },
        )
        .expect("reconcile runs");
        assert_eq!(report.mismatched, vec!["k".to_owned()]);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 40);
    }

    #[test]
    fn a_publish_lifecycle_settles_reserved_into_committed_once() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // The publish path in order: an existence-head consume
        // (BPublish), then a Class A consume plus a Primary reserve,
        // then the commit that settles reserved -> committed WITHOUT
        // inflating the pool.
        allowed(&mut store, &limits, &consume(OpPool::BPublish, 1));
        let admit = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "blobs/sha256/ab".to_owned(),
                bytes: 60,
            }],
            ..Decision::default()
        };
        allowed(&mut store, &limits, &admit);
        let snapshot = usage(&mut store).expect("usage reads");
        assert!(
            snapshot
                .storage
                .iter()
                .any(|row| row.state == "reserved" && row.bytes == 60)
        );
        allowed(
            &mut store,
            &limits,
            &commit(StoragePool::Primary, "blobs/sha256/ab", 60),
        );
        let snapshot = usage(&mut store).expect("usage reads");
        assert_eq!(snapshot.storage.len(), 1);
        assert_eq!(snapshot.storage[0].state, "committed");
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 60);
        assert_eq!(pool_used(&mut store, OpPool::APublish), 1);
        assert_eq!(pool_used(&mut store, OpPool::BPublish), 1);
    }

    #[test]
    fn a_replayed_admit_dedups_the_reserve_but_recharges_the_op() {
        let mut store = Sqlite::new();
        let limits = Limits {
            a_publish_month: 1_000,
            ..small_limits()
        };
        // A lost decide-response makes the caller retry the same admit.
        // The content-addressed reserve is idempotent (one storage
        // row), but the op consume has no idempotency key and recharges
        // - accepted because over-counting ops is conservative, and the
        // retry's R2 put targets the same object so storage stays
        // counted once.
        let admit = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: "blobs/sha256/ab".to_owned(),
                bytes: 60,
            }],
            ..Decision::default()
        };
        allowed(&mut store, &limits, &admit);
        allowed(&mut store, &limits, &admit);
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), 60);
        assert_eq!(pool_used(&mut store, OpPool::APublish), 2);
    }

    #[test]
    fn committed_plus_reserved_never_exceeds_the_limit() {
        let mut store = Sqlite::new();
        let limits = small_limits(); // primary 100
        // A deterministic pseudo-random sequence of reserves, settles,
        // and releases over a small key space (so the dedup and
        // conflict paths both fire and both ledger states coexist): the
        // admission invariant SUM(reserved+committed) <= limit must hold
        // after every step, whatever the interleaving.
        let mut seed = 1_u64;
        for step in 0..1_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let bytes = (seed >> 33) % 40 + 1;
            let key = format!("k{}", (seed >> 20) % 12);
            let reserved = decide(
                &mut store,
                &limits,
                NOW,
                &reserve(StoragePool::Primary, &key, bytes),
            )
            .expect("decide runs")
            .ok;
            // Settle some just-reserved keys into COMMITTED state at the
            // same bytes: reserved -> committed never changes the pool
            // sum, so both states mix into the ledger and the invariant
            // (which spans committed + reserved) still has to hold.
            if reserved && step % 3 == 0 {
                decide(
                    &mut store,
                    &limits,
                    NOW,
                    &commit(StoragePool::Primary, &key, bytes),
                )
                .expect("decide runs");
            }
            if step % 7 == 0 {
                decide(
                    &mut store,
                    &limits,
                    NOW,
                    &release(StoragePool::Primary, &key),
                )
                .expect("decide runs");
            }
            assert!(
                pool_bytes(&mut store, StoragePool::Primary) <= 100,
                "admission invariant broken at step {step}"
            );
        }
    }

    #[test]
    fn admission_is_overflow_free_at_the_clamp_boundary() {
        let mut store = Sqlite::new();
        let limits = Limits {
            storage_primary_bytes: LIMIT_CLAMP,
            ..small_limits()
        };
        // Reserve exactly the clamp against a clamp-sized limit: the
        // largest SUM(bytes)+bytes the admission arithmetic evaluates.
        // It must land exactly, and one more byte must refuse - no i64
        // wrap that would admit an over-limit reserve.
        allowed(
            &mut store,
            &limits,
            &reserve(StoragePool::Primary, "a", LIMIT_CLAMP),
        );
        refused(&mut store, &limits, &reserve(StoragePool::Primary, "b", 1));
        assert_eq!(pool_bytes(&mut store, StoragePool::Primary), LIMIT_CLAMP);
    }

    #[test]
    fn op_windows_roll_across_a_year_boundary_and_a_zero_limit_refuses() {
        let mut store = Sqlite::new();
        let limits = small_limits();
        // December's window is spent to the limit...
        let outcome = decide(
            &mut store,
            &limits,
            "2026-12-31T23:59:00.000Z",
            &consume(OpPool::BOrdinary, 5),
        )
        .expect("decide runs");
        assert!(outcome.ok);
        let outcome = decide(
            &mut store,
            &limits,
            "2026-12-31T23:59:30.000Z",
            &consume(OpPool::BOrdinary, 1),
        )
        .expect("decide runs");
        assert!(!outcome.ok);
        // ...January of the next year rolls it forward (lexical YYYY-MM
        // order holds: "2027-01" > "2026-12")...
        let outcome = decide(
            &mut store,
            &limits,
            "2027-01-01T00:00:00.000Z",
            &consume(OpPool::BOrdinary, 5),
        )
        .expect("decide runs");
        assert!(outcome.ok);
        // ...and a clock regressing back into December must not mint
        // fresh allowance.
        let outcome = decide(
            &mut store,
            &limits,
            "2026-12-31T23:59:59.000Z",
            &consume(OpPool::BOrdinary, 1),
        )
        .expect("decide runs");
        assert!(!outcome.ok, "a regressed clock must not reset the window");

        // A zero op limit (a GOVERNOR_R2_CLASS_B_*="0" typo fails closed
        // to zero) refuses every consume on that pool.
        let zero = Limits {
            b_source_month: 0,
            ..small_limits()
        };
        let mut store = Sqlite::new();
        assert!(matches!(
            refused(&mut store, &zero, &consume(OpPool::BSource, 1)),
            Refusal::PoolExhausted { .. }
        ));
    }
}
