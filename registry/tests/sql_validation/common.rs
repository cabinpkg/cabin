//! Shared fixtures: the migrated in-memory database and the helpers
//! more than one statement module exercises.

use std::sync::atomic::{AtomicU64, Ordering};

use cabin_registry_worker::{quota, sql, upload};
use rusqlite::types::Value;
use rusqlite::{Connection, StatementStatus};

#[path = "../../src/test_db.rs"]
mod test_db;

pub use test_db::migrated_connection;

/// One class's limits as the publish glue binds them into the admission
/// gates: D1 binds every number as a REAL.
#[derive(Clone, Copy)]
pub struct Limits {
    pub bytes: f64,
    pub versions_daily: f64,
    pub packages_daily: f64,
    pub packages_total: f64,
    pub upload_bytes_daily: f64,
    pub uploads_daily: f64,
    pub shared_bytes_daily: f64,
    pub shared_uploads_daily: f64,
    pub pending: f64,
}

impl Limits {
    #[allow(clippy::cast_precision_loss)]
    pub fn of(class: &str) -> Self {
        let quotas = quota::quotas_for_class(class);
        Self {
            bytes: quotas.max_total_bytes_per_user as f64,
            versions_daily: quotas.max_versions_per_package_per_day as f64,
            packages_daily: quotas.max_new_packages_per_day as f64,
            packages_total: quotas.max_packages_total as f64,
            upload_bytes_daily: quotas.max_upload_bytes_per_day as f64,
            uploads_daily: quotas.max_uploads_per_day as f64,
            shared_bytes_daily: quotas.shared_upload_bytes_per_day as f64,
            shared_uploads_daily: quotas.shared_uploads_per_day as f64,
            pending: quotas.max_pending_uploads as f64,
        }
    }
}

/// One upload operation as the publish glue binds it into an admission
/// batch. `revision` doubles as the checksum, as it does in the seeds.
#[derive(Clone, Copy)]
pub struct Op<'a> {
    pub user: f64,
    /// The token's quota class: the shared allowance it is charged to.
    pub class: &'a str,
    pub scope: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub revision: &'a str,
    pub metadata: &'a str,
    pub size: f64,
    pub opt_in: bool,
    /// The request's clock sample.
    pub stamp: &'a str,
    pub upload_id: &'a str,
}

/// An upload id no other call returned.
pub fn fresh_upload_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("{:032x}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

/// Runs one batch's statements, in order, as one transaction; each
/// statement's changed rows and VM steps.
pub fn batch(conn: &Connection, statements: Vec<(&str, Vec<Value>)>) -> Vec<(usize, i32)> {
    let tx = conn.unchecked_transaction().expect("begin");
    let results = statements
        .into_iter()
        .map(|(statement, values)| {
            let mut prepared = tx.prepare(statement).expect("prepare");
            let changed = prepared
                .execute(rusqlite::params_from_iter(values))
                .expect(statement);
            (changed, prepared.get_status(StatementStatus::VmStep))
        })
        .collect();
    tx.commit().expect("commit");
    results
}

/// The bind list the three admission gates share, as
/// `glue::bearer::package::admit_upload` builds it; `slot_14` is the
/// one position whose meaning differs between them.
fn gate(op: &Op, limits: &Limits, slot_14: Value) -> Vec<Value> {
    vec![
        text(op.stamp),
        op.user.into(),
        op.size.into(),
        text(op.scope),
        text(op.name),
        text(op.version),
        text(op.revision),
        text(op.revision),
        f64::from(u8::from(op.opt_in)).into(),
        text(op.upload_id),
        text(quota::shared_class(op.class)),
        limits.bytes.into(),
        limits.versions_daily.into(),
        slot_14,
        limits.packages_total.into(),
        limits.upload_bytes_daily.into(),
        limits.uploads_daily.into(),
        limits.shared_bytes_daily.into(),
        limits.shared_uploads_daily.into(),
        limits.pending.into(),
    ]
}

fn charge_shared<'a>(op: &Op) -> (&'a str, Vec<Value>) {
    (
        sql::CHARGE_SHARED_DAILY_UPLOAD,
        vec![
            text(op.stamp),
            text(quota::shared_class(op.class)),
            op.size.into(),
        ],
    )
}

fn count_stored_bytes<'a>(op: &Op) -> (&'a str, Vec<Value>) {
    (
        sql::COUNT_USER_STORED_BYTES,
        vec![op.user.into(), op.size.into()],
    )
}

/// The admission batch of a new revision, in the glue's statement
/// order. Its last statement's changed row is the admission.
pub fn admit_new(conn: &Connection, op: &Op, limits: &Limits) -> Vec<(usize, i32)> {
    batch(
        conn,
        vec![
            (
                sql::ADMIT_NEW_UPLOAD,
                gate(op, limits, limits.packages_daily.into()),
            ),
            charge_shared(op),
            (
                sql::COUNT_ADMITTED_UPLOAD,
                vec![
                    op.user.into(),
                    op.size.into(),
                    text(op.scope),
                    text(op.name),
                ],
            ),
            (
                sql::ENSURE_PACKAGE,
                vec![
                    text(op.scope),
                    text(op.name),
                    text(op.stamp),
                    op.user.into(),
                ],
            ),
            (
                sql::ENSURE_VERSION,
                vec![text(op.scope), text(op.name), text(op.version)],
            ),
            (
                sql::INSERT_ADMITTED_REVISION,
                vec![
                    text(op.scope),
                    text(op.name),
                    text(op.version),
                    text(op.revision),
                    text(op.revision),
                    text(op.metadata),
                    text(op.stamp),
                    op.size.into(),
                    op.user.into(),
                    text(op.upload_id),
                    text(quota::shared_class(op.class)),
                ],
            ),
        ],
    )
}

/// The admission batch of a revival. Its first statement's changed row
/// is the admission.
pub fn admit_revival(conn: &Connection, op: &Op, limits: &Limits) -> Vec<(usize, i32)> {
    let mut gate = gate(op, limits, limits.packages_daily.into());
    gate.push(text(op.metadata));
    batch(
        conn,
        vec![
            (sql::REVIVE_REJECTED_UPLOAD, gate),
            (
                sql::CHARGE_USER_DAILY_UPLOAD,
                vec![text(op.stamp), op.user.into(), op.size.into()],
            ),
            charge_shared(op),
            count_stored_bytes(op),
        ],
    )
}

/// The admission batch of a resumed operation. Its first statement's
/// changed row is the admission.
pub fn admit_resumed(conn: &Connection, op: &Op, limits: &Limits) -> Vec<(usize, i32)> {
    batch(
        conn,
        vec![
            (
                sql::RESUME_RELEASED_UPLOAD,
                gate(op, limits, text(upload::RETRY_WINDOW)),
            ),
            count_stored_bytes(op),
        ],
    )
}

/// One store confirmation of the generation stamped `published_at`;
/// its changed rows.
pub fn confirm(conn: &Connection, op: &Op, published_at: &str, stored_at: &str) -> usize {
    conn.execute(
        sql::CONFIRM_UPLOAD_STORED,
        rusqlite::params![
            stored_at,
            op.scope,
            op.name,
            op.version,
            op.revision,
            op.upload_id,
            published_at
        ],
    )
    .expect("CONFIRM_UPLOAD_STORED")
}

fn refund<'a>(op: &Op) -> (&'a str, Vec<Value>) {
    (
        sql::REFUND_USER_STORED_BYTES,
        vec![
            text(op.scope),
            text(op.name),
            text(op.version),
            text(op.revision),
        ],
    )
}

/// The verifier's rejection batch against the generation stamped
/// `published_at`; the flip's changed rows.
pub fn reject(conn: &Connection, op: &Op, published_at: &str, now: &str) -> usize {
    batch(
        conn,
        vec![
            (
                sql::MARK_REVISION_REJECTED,
                vec![
                    text("bad"),
                    text(op.scope),
                    text(op.name),
                    text(op.version),
                    text(op.revision),
                    text(published_at),
                    text(op.revision),
                    text(now),
                ],
            ),
            refund(op),
        ],
    )[0]
    .0
}

/// The cron's release batch for one expired reservation of the
/// generation stamped `published_at`; the flip's changed rows.
pub fn release(conn: &Connection, op: &Op, published_at: &str, now: &str) -> usize {
    batch(
        conn,
        vec![
            (
                sql::RELEASE_EXPIRED_RESERVATION,
                vec![
                    text(now),
                    text(op.scope),
                    text(op.name),
                    text(op.version),
                    text(op.revision),
                    text(published_at),
                ],
            ),
            refund(op),
        ],
    )[0]
    .0
}

/// The cron's sweep batch (`glue::cron::sweep_uploads`) at `now`.
pub fn sweep(conn: &Connection, now: &str) {
    let timeout = || vec![text(now), text(upload::PENDING_TIMEOUT)];
    batch(
        conn,
        vec![
            (sql::REFUND_TIMED_OUT_UPLOADS, timeout()),
            (sql::TIME_OUT_PENDING_UPLOADS, timeout()),
            (sql::PRUNE_USER_DAILY_UPLOADS, vec![text(now)]),
            (sql::PRUNE_SHARED_DAILY_UPLOADS, vec![text(now)]),
            (
                sql::PRUNE_SETTLED_UPLOAD_IDS,
                vec![text(now), text(upload::UPLOAD_ID_RETENTION)],
            ),
        ],
    );
}

/// The rows of `query`, one string each.
pub fn rows(conn: &Connection, query: &str) -> Vec<String> {
    let mut statement = conn.prepare(query).expect("prepare");
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, Value>(column).map(|cell| format!("{cell:?}")))
                .collect::<Result<Vec<_>, _>>()
                .map(|cells| cells.join(" | "))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// Every row an admission, a verdict, or a sweep can write.
pub fn footprint(conn: &Connection) -> Vec<String> {
    [
        "SELECT * FROM packages",
        "SELECT * FROM versions",
        "SELECT * FROM revisions",
        "SELECT * FROM user_daily_upload_usage",
        "SELECT * FROM shared_daily_upload_usage",
        "SELECT id, package_count, stored_bytes FROM users",
    ]
    .iter()
    .flat_map(|query| rows(conn, query))
    .collect()
}

/// One sign-in's identity upsert, exactly as the OAuth callback runs it:
/// both statements back-to-back on one connection, user creation first
/// (a D1 batch is one transaction on one connection, so the
/// `last_insert_rowid()` coupling behaves identically there).
pub fn sign_in(conn: &Connection, provider: &str, account_id: &str, login: &str, now: &str) {
    conn.execute(
        sql::INSERT_USER_FOR_NEW_IDENTITY,
        rusqlite::params![now, provider, account_id],
    )
    .expect("insert user for new identity");
    conn.execute(
        sql::UPSERT_IDENTITY,
        rusqlite::params![provider, account_id, login],
    )
    .expect("upsert identity");
}

/// The `(user_id, login_snapshot, quota_class)` the session plane
/// resolves for an identity, if any.
pub fn resolve(
    conn: &Connection,
    provider: &str,
    account_id: &str,
) -> Option<(i64, String, String)> {
    conn.query_row(
        sql::USER_BY_IDENTITY,
        rusqlite::params![provider, account_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .expect("resolve identity")
}

pub fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count rows")
}

/// Records every verified revision's archive as placed, as the
/// placement pass would: a seed's verified rows are served ones unless
/// the test says otherwise.
pub fn archive_verified(conn: &Connection) {
    conn.execute(
        "UPDATE revisions SET archived_at = '2026-07-19T00:00:00.000Z' \
         WHERE verification = 'verified' AND archived_at IS NULL",
        [],
    )
    .expect("archive the verified revisions");
}

/// Seeds one user, two scopes the user is a member of, and the same
/// `(name, version)` under both, each with one stored revision - the
/// collision the scoped statements must keep apart. alpha's is served,
/// beta's pending.
pub fn seed_scope_collision(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '1', '2026-07-15T00:00:00.000Z'),
                  ('beta', 'github', '2', '2026-07-15T00:00:00.000Z');
         INSERT INTO scope_members (scope_name, user_id, role) VALUES ('alpha', 2, 'owner');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'pkg', '2026-07-15T00:00:00.000Z', 2),
                  ('beta', 'pkg', '2026-07-15T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version) VALUES ('alpha', 'pkg', '1.0.0'),
                  ('beta', 'pkg', '1.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
                                published_at, archive_size, published_by, verification, \
                                upload_id, stored_at)
           VALUES ('alpha', 'pkg', '1.0.0', 'aa', 'aa', '{}', '2026-07-15T00:00:00.000Z', 10, 2,
                   'verified', 'seed-aa', '2026-07-15T00:00:00.000Z'),
                  ('beta', 'pkg', '1.0.0', 'bb', 'bb', '{}', '2026-07-15T00:00:00.000Z', 20, 2,
                   'pending', 'seed-bb', '2026-07-15T00:00:00.000Z');
         UPDATE users SET package_count = 2, stored_bytes = 30 WHERE id = 2;",
    )
    .expect("seed the cross-scope collision");
    archive_verified(conn);
}
