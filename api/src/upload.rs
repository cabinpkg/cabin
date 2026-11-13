//! Upload operations: the `Cabin-Upload-Id` a publish names itself
//! with, where its archive is stored, how long each stage may take, and
//! what a publish request means given the revisions already recorded.
//!
//! Pure and host-testable; `glue::bearer::package` supplies the rows
//! and performs the writes (`sql::admission`).

use crate::error;
use crate::verify::Status;

/// The request header carrying the upload id.
pub const HEADER: &str = "cabin-upload-id";

pub const INVALID_UPLOAD_ID: &str =
    "the Cabin-Upload-Id header is required and must be 32 lowercase hexadecimal digits";
/// One id names one operation: its user, revision, and bytes.
pub const UPLOAD_ID_REUSED: &str =
    "the Cabin-Upload-Id already names a different upload; a new upload needs a new id";
/// The operation the id names ended without a stored archive it could
/// still return to.
pub const UPLOAD_ID_SPENT: &str = "the upload this Cabin-Upload-Id names was rejected or has expired; publish again as a new upload";
/// The reservation was released under the request that took it, or the
/// request's clock is not past the released generation's stamp: only a
/// later request can take it again.
pub const RESERVATION_LOST: &str =
    "the upload's reservation expired before the archive was stored; retry the upload";

// The lifecycle's durations, as the `SQLite` time modifiers the
// statements in `sql::admission` apply to the request's clock sample.

/// How long an admitted upload may stay unconfirmed before the cron
/// checks its object and confirms or releases it. The body is fully
/// received before admission, so the store itself takes seconds; the
/// cron runs every fifteen minutes, so nothing shorter would be acted
/// on sooner.
pub const RESERVATION_TTL: &str = "-15 minutes";
/// How long after its first charge an operation may still return to its
/// released reservation. Returning never extends it.
pub const RETRY_WINDOW: &str = "-14 days";
/// How long a stored upload may await a verdict: UPLOADS expires
/// objects after fourteen days, and an upload whose object is gone can
/// no longer be verified.
pub const PENDING_TIMEOUT: &str = "-14 days";
/// How long a terminal row keeps its upload id.
pub const UPLOAD_ID_RETENTION: &str = "-30 days";

/// The upload id a request carries, when it is well-formed.
pub fn parse_id(header: Option<&str>) -> Option<&str> {
    header.filter(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// The UPLOADS key of an operation's archive. One key per operation:
/// a later upload of the same bytes is a new object with its own
/// lifetime, never an extension of this one's.
pub fn object_key(upload_id: &str) -> String {
    format!("uploads/{upload_id}.zip")
}

/// One recorded revision of the version a publish targets.
#[derive(Debug, Clone, Copy)]
pub struct Recorded<'a> {
    pub revision: &'a str,
    pub checksum: &'a str,
    /// `None` for a stored value outside the lifecycle's three.
    pub status: Option<Status>,
    pub upload_id: Option<&'a str>,
    /// Whether the archive was confirmed stored.
    pub stored: bool,
    pub published_at: &'a str,
    pub published_by: i64,
    /// Whether the operation's first charge is within [`RETRY_WINDOW`].
    pub resumable: bool,
}

/// One publish request, after its bytes were hashed.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub revision: &'a str,
    pub checksum: &'a str,
    pub upload_id: &'a str,
    pub user_id: i64,
    /// The request's clock sample, in the stored ISO 8601 shape.
    pub now: &'a str,
    /// The `new-revision` opt-in.
    pub new_revision: bool,
    /// Whether the upload id is bound to some other revision row.
    pub upload_id_bound_elsewhere: bool,
}

/// What a publish request means.
#[derive(Debug, PartialEq, Eq)]
pub enum Disposition<'a> {
    /// The bytes are already recorded and stored: the `200` no-op,
    /// reporting the revision's status. Nothing is charged or written.
    NoOp(Status),
    /// The bytes are admitted but not yet confirmed stored: make sure
    /// the reservation's object exists, then confirm it. Whoever sends
    /// the bytes - the original request retried, or another publish of
    /// the same archive - lands on the one operation.
    Converge {
        upload_id: &'a str,
        published_at: &'a str,
    },
    /// The same operation returning to its own released reservation: it
    /// takes a pending slot and the storage again and is not charged
    /// the daily allowances a second time.
    Resume,
    /// A new operation taking over a rejected revision with the same
    /// bytes: charged in full.
    Revive,
    /// A new operation creating the revision: charged in full.
    New,
    /// A `409` with this detail.
    Conflict(&'static str),
    /// A stored row the lifecycle cannot interpret.
    Corrupt,
}

/// Decides `request` against the version's recorded revisions. The
/// write batches re-enforce every rule here inside D1; this only shapes
/// the response, and a request that loses a race is decided again
/// against the winner's rows.
pub fn dispose<'a>(recorded: &[Recorded<'a>], request: &Request<'_>) -> Disposition<'a> {
    // A live revision with different bytes makes any new or returning
    // revision a respin, which needs the opt-in.
    let respin_refused = !request.new_revision
        && recorded.iter().any(|row| {
            row.checksum != request.checksum
                && matches!(row.status, Some(Status::Pending | Status::Verified))
        });
    let Some(row) = recorded.iter().find(|row| row.revision == request.revision) else {
        return if respin_refused {
            Disposition::Conflict(error::NEW_REVISION_REQUIRED)
        } else if request.upload_id_bound_elsewhere {
            Disposition::Conflict(UPLOAD_ID_REUSED)
        } else {
            Disposition::New
        };
    };
    if row.checksum != request.checksum {
        // Two archives whose digests share the revision's 16-hex
        // prefix: replacing either would break immutability.
        return Disposition::Conflict(error::REVISION_COLLISION);
    }
    match row.status {
        None => Disposition::Corrupt,
        Some(Status::Verified) => Disposition::NoOp(Status::Verified),
        Some(Status::Pending) if row.stored => Disposition::NoOp(Status::Pending),
        Some(Status::Pending) => match row.upload_id {
            Some(upload_id) => Disposition::Converge {
                upload_id,
                published_at: row.published_at,
            },
            None => Disposition::Corrupt,
        },
        Some(Status::Rejected) if respin_refused => {
            Disposition::Conflict(error::NEW_REVISION_REQUIRED)
        }
        Some(Status::Rejected) if row.upload_id == Some(request.upload_id) => {
            // A verifier's rejection and a timed-out upload both leave
            // `stored` set: returning under the same id would reuse an
            // object whose lifetime is already running out.
            if row.stored || !row.resumable || row.published_by != request.user_id {
                Disposition::Conflict(UPLOAD_ID_SPENT)
            } else if row.published_at >= request.now {
                // A returning generation needs a later stamp
                // (`RESUME_RELEASED_UPLOAD`), which this request cannot
                // carry.
                Disposition::Conflict(RESERVATION_LOST)
            } else {
                Disposition::Resume
            }
        }
        Some(Status::Rejected) if request.upload_id_bound_elsewhere => {
            Disposition::Conflict(UPLOAD_ID_REUSED)
        }
        Some(Status::Rejected) => Disposition::Revive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const OTHER_ID: &str = "ffffffffffffffffffffffffffffffff";

    #[test]
    fn an_upload_id_is_exactly_32_lowercase_hex_digits() {
        assert_eq!(parse_id(Some(ID)), Some(ID));
        for malformed in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789abcdef",
            "0123456789abcdeg0123456789abcdef",
            "0123456789abcdef 123456789abcdef",
        ] {
            assert_eq!(parse_id(Some(malformed)), None, "{malformed:?}");
        }
        assert_eq!(parse_id(None), None);
    }

    #[test]
    fn each_operation_has_its_own_object() {
        assert_eq!(
            object_key(ID),
            "uploads/0123456789abcdef0123456789abcdef.zip"
        );
        assert_ne!(object_key(ID), object_key(OTHER_ID));
    }

    fn row(status: Status) -> Recorded<'static> {
        Recorded {
            revision: "aaaa",
            checksum: "sha256:aaaa",
            status: Some(status),
            upload_id: Some(ID),
            stored: true,
            published_at: "2026-11-01T00:00:00.000Z",
            published_by: 7,
            resumable: true,
        }
    }

    fn request() -> Request<'static> {
        Request {
            revision: "aaaa",
            checksum: "sha256:aaaa",
            upload_id: ID,
            user_id: 7,
            now: "2026-11-02T00:00:00.000Z",
            new_revision: false,
            upload_id_bound_elsewhere: false,
        }
    }

    #[test]
    fn recorded_stored_bytes_are_a_no_op_under_any_upload_id() {
        for status in [Status::Pending, Status::Verified] {
            for upload_id in [ID, OTHER_ID] {
                let request = Request {
                    upload_id,
                    upload_id_bound_elsewhere: true,
                    ..request()
                };
                assert_eq!(dispose(&[row(status)], &request), Disposition::NoOp(status));
            }
        }
    }

    #[test]
    fn an_unstored_reservation_converges_on_its_own_operation() {
        let reservation = Recorded {
            stored: false,
            ..row(Status::Pending)
        };
        for upload_id in [ID, OTHER_ID] {
            assert_eq!(
                dispose(
                    &[reservation],
                    &Request {
                        upload_id,
                        user_id: 8,
                        ..request()
                    }
                ),
                Disposition::Converge {
                    upload_id: ID,
                    published_at: "2026-11-01T00:00:00.000Z",
                }
            );
        }
    }

    #[test]
    fn only_the_same_operation_resumes_and_only_a_released_reservation() {
        let released = Recorded {
            stored: false,
            ..row(Status::Rejected)
        };
        assert_eq!(dispose(&[released], &request()), Disposition::Resume);
        // Past the retry window, or presented by another user, the id
        // is spent.
        for spent in [
            Recorded {
                resumable: false,
                ..released
            },
            Recorded {
                published_by: 8,
                ..released
            },
            // Rejected by the verifier or timed out: the archive was
            // stored.
            row(Status::Rejected),
        ] {
            assert_eq!(
                dispose(&[spent], &request()),
                Disposition::Conflict(UPLOAD_ID_SPENT)
            );
        }
    }

    #[test]
    fn a_request_cannot_resume_a_reservation_released_under_it() {
        let released = Recorded {
            stored: false,
            ..row(Status::Rejected)
        };
        // The request that took the reservation carries its stamp; a
        // clock behind that stamp is refused the same way.
        for now in ["2026-11-01T00:00:00.000Z", "2026-10-31T23:59:59.999Z"] {
            assert_eq!(
                dispose(&[released], &Request { now, ..request() }),
                Disposition::Conflict(RESERVATION_LOST)
            );
        }
        let later = Request {
            now: "2026-11-01T00:00:00.001Z",
            ..request()
        };
        assert_eq!(dispose(&[released], &later), Disposition::Resume);
    }

    #[test]
    fn a_new_upload_id_revives_a_rejected_revision_as_a_new_operation() {
        for rejected in [
            row(Status::Rejected),
            Recorded {
                stored: false,
                resumable: false,
                ..row(Status::Rejected)
            },
            // The prune forgot the earlier operation.
            Recorded {
                upload_id: None,
                ..row(Status::Rejected)
            },
        ] {
            let request = Request {
                upload_id: OTHER_ID,
                ..request()
            };
            assert_eq!(dispose(&[rejected], &request), Disposition::Revive);
        }
    }

    #[test]
    fn an_upload_id_bound_to_another_revision_starts_nothing() {
        let bound_elsewhere = Request {
            upload_id: OTHER_ID,
            upload_id_bound_elsewhere: true,
            ..request()
        };
        assert_eq!(
            dispose(&[], &bound_elsewhere),
            Disposition::Conflict(UPLOAD_ID_REUSED)
        );
        assert_eq!(
            dispose(&[row(Status::Rejected)], &bound_elsewhere),
            Disposition::Conflict(UPLOAD_ID_REUSED)
        );
        assert_eq!(dispose(&[], &request()), Disposition::New);
    }

    #[test]
    fn different_bytes_beside_a_live_revision_need_the_opt_in() {
        let live = Recorded {
            revision: "bbbb",
            checksum: "sha256:bbbb",
            ..row(Status::Verified)
        };
        let rejected_sibling = Recorded {
            revision: "bbbb",
            checksum: "sha256:bbbb",
            ..row(Status::Rejected)
        };
        let opted_in = Request {
            new_revision: true,
            ..request()
        };
        // A new revision, a revival, and a resume alike.
        let released = Recorded {
            stored: false,
            ..row(Status::Rejected)
        };
        for own in [None, Some(released)] {
            let rows: Vec<_> = own.into_iter().chain([live]).collect();
            assert_eq!(
                dispose(&rows, &request()),
                Disposition::Conflict(error::NEW_REVISION_REQUIRED)
            );
            assert_ne!(
                dispose(&rows, &opted_in),
                Disposition::Conflict(error::NEW_REVISION_REQUIRED)
            );
        }
        // A rejected sibling is not live.
        assert_eq!(dispose(&[rejected_sibling], &request()), Disposition::New);
    }

    #[test]
    fn a_revision_id_shared_by_different_bytes_is_a_collision() {
        let request = Request {
            checksum: "sha256:aaaa-other-tail",
            new_revision: true,
            ..request()
        };
        for status in [Status::Pending, Status::Verified, Status::Rejected] {
            assert_eq!(
                dispose(&[row(status)], &request),
                Disposition::Conflict(error::REVISION_COLLISION)
            );
        }
    }

    #[test]
    fn rows_outside_the_lifecycle_are_corrupt() {
        let unknown_status = Recorded {
            status: None,
            ..row(Status::Pending)
        };
        assert_eq!(dispose(&[unknown_status], &request()), Disposition::Corrupt);
        let reservation_without_id = Recorded {
            stored: false,
            upload_id: None,
            ..row(Status::Pending)
        };
        assert_eq!(
            dispose(&[reservation_without_id], &request()),
            Disposition::Corrupt
        );
    }
}
