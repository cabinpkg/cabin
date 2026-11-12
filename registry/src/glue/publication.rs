//! The two publication passes. The rules live in `crate::publication`.
//!
//! The archive placement pass copies every verified revision's archive
//! from its upload to its own key in ARCHIVES, and the revision is
//! served from the moment that is recorded. Its work is the rows
//! themselves (verified, `archived_at` unset).
//!
//! The publish pass puts the documents that say what is served - each
//! changed package's, and the catalog - into INDEX. Its work is the
//! open rows of `publication_jobs`, one per change, written in the
//! batch that makes the change.
//!
//! Both records are in D1, so a pass that dies anywhere leaves its work
//! for the next one. A verdict's request places the archive and puts
//! that package's document at once. The cron runs both passes every
//! fifteen minutes, and only it puts the catalog and completes changes.
//! A yank's request puts nothing: yank has no quota, so it makes no R2
//! call, and the pass's work list bounds what yanks can draw from the
//! infrastructure pools (`governor::Limits`).
//!
//! Why INDEX holds a completed change from then on. Generations come
//! from one counter that only grows, a change takes the next one in the
//! batch that makes it, and a document is rendered from rows read in
//! one batch with its generation, so a rendering at a later generation
//! says everything an earlier one does. A writer puts its rendering
//! only where its head found nothing (the put creates) or found an
//! earlier generation (the put replaces an object with the entity tag
//! the head saw).
//!
//! That tag names the object's bytes, not the write that stored them.
//! When a document returns to earlier bytes (a yank, then the un-yank),
//! a writer that rendered between the two and whose head saw those
//! bytes may still replace the object, so INDEX can go back to an
//! earlier document. Hence the settle time
//! (`publication::SETTLE_MS`), which rests on one premise: every
//! writer whose rendering lacks a change has made its put before the
//! settle time has passed since the change was recorded
//! (`created_at`). Between a writer's read of D1 and its put are the
//! rendering, the governor's decision and a head, which take seconds.
//! Nothing enforces the premise: it fails when they, or the change's
//! own batch, stall for the settle time.
//!
//! The cron's pass completes a change only when the pass started a
//! settle time or more after it, and only after its own head or put
//! showed the package's document at the change's generation or later,
//! and then the catalog at a generation read after that. Under the
//! premise no put older than the change follows that head, and every
//! later put carries the change, so INDEX keeps it. Until then INDEX
//! may go back, and the next pass that reaches the package puts the
//! current document again.

use std::collections::HashMap;

use serde::Deserialize;
use worker::{
    Conditional, D1Database, D1PreparedStatement, D1Result, Date, Env, HttpMetadata, ResponseBody,
    console_error, console_log,
};

use crate::governor::{Consume, Decision, OpPool};
use crate::governor_client::{self, Gate};
use crate::{documents, publication, sql, upload};

use super::read::package_document;
use super::{changed_rows, iso8601_from_ms, js_int, non_negative, now_iso8601};

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

#[derive(Deserialize)]
struct PackageRecord {
    scope: String,
    name: String,
}

#[derive(Deserialize)]
struct GenerationRecord {
    generation: i64,
}

/// The governor admission of one pass's R2 calls for one object.
fn admission(reads: u32, writes: u32) -> Decision {
    Decision {
        consume: vec![
            Consume {
                pool: OpPool::BInfra,
                n: reads,
                principal: None,
                principal_cap: None,
            },
            Consume {
                pool: OpPool::AInfra,
                n: writes,
                principal: None,
                principal_cap: None,
            },
        ],
        ..Decision::default()
    }
}

pub(crate) fn result(results: &[D1Result], at: usize) -> worker::Result<&D1Result> {
    results
        .get(at)
        .ok_or_else(|| worker::Error::RustError(format!("missing batch result {at}")))
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
        if let Gate::Refused(_) = governor_client::decide(env, &admission(2, 1)).await {
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
    let batch = if placed {
        let mut batch = vec![db.prepare(sql::MARK_REVISION_ARCHIVED).bind(&[
            now.as_str().into(),
            scope.into(),
            name.into(),
            version.into(),
            revision.into(),
        ])?];
        batch.extend(document_changed(db, scope, name, &now)?);
        batch
    } else {
        console_error!(
            "archive placement: the upload of {scope}/{name}@{version}#{revision} no longer \
             holds the verified archive; rejecting it as {}",
            publication::SOURCE_LOST
        );
        vec![
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
        ]
    };
    if changed_rows(result(&db.batch(batch).await?, 0)?.meta()?) == 0 {
        console_log!(
            "archive placement: {scope}/{name}@{version}#{revision} moved on before it settled"
        );
    } else if placed {
        // The change is recorded, so a failure only logs and the
        // publish pass puts the document.
        if let Err(err) = publish_package(env, db, scope, name).await {
            console_error!("document publication: {scope}/{name}: {err}");
        }
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

/// The statements that count a change to what `scope/name`'s document
/// says and record it as work for the publish pass. They go directly
/// after the statement that makes the change, in its batch, and do
/// nothing unless it changed its row.
pub(crate) fn document_changed(
    db: &D1Database,
    scope: &str,
    name: &str,
    now: &str,
) -> worker::Result<[D1PreparedStatement; 3]> {
    Ok([
        db.prepare(sql::ADVANCE_CATALOG_GENERATION),
        db.prepare(sql::STAMP_PACKAGE_GENERATION)
            .bind(&[scope.into(), name.into()])?,
        db.prepare(sql::RECORD_PUBLICATION_JOB)
            .bind(&[scope.into(), name.into(), now.into()])?,
    ])
}

/// The publish pass: the documents of the packages with open changes,
/// then the catalog, then the completion of the changes that have
/// settled. Failures only log, like the placement pass's. The catalog
/// is rendered here alone because that reads every served version:
/// once per pass, whatever the number of changes.
pub(crate) async fn publish_documents(env: &Env, db: &D1Database) {
    if let Err(err) = try_publish(env, db).await {
        console_error!("document publication: {err}");
    }
}

async fn try_publish(env: &Env, db: &D1Database) -> worker::Result<()> {
    // Read before the pass's first head: see the module comment.
    let settled = iso8601_from_ms(
        Date::now()
            .as_millis()
            .saturating_sub(publication::SETTLE_MS),
    );
    // shortcut: a hundred packages whose document keeps failing would
    // starve the rest, like the placement pass's list.
    let packages: Vec<PackageRecord> = db
        .prepare(sql::UNPUBLISHED_PACKAGES)
        .all()
        .await?
        .results()?;
    let mut published = Vec::new();
    for PackageRecord { scope, name } in &packages {
        match publish_package(env, db, scope, name).await {
            Ok(Some(generation)) => published.push((scope, name, generation)),
            Ok(None) => {}
            Err(err) => console_error!("document publication: {scope}/{name}: {err}"),
        }
    }
    if published.is_empty() {
        return Ok(());
    }
    let results = db
        .batch(vec![
            db.prepare(sql::CATALOG_GENERATION),
            db.prepare(sql::CATALOG_VERSIONS),
        ])
        .await?;
    let generation = generation(result(&results, 0)?)?;
    let rows: Vec<documents::CatalogRow> = result(&results, 1)?.results()?;
    let body = documents::catalog_json(&rows).map_err(worker::Error::RustError)?;
    if !put_document(env, publication::CATALOG_KEY, &body, generation).await? {
        return Ok(());
    }
    let now = now_iso8601();
    let completions = published
        .into_iter()
        .map(|(scope, name, generation)| {
            db.prepare(sql::COMPLETE_PUBLICATION_JOBS).bind(&[
                now.as_str().into(),
                scope.into(),
                name.into(),
                js_int(generation),
                settled.as_str().into(),
            ])
        })
        .collect::<worker::Result<Vec<_>>>()?;
    db.batch(completions).await?;
    Ok(())
}

fn generation(result: &D1Result) -> worker::Result<i64> {
    result
        .results::<GenerationRecord>()?
        .first()
        .map(|record| record.generation)
        .ok_or_else(|| worker::Error::RustError("no generation row".to_owned()))
}

/// Puts one package's document into INDEX: the generation it was
/// rendered at, once INDEX held that rendering or a later one, or
/// `None` when it could not tell.
async fn publish_package(
    env: &Env,
    db: &D1Database,
    scope: &str,
    name: &str,
) -> worker::Result<Option<i64>> {
    let results = db
        .batch(vec![
            db.prepare(sql::PACKAGE_GENERATION)
                .bind(&[scope.into(), name.into()])?,
            db.prepare(sql::CURRENT_REVISIONS_BY_PACKAGE)
                .bind(&[scope.into(), name.into()])?,
            db.prepare(sql::VERIFIED_REVISIONS_BY_PACKAGE)
                .bind(&[scope.into(), name.into()])?,
        ])
        .await?;
    let generation = generation(result(&results, 0)?)?;
    let body = package_document(
        scope,
        name,
        result(&results, 1)?.results()?,
        result(&results, 2)?.results()?,
    )
    .map_err(worker::Error::RustError)?;
    let key = publication::document_key(scope, name);
    Ok(put_document(env, &key, &body, generation)
        .await?
        .then_some(generation))
}

/// Makes sure INDEX holds, under `key`, `body` - rendered at
/// `generation` - or a later rendering: `false` when it could not tell,
/// which leaves the work to the next pass. An object is replaced only
/// while its bytes are the ones the head saw, and created only while
/// nothing is there. A later rendering with those same bytes can still
/// be replaced: the module comment argues what holds all the same.
async fn put_document(env: &Env, key: &str, body: &str, generation: i64) -> worker::Result<bool> {
    let index = env.bucket("INDEX")?;
    if let Gate::Refused(_) = governor_client::decide(env, &admission(1, 1)).await {
        console_error!("document publication: the governor refused the write of {key}");
        return Ok(false);
    }
    let only_if = match index.head(key).await? {
        Some(held) => {
            let held_at = held.custom_metadata()?;
            let held_at = held_at.get(publication::GENERATION).map(String::as_str);
            if publication::holds_document(held_at, generation) {
                return Ok(true);
            }
            Conditional {
                etag_matches: Some(held.etag()),
                ..Conditional::default()
            }
        }
        None => Conditional {
            etag_does_not_match: Some("*".to_owned()),
            ..Conditional::default()
        },
    };
    let put = index
        .put(key, body.to_owned())
        .http_metadata(HttpMetadata {
            content_type: Some("application/json".to_owned()),
            cache_control: Some(publication::DOCUMENT_CACHE_CONTROL.to_owned()),
            ..HttpMetadata::default()
        })
        .custom_metadata(HashMap::from([(
            publication::GENERATION.to_owned(),
            generation.to_string(),
        )]))
        .only_if(only_if)
        .execute()
        .await?;
    if put.is_none() {
        console_error!("document publication: another write of {key} came first");
    }
    Ok(put.is_some())
}
