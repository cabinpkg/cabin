//! The verification lifecycle's pure domain logic: the
//! `versions.verification` status values, the verdict request body and
//! transition rules, the artifact read gate, and the stuck-verifier
//! alert (`docs/remote-registry.md`, "Verification lifecycle").
//!
//! Fail-safe direction: nothing becomes resolvable or downloadable by
//! ordinary tokens unless its status is exactly `verified`, so a
//! verifier that never runs - or a status value that does not parse -
//! can only keep content unexposed, never expose it.

use serde::Deserialize;

use crate::error;

/// The `versions.verification` column. Every published version is
/// `pending` until the external verifier renders a verdict; only
/// `verified` versions are part of the registry (composed into package
/// documents, downloadable with ordinary tokens, immutable). `rejected`
/// versions never became part of the registry: their blob is reclaimed,
/// their quota refunded, and the same `(scope, name, version)` may be
/// republished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Verified,
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Verified => "verified",
            Status::Rejected => "rejected",
        }
    }

    /// Parses a stored column value. `None` (a value the schema never
    /// writes) must gate like a missing row - fail safe, never serve
    /// content whose status is unreadable.
    pub fn parse(value: &str) -> Option<Status> {
        match value {
            "pending" => Some(Status::Pending),
            "verified" => Some(Status::Verified),
            "rejected" => Some(Status::Rejected),
            _ => None,
        }
    }
}

/// A verifier's verdict on a pending version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Rejected,
}

/// The admin verdict request body, exactly
/// `{"verdict":"verified"|"rejected","reason":"...","checksum":"...",
/// "published_at":"..."}`; `reason` is required for rejections (it is
/// recorded on the row) and ignored otherwise. `checksum` and
/// `published_at` echo what the admin listing reported and bind the
/// verdict to exactly that row generation - a verdict computed against
/// one listing must never land on a replacement published meanwhile
/// (the checksum names the archive bytes; `published_at` changes on
/// every replacement, catching even a same-bytes republish with new
/// metadata). Both are **required** for verified verdicts (the
/// direction that exposes content: fail safe means naming what was
/// inspected) and optional for rejections (the conservative
/// direction).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerdictBody {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}

/// A parsed, validated verdict request.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedVerdict {
    pub verdict: Verdict,
    /// The recorded rejection reason; always present for rejections.
    pub reason: Option<String>,
    /// When present, the verdict applies only while the row stores
    /// exactly these bytes.
    pub checksum: Option<String>,
    /// When present, the verdict applies only to the row generation
    /// the listing reported.
    pub published_at: Option<String>,
}

/// Parses and validates a verdict request body.
///
/// # Errors
///
/// The fixed `400` detail string for a malformed body, an unknown
/// verdict value, a rejection without a reason, or a verified verdict
/// without its checksum + `published_at` binding.
pub fn parse_verdict(body: &[u8]) -> Result<ParsedVerdict, &'static str> {
    let Ok(VerdictBody {
        verdict,
        reason,
        checksum,
        published_at,
    }) = serde_json::from_slice::<VerdictBody>(body)
    else {
        return Err(error::INVALID_VERDICT_BODY);
    };
    let verdict = match verdict.as_str() {
        "verified" => Verdict::Verified,
        "rejected" => Verdict::Rejected,
        _ => return Err(error::INVALID_VERDICT_BODY),
    };
    let reason = match verdict {
        Verdict::Verified => None,
        Verdict::Rejected => match reason.filter(|reason| !reason.trim().is_empty()) {
            Some(reason) => Some(reason),
            None => return Err(error::VERDICT_REASON_REQUIRED),
        },
    };
    if verdict == Verdict::Verified
        && (checksum.as_deref().is_none_or(str::is_empty)
            || published_at.as_deref().is_none_or(str::is_empty))
    {
        return Err(error::VERDICT_BINDING_REQUIRED);
    }
    Ok(ParsedVerdict {
        verdict,
        reason,
        checksum,
        published_at,
    })
}

/// What a verdict on a version in `current` status does.
#[derive(Debug, PartialEq, Eq)]
pub enum Transition {
    /// Apply it: the version is pending and the verdict decides it.
    Apply,
    /// Idempotent repeat of the verdict already applied: `200`, no
    /// change.
    NoOp,
    /// `409` with this detail: a conflicting verdict on a verified
    /// version (immutability), or any verdict on a rejected version
    /// (republish is the recovery path - a late duplicate verdict must
    /// never race the replacement).
    Conflict(&'static str),
}

/// The verdict transition table.
pub fn transition(current: Status, verdict: Verdict) -> Transition {
    match (current, verdict) {
        (Status::Pending, _) => Transition::Apply,
        (Status::Verified, Verdict::Verified) => Transition::NoOp,
        (Status::Verified, Verdict::Rejected) => Transition::Conflict(error::VERSION_IMMUTABLE),
        (Status::Rejected, _) => Transition::Conflict(error::VERSION_REJECTED_REVERDICT),
    }
}

/// Whether the artifact route serves a version to this request:
/// verified versions to any valid token, pending versions only to the
/// `verify` scope (the verifier downloads the bytes it inspects), and
/// rejected versions to no one - their blob is reclaimed.
pub fn artifact_readable(status: Status, has_verify_scope: bool) -> bool {
    match status {
        Status::Verified => true,
        Status::Pending => has_verify_scope,
        Status::Rejected => false,
    }
}

/// The stuck-verifier alert for the breaker cron's webhook payload:
/// versions pending for over an hour mean the verifier is not keeping
/// up (or not running), and an unreadable count must alert rather than
/// pass as healthy.
pub fn stale_pending_alert(stale_pending: Option<u64>) -> Option<String> {
    match stale_pending {
        Some(0) => None,
        Some(count) => Some(format!(
            "{count} version(s) have been pending verification for over an hour"
        )),
        None => Some("the stale-pending count could not be read from d1".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_rejects_unknown_values() {
        for status in [Status::Pending, Status::Verified, Status::Rejected] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        for value in ["", "PENDING", "verified ", "unknown"] {
            assert_eq!(Status::parse(value), None, "value: {value:?}");
        }
    }

    const BOUND_VERIFIED: &[u8] =
        br#"{"verdict":"verified","checksum":"aa12","published_at":"2026-07-10T00:00:00.000Z"}"#;

    #[test]
    fn parse_verdict_accepts_the_two_verdicts() {
        assert_eq!(
            parse_verdict(BOUND_VERIFIED),
            Ok(ParsedVerdict {
                verdict: Verdict::Verified,
                reason: None,
                checksum: Some("aa12".to_owned()),
                published_at: Some("2026-07-10T00:00:00.000Z".to_owned()),
            })
        );
        // A reason on a verified verdict is accepted and ignored.
        assert_eq!(
            parse_verdict(
                br#"{"verdict":"verified","reason":"fine","checksum":"aa12","published_at":"t"}"#
            ),
            Ok(ParsedVerdict {
                verdict: Verdict::Verified,
                reason: None,
                checksum: Some("aa12".to_owned()),
                published_at: Some("t".to_owned()),
            })
        );
        assert_eq!(
            parse_verdict(br#"{"verdict":"rejected","reason":"malware"}"#),
            Ok(ParsedVerdict {
                verdict: Verdict::Rejected,
                reason: Some("malware".to_owned()),
                checksum: None,
                published_at: None,
            })
        );
        assert_eq!(
            parse_verdict(br#"{"verdict":"rejected","reason":"malware","checksum":"aa12"}"#),
            Ok(ParsedVerdict {
                verdict: Verdict::Rejected,
                reason: Some("malware".to_owned()),
                checksum: Some("aa12".to_owned()),
                published_at: None,
            })
        );
    }

    #[test]
    fn parse_verdict_requires_the_listing_binding_to_verify() {
        // Verifying is the direction that exposes content, so the
        // verdict must name the exact row generation it inspected -
        // both the archive checksum and the listing's published_at;
        // rejections (above) stay valid without either.
        for body in [
            br#"{"verdict":"verified"}"#.as_slice(),
            br#"{"verdict":"verified","checksum":"aa12"}"#,
            br#"{"verdict":"verified","published_at":"t"}"#,
            br#"{"verdict":"verified","checksum":"","published_at":"t"}"#,
            br#"{"verdict":"verified","checksum":"aa12","published_at":""}"#,
        ] {
            assert_eq!(
                parse_verdict(body),
                Err(error::VERDICT_BINDING_REQUIRED),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn parse_verdict_requires_a_reason_for_rejections() {
        for body in [
            br#"{"verdict":"rejected"}"#.as_slice(),
            br#"{"verdict":"rejected","reason":""}"#,
            br#"{"verdict":"rejected","reason":"  "}"#,
        ] {
            assert_eq!(
                parse_verdict(body),
                Err(error::VERDICT_REASON_REQUIRED),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn parse_verdict_rejects_malformed_bodies() {
        for body in [
            b"".as_slice(),
            b"not json",
            br#"{"verdict":"maybe"}"#,
            br#"{"verdict":"verified","extra":true}"#,
            br#"{"reason":"no verdict"}"#,
        ] {
            assert_eq!(
                parse_verdict(body),
                Err(error::INVALID_VERDICT_BODY),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn transition_covers_the_whole_matrix() {
        assert_eq!(
            transition(Status::Pending, Verdict::Verified),
            Transition::Apply
        );
        assert_eq!(
            transition(Status::Pending, Verdict::Rejected),
            Transition::Apply
        );
        // Repeating the applied verdict on a verified version is the
        // idempotent 200.
        assert_eq!(
            transition(Status::Verified, Verdict::Verified),
            Transition::NoOp
        );
        // Rejecting a verified version hits the immutability wall.
        assert_eq!(
            transition(Status::Verified, Verdict::Rejected),
            Transition::Conflict(error::VERSION_IMMUTABLE)
        );
        // Any verdict on a rejected version conflicts: republish is the
        // recovery path, and a late duplicate rejection must not race a
        // replacement that reset the row to pending.
        assert_eq!(
            transition(Status::Rejected, Verdict::Verified),
            Transition::Conflict(error::VERSION_REJECTED_REVERDICT)
        );
        assert_eq!(
            transition(Status::Rejected, Verdict::Rejected),
            Transition::Conflict(error::VERSION_REJECTED_REVERDICT)
        );
    }

    #[test]
    fn artifact_gate_serves_verified_to_all_and_pending_to_verify_only() {
        assert!(artifact_readable(Status::Verified, false));
        assert!(artifact_readable(Status::Verified, true));
        assert!(!artifact_readable(Status::Pending, false));
        assert!(artifact_readable(Status::Pending, true));
        assert!(!artifact_readable(Status::Rejected, false));
        assert!(!artifact_readable(Status::Rejected, true));
    }

    #[test]
    fn stale_pending_alerts_on_counts_and_on_unreadable_data() {
        assert_eq!(stale_pending_alert(Some(0)), None);
        assert_eq!(
            stale_pending_alert(Some(3)),
            Some("3 version(s) have been pending verification for over an hour".to_owned())
        );
        assert!(stale_pending_alert(None).is_some());
    }
}
