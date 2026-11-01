//! The verification lifecycle's pure domain logic: the
//! `revisions.verification` status values, the verdict request body and
//! transition rules, the artifact read gate, and the stuck-verifier
//! alert (`docs/remote-registry.md`, "Verification lifecycle").
//!
//! Fail-safe direction: nothing becomes resolvable or downloadable by
//! ordinary tokens unless its status is exactly `verified`, so a
//! verifier that never runs - or a status value that does not parse -
//! can only keep content unexposed, never expose it.

use serde::Deserialize;

use crate::error;

/// The `revisions.verification` column. Every published revision is
/// `pending` until the external verifier renders a verdict; only
/// `verified` revisions are part of the registry (composed into package
/// documents, downloadable with ordinary tokens, immutable). `rejected`
/// revisions never became part of the registry: their blob is reclaimed
/// (unless another live revision stores the same bytes), their quota
/// refunded, and the same `(scope, name, version)` may be republished.
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

/// A verifier's verdict on a pending revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Rejected,
}

/// The admin verdict request body, exactly
/// `{"verdict":"verified"|"rejected","reason":"...","checksum":"...",
/// "published_at":"..."}`; `reason` is required for rejections (it is
/// recorded on the row) and ignored otherwise. `checksum` and
/// `published_at` are required for both verdicts; they echo what the
/// admin listing reported and bind the verdict to exactly that row
/// generation. The checksum names the revision the verdict targets, and
/// `published_at` names the publish event: a byte-identical revival
/// regenerates the row under the same checksum with a new
/// `published_at`, so without the generation bind a delayed verdict
/// computed against the rejected generation's listing would land on the
/// revived one.
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

/// 400 detail for a verdict whose `checksum` is not the canonical
/// `sha256:<64 lowercase hex>` spelling.  The binding compares
/// against the stored column, which holds exactly that spelling, so
/// any other shape could never name a revision.
pub const INVALID_VERDICT_CHECKSUM: &str =
    "verdict checksum must be `sha256:` followed by 64 lowercase hexadecimal characters";

/// A parsed, validated verdict request.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedVerdict {
    pub verdict: Verdict,
    /// The recorded rejection reason; always present for rejections.
    pub reason: Option<String>,
    /// The bytes the verdict targets.  Always present: the checksum
    /// is what names the revision row (`revision` is its leading hex
    /// prefix), so a verdict without one would be ambiguous the
    /// moment a pending respin sits beside the version's other
    /// revisions.
    pub checksum: String,
    /// The publish event the verdict targets.  Always present: a
    /// byte-identical revival regenerates the row under the same
    /// checksum, so only the pair pins one generation.
    pub published_at: String,
}

/// Parses and validates a verdict request body.
///
/// # Errors
///
/// The fixed `400` detail string for a malformed body, an unknown
/// verdict value, a rejection without a reason, a missing or empty
/// `checksum` or `published_at` binding (either verdict), or a
/// non-canonical checksum.
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
    let Some(checksum) = checksum.filter(|checksum| !checksum.is_empty()) else {
        return Err(error::VERDICT_BINDING_REQUIRED);
    };
    let Some(published_at) = published_at.filter(|stamp| !stamp.is_empty()) else {
        return Err(error::VERDICT_BINDING_REQUIRED);
    };
    if !crate::checksum::is_canonical(&checksum) {
        return Err(INVALID_VERDICT_CHECKSUM);
    }
    Ok(ParsedVerdict {
        verdict,
        reason,
        checksum,
        published_at,
    })
}

/// What a verdict on a revision in `current` status does.
#[derive(Debug, PartialEq, Eq)]
pub enum Transition {
    /// Apply it: the revision is pending and the verdict decides it.
    Apply,
    /// Idempotent repeat of the verdict already applied: `200`, no
    /// change.
    NoOp,
    /// `409` with this detail: a conflicting verdict on a verified
    /// revision (immutability), or a verifying verdict on a rejected
    /// revision (republish is the recovery path). A repeat of the
    /// applied verdict is a [`Transition::NoOp`] for both terminal
    /// states: the caller checks the generation binding before
    /// consulting the transition, so a matching repeat is the same
    /// verdict retried after a lost response - a late duplicate racing
    /// a revival carries the old `published_at` and answers `409` as
    /// target-changed before this table is reached.
    Conflict(&'static str),
}

/// The verdict transition table.
pub fn transition(current: Status, verdict: Verdict) -> Transition {
    match (current, verdict) {
        (Status::Pending, _) => Transition::Apply,
        (Status::Verified, Verdict::Verified) | (Status::Rejected, Verdict::Rejected) => {
            Transition::NoOp
        }
        (Status::Verified, Verdict::Rejected) => Transition::Conflict(error::VERSION_IMMUTABLE),
        (Status::Rejected, Verdict::Verified) => {
            Transition::Conflict(error::VERSION_REJECTED_REVERDICT)
        }
    }
}

/// Whether the artifact route serves a revision to this request:
/// verified revisions to everyone (reads are public), pending revisions
/// only to the `verify` scope (the verifier downloads the bytes it
/// inspects), and rejected revisions to no one - their blob is
/// reclaimed.
pub fn artifact_readable(status: Status, has_verify_scope: bool) -> bool {
    match status {
        Status::Verified => true,
        Status::Pending => has_verify_scope,
        Status::Rejected => false,
    }
}

/// One row of the admin corpus listing: a package plus whether any of
/// its versions is verified - the name was accepted once, either by
/// the advisories proceeding or by an operator's manual verdict.
/// Deliberately not "has any verdict": a rejection never vets a name.
pub struct CorpusPackage {
    pub scope: String,
    pub name: String,
    pub vetted: bool,
}

/// `GET /api/v1/admin/packages` (`docs/remote-registry.md`, "Admin
/// API"): the corpus the verifier's name advisories compare a
/// candidate against. Deliberately minimal - the names plus the
/// vetted-once bit - and deterministic (the query orders by scope,
/// then name).
pub fn packages_json(packages: &[CorpusPackage]) -> String {
    let entries: Vec<serde_json::Value> = packages
        .iter()
        .map(|package| {
            serde_json::json!({
                "scope": package.scope,
                "name": package.name,
                "vetted": package.vetted,
            })
        })
        .collect();
    serde_json::json!({ "packages": entries }).to_string()
}

/// The stuck-verifier alert for the breaker cron's webhook payload:
/// revisions pending for over an hour mean the verifier is not keeping
/// up (or not running), and an unreadable count must alert rather than
/// pass as healthy.
pub fn stale_pending_alert(stale_pending: Option<u64>) -> Option<String> {
    match stale_pending {
        Some(0) => None,
        Some(count) => Some(format!(
            "{count} revision(s) have been pending verification for over an hour"
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

    const CHECKSUM: &str =
        "sha256:9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";

    #[test]
    fn parse_verdict_accepts_the_two_verdicts() {
        let body = format!(
            r#"{{"verdict":"verified","checksum":"{CHECKSUM}","published_at":"2026-07-10T00:00:00.000Z"}}"#
        );
        assert_eq!(
            parse_verdict(body.as_bytes()),
            Ok(ParsedVerdict {
                verdict: Verdict::Verified,
                reason: None,
                checksum: CHECKSUM.to_owned(),
                published_at: "2026-07-10T00:00:00.000Z".to_owned(),
            })
        );
        // A reason on a verified verdict is accepted and ignored.
        let body = format!(
            r#"{{"verdict":"verified","reason":"fine","checksum":"{CHECKSUM}","published_at":"t"}}"#
        );
        assert_eq!(
            parse_verdict(body.as_bytes()),
            Ok(ParsedVerdict {
                verdict: Verdict::Verified,
                reason: None,
                checksum: CHECKSUM.to_owned(),
                published_at: "t".to_owned(),
            })
        );
        // A rejection carries both bindings too.
        let body = format!(
            r#"{{"verdict":"rejected","reason":"malware","checksum":"{CHECKSUM}","published_at":"t"}}"#
        );
        assert_eq!(
            parse_verdict(body.as_bytes()),
            Ok(ParsedVerdict {
                verdict: Verdict::Rejected,
                reason: Some("malware".to_owned()),
                checksum: CHECKSUM.to_owned(),
                published_at: "t".to_owned(),
            })
        );
    }

    #[test]
    fn parse_verdict_requires_the_canonical_checksum() {
        for body in [
            br#"{"verdict":"verified","checksum":"aa12","published_at":"t"}"#.as_slice(),
            br#"{"verdict":"rejected","reason":"malware","checksum":"aa12","published_at":"t"}"#,
        ] {
            assert_eq!(
                parse_verdict(body),
                Err(INVALID_VERDICT_CHECKSUM),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn parse_verdict_requires_the_listing_binding_for_both_verdicts() {
        for body in [
            br#"{"verdict":"verified"}"#.as_slice(),
            br#"{"verdict":"verified","checksum":"aa12"}"#,
            br#"{"verdict":"verified","published_at":"t"}"#,
            br#"{"verdict":"verified","checksum":"","published_at":"t"}"#,
            br#"{"verdict":"verified","checksum":"aa12","published_at":""}"#,
            br#"{"verdict":"rejected","reason":"malware"}"#,
            br#"{"verdict":"rejected","reason":"malware","checksum":"aa12"}"#,
            br#"{"verdict":"rejected","reason":"malware","published_at":"t"}"#,
            br#"{"verdict":"rejected","reason":"malware","checksum":"aa12","published_at":""}"#,
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
        // Repeating the applied verdict on a verified revision is the
        // idempotent 200.
        assert_eq!(
            transition(Status::Verified, Verdict::Verified),
            Transition::NoOp
        );
        // Rejecting a verified revision hits the immutability wall.
        assert_eq!(
            transition(Status::Verified, Verdict::Rejected),
            Transition::Conflict(error::VERSION_IMMUTABLE)
        );
        // Verifying a rejected revision conflicts: republish is the
        // recovery path.
        assert_eq!(
            transition(Status::Rejected, Verdict::Verified),
            Transition::Conflict(error::VERSION_REJECTED_REVERDICT)
        );
        // Repeating the applied rejection is the idempotent 200 too
        // (see `Transition::Conflict` for why no revival reaches it).
        assert_eq!(
            transition(Status::Rejected, Verdict::Rejected),
            Transition::NoOp
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
    fn packages_json_matches_the_contract_byte_for_byte() {
        assert_eq!(packages_json(&[]), r#"{"packages":[]}"#);
        let corpus = [
            CorpusPackage {
                scope: "fmtlib".to_owned(),
                name: "fmt".to_owned(),
                vetted: true,
            },
            CorpusPackage {
                scope: "gabime".to_owned(),
                name: "spdlog".to_owned(),
                vetted: false,
            },
        ];
        assert_eq!(
            packages_json(&corpus),
            r#"{"packages":[{"scope":"fmtlib","name":"fmt","vetted":true},{"scope":"gabime","name":"spdlog","vetted":false}]}"#
        );
    }

    #[test]
    fn stale_pending_alerts_on_counts_and_on_unreadable_data() {
        assert_eq!(stale_pending_alert(Some(0)), None);
        assert_eq!(
            stale_pending_alert(Some(3)),
            Some("3 revision(s) have been pending verification for over an hour".to_owned())
        );
        assert!(stale_pending_alert(None).is_some());
    }
}
