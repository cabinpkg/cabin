//! The archive placement pass: every verified revision's archive is
//! copied from its upload to its own key in ARCHIVES, and the revision
//! is served from the moment that is recorded. The rules live in
//! `crate::publication`.
//!
//! The work is the rows themselves (verified, `archived_at` unset), so
//! a pass that dies anywhere leaves it for the next one: the verdict
//! that verifies a revision runs a pass at once, and the cron runs one
//! every fifteen minutes.

use serde::Deserialize;
use worker::{
    Conditional, D1Database, Env, HttpMetadata, ResponseBody, console_error, console_log,
};

use crate::governor::{Consume, Decision, OpPool};
use crate::governor_client::{self, Gate};
use crate::{publication, sql, upload};

use super::{changed_rows, non_negative, now_iso8601};

#[derive(Deserialize)]
struct UnarchivedRecord {
    scope: String,
    name: String,
    version: String,
    revision: String,
    checksum: String,
    archive_size: i64,
    upload_id: Option<String>,
    published_at: String,
}

/// One placement pass. Failures only log: the rows stay listed for the
/// next pass, and one revision's failure does not stop the others.
pub(crate) async fn place_archives(env: &Env, db: &D1Database) {
    if let Err(err) = try_place_archives(env, db).await {
        console_error!("archive placement: {err}");
    }
}

async fn try_place_archives(env: &Env, db: &D1Database) -> worker::Result<()> {
    // shortcut: a revision whose placement keeps failing stays at the
    // head of the list, so twenty of them would starve the rest; order
    // by a last-attempt time if that ever happens.
    let unarchived: Vec<UnarchivedRecord> = db
        .prepare(sql::UNARCHIVED_REVISIONS)
        .all()
        .await?
        .results()?;
    for record in &unarchived {
        // One admission for the three R2 calls a placement can make:
        // the head of the archive key, the read of the upload, the put.
        let decision = Decision {
            consume: vec![
                Consume {
                    pool: OpPool::BInfra,
                    n: 2,
                    principal: None,
                    principal_cap: None,
                },
                Consume {
                    pool: OpPool::AInfra,
                    n: 1,
                    principal: None,
                    principal_cap: None,
                },
            ],
            ..Decision::default()
        };
        if let Gate::Refused(_) = governor_client::decide(env, &decision).await {
            console_error!("archive placement: the governor refused the copy; stopping");
            return Ok(());
        }
        if let Err(err) = place_archive(env, db, record).await {
            let UnarchivedRecord {
                scope,
                name,
                version,
                revision,
                ..
            } = record;
            console_error!("archive placement: {scope}/{name}@{version}#{revision}: {err}");
        }
    }
    Ok(())
}

/// Places one revision's archive and records the outcome: the revision
/// is served from then on, or - when its upload cannot be placed - is
/// rejected so that it can be published again. Both writes are guarded
/// on the row still being verified and unplaced.
async fn place_archive(
    env: &Env,
    db: &D1Database,
    record: &UnarchivedRecord,
) -> worker::Result<()> {
    let Some(placed) = copy_archive(env, record).await? else {
        return Ok(());
    };
    let UnarchivedRecord {
        scope,
        name,
        version,
        revision,
        published_at,
        ..
    } = record;
    let now = now_iso8601();
    let result = if placed {
        db.prepare(sql::MARK_REVISION_ARCHIVED)
            .bind(&[
                now.as_str().into(),
                scope.into(),
                name.into(),
                version.into(),
                revision.into(),
            ])?
            .run()
            .await?
    } else {
        console_error!(
            "archive placement: the upload of {scope}/{name}@{version}#{revision} no longer \
             holds the verified archive; rejecting it as {}",
            publication::SOURCE_LOST
        );
        db.batch(vec![
            db.prepare(sql::REJECT_UNARCHIVED_REVISION).bind(&[
                publication::SOURCE_LOST.into(),
                now.as_str().into(),
                scope.into(),
                name.into(),
                version.into(),
                revision.into(),
                published_at.into(),
            ])?,
            db.prepare(sql::REFUND_USER_STORED_BYTES).bind(&[
                scope.into(),
                name.into(),
                version.into(),
                revision.into(),
            ])?,
        ])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| worker::Error::RustError("missing batch result 0".to_owned()))?
    };
    if changed_rows(result.meta()?) == 0 {
        console_log!(
            "archive placement: {scope}/{name}@{version}#{revision} moved on before it settled"
        );
    }
    Ok(())
}

/// Makes sure the revision's archive is in ARCHIVES: `Some(true)` when
/// it is, `Some(false)` when it cannot be - the upload is gone, or no
/// longer holds the verified bytes - and `None` when another pass
/// created the object meanwhile, which the next pass compares. An
/// object already under the key is the archive or it is an error; it
/// is never replaced. The copy streams, R2 checking the bytes against
/// the revision's SHA-256 as it stores them.
async fn copy_archive(env: &Env, record: &UnarchivedRecord) -> worker::Result<Option<bool>> {
    let UnarchivedRecord {
        scope,
        name,
        version,
        revision,
        checksum,
        archive_size,
        upload_id,
        ..
    } = record;
    let size = non_negative(*archive_size);
    let key = publication::archive_key(scope, name, version, revision);
    let archives = env.bucket("ARCHIVES")?;
    if let Some(existing) = archives.head(&key).await? {
        if publication::holds_archive(
            existing.size(),
            existing.checksum().sha256.as_deref(),
            size,
            checksum,
        ) {
            return Ok(Some(true));
        }
        return Err(worker::Error::RustError(format!(
            "{key} is in ARCHIVES with other contents and is not replaced"
        )));
    }
    let upload = match upload_id {
        Some(upload_id) => {
            env.bucket("UPLOADS")?
                .get(upload::object_key(upload_id))
                .execute()
                .await?
        }
        None => None,
    };
    let Some(upload) = upload.filter(|object| {
        publication::holds_archive(
            object.size(),
            object.checksum().sha256.as_deref(),
            size,
            checksum,
        )
    }) else {
        return Ok(Some(false));
    };
    let (Some(body), Some(digest)) = (upload.body(), publication::digest(checksum)) else {
        return Err(worker::Error::RustError(
            "the upload has no body".to_owned(),
        ));
    };
    let ResponseBody::Stream(stream) = body.response_body()? else {
        return Err(worker::Error::RustError(
            "the upload's body is not a stream".to_owned(),
        ));
    };
    let put = archives
        .put(&key, stream)
        .sha256(digest)
        .http_metadata(HttpMetadata {
            content_type: Some("application/zip".to_owned()),
            cache_control: Some(publication::CACHE_CONTROL.to_owned()),
            ..HttpMetadata::default()
        })
        .only_if(Conditional {
            etag_does_not_match: Some("*".to_owned()),
            ..Conditional::default()
        })
        .execute()
        .await?;
    Ok(put.map(|_| true))
}
