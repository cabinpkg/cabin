//! JSON composition and body validation for the session-plane user API
//! (`/api/v1/user/*`): pure and host-testable, mirroring
//! [`crate::documents`] for the read plane. Response shapes are
//! deterministic; token hashes never appear in any of them.

use serde::Deserialize;

use crate::quota::PlanQuotas;

/// `GET /api/v1/user`: who the session belongs to.
pub fn user_json(github_id: i64, login: &str, plan: &str) -> String {
    serde_json::json!({
        "github_id": github_id,
        "login": login,
        "plan": plan,
    })
    .to_string()
}

/// The usage block: the user's plan, current consumption, and the plan's
/// quota values. `stored_bytes` excludes rejected versions (their bytes
/// are refunded); the per-status counts cover everything the user ever
/// published.
pub struct UsageInfo {
    pub plan: String,
    pub package_count: u64,
    pub stored_bytes: u64,
    pub published_today: u64,
    pub verified_count: u64,
    pub pending_count: u64,
    pub rejected_count: u64,
    pub quotas: PlanQuotas,
}

/// `GET /api/v1/user/usage`.
pub fn usage_json(usage: &UsageInfo) -> String {
    serde_json::json!({
        "plan": usage.plan,
        "package_count": usage.package_count,
        "stored_bytes": usage.stored_bytes,
        "published_today": usage.published_today,
        "versions": {
            "verified": usage.verified_count,
            "pending": usage.pending_count,
            "rejected": usage.rejected_count,
        },
        "quotas": {
            "max_archive_bytes": usage.quotas.max_archive_bytes,
            "max_total_bytes_per_user": usage.quotas.max_total_bytes_per_user,
            "max_new_packages_per_day": usage.quotas.max_new_packages_per_day,
            "max_packages_total": usage.quotas.max_packages_total,
            "max_versions_per_package_per_day": usage.quotas.max_versions_per_package_per_day,
            "publish_burst": usage.quotas.publish_burst,
            "publish_refill_per_minute": usage.quotas.publish_refill_per_minute,
        },
    })
    .to_string()
}

/// One version row of one of the user's packages, as
/// `GET /api/v1/user/packages` serves it.
pub struct PackageVersionRow {
    /// The package's canonical `<scope>/<name>` name.
    pub name: String,
    pub version: String,
    /// The verification lifecycle state: `pending`, `verified`, or
    /// `rejected`.
    pub verification: String,
    pub yanked: bool,
    pub published_at: String,
    /// The approximate served-download count (`docs/architecture.md`,
    /// "Download counts"); always 0 for pending and rejected versions.
    pub downloads: u64,
}

/// `GET /api/v1/user/packages`: the user's packages with every version's
/// verification and yanked state. `rows` arrive already ordered (name,
/// then newest first) and are grouped by name preserving that order, so
/// the output is deterministic.
pub fn packages_json(rows: &[PackageVersionRow]) -> String {
    let mut groups: Vec<(&str, Vec<serde_json::Value>)> = Vec::new();
    for row in rows {
        let version = serde_json::json!({
            "version": row.version,
            "verification": row.verification,
            "yanked": row.yanked,
            "published_at": row.published_at,
            "downloads": row.downloads,
        });
        match groups.last_mut() {
            Some((name, versions)) if *name == row.name => versions.push(version),
            _ => groups.push((&row.name, vec![version])),
        }
    }
    let packages: Vec<serde_json::Value> = groups
        .into_iter()
        .map(|(name, versions)| serde_json::json!({ "name": name, "versions": versions }))
        .collect();
    serde_json::json!({ "packages": packages }).to_string()
}

/// One member of a scope, as `GET /api/v1/user/scopes/<scope>/members`
/// serves it: the GitHub numeric id (the identity the management API
/// speaks), the display login snapshot, and the member role.
pub struct MemberRow {
    pub github_id: i64,
    pub login: String,
    pub role: String,
}

/// `GET /api/v1/user/scopes/<scope>/members`. `rows` arrive already
/// deterministically ordered.
pub fn members_json(rows: &[MemberRow]) -> String {
    let members: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "github_id": row.github_id,
                "login": row.login,
                "role": row.role,
            })
        })
        .collect();
    serde_json::json!({ "members": members }).to_string()
}

/// `POST /api/v1/user/scopes/<scope>/members` success body: the resulting
/// membership (an already-present member keeps their role) and whether
/// this request created it.
pub fn member_added_json(github_id: i64, role: &str, changed: bool) -> String {
    serde_json::json!({
        "ok": true,
        "github_id": github_id,
        "role": role,
        "changed": changed,
    })
    .to_string()
}

/// `POST /api/v1/user/scopes/<scope>/members/<github_id>/remove` success
/// body; removing a non-member is the idempotent `"changed": false`.
pub fn member_removed_json(github_id: i64, changed: bool) -> String {
    serde_json::json!({
        "ok": true,
        "github_id": github_id,
        "changed": changed,
    })
    .to_string()
}

pub const INVALID_ADD_MEMBER_BODY: &str =
    r#"the body must be {"github_id": <number>, "role": "owner" | "member"}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddMemberBody {
    github_id: i64,
    role: String,
}

/// A validated add-member request.
#[derive(Debug, PartialEq, Eq)]
pub struct AddMember {
    pub github_id: i64,
    pub role: String,
}

/// Parses and validates an add-member body: a positive GitHub numeric id
/// and one of the two member roles (`owner` is the admin role;
/// `docs/architecture.md`, "Scopes").
///
/// # Errors
///
/// [`INVALID_ADD_MEMBER_BODY`], a fixed string that never echoes request
/// bytes.
pub fn parse_add_member(body: &[u8]) -> Result<AddMember, &'static str> {
    let Ok(parsed) = serde_json::from_slice::<AddMemberBody>(body) else {
        return Err(INVALID_ADD_MEMBER_BODY);
    };
    if parsed.github_id <= 0 || !matches!(parsed.role.as_str(), "owner" | "member") {
        return Err(INVALID_ADD_MEMBER_BODY);
    }
    Ok(AddMember {
        github_id: parsed.github_id,
        role: parsed.role,
    })
}

/// One token row, as the listing serves it: metadata only, never the
/// token or its hash (the plaintext exists once, on the create response).
pub struct TokenRow {
    pub id: String,
    pub name: String,
    /// The stored comma-separated scopes column.
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// `GET /api/v1/user/tokens`.
pub fn tokens_json(rows: &[TokenRow]) -> String {
    let tokens: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "name": row.name,
                "scopes": split_scopes(&row.scopes),
                "created_at": row.created_at,
                "last_used_at": row.last_used_at,
                "revoked": row.revoked,
            })
        })
        .collect();
    serde_json::json!({ "tokens": tokens }).to_string()
}

/// `POST /api/v1/user/tokens` success body: the one response that ever
/// carries the plaintext token.
pub fn token_created_json(id: &str, name: &str, scopes: &str, token: &str) -> String {
    serde_json::json!({
        "id": id,
        "name": name,
        "scopes": split_scopes(scopes),
        "token": token,
    })
    .to_string()
}

fn split_scopes(scopes: &str) -> Vec<&str> {
    scopes
        .split(',')
        .filter(|scope| !scope.is_empty())
        .collect()
}

pub const INVALID_CREATE_TOKEN_BODY: &str = r#"the body must be {"name": <1-64 chars>, "scopes": [..]} with scopes drawn from "publish", "yank", "verify""#;

/// The known scopes, in the canonical stored order.
const KNOWN_SCOPES: [&str; 3] = ["publish", "yank", "verify"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTokenBody {
    name: String,
    scopes: Vec<String>,
}

/// A validated create-token request: the trimmed name and the scopes
/// joined into the canonical comma-separated column value.
#[derive(Debug, PartialEq, Eq)]
pub struct CreateToken {
    pub name: String,
    pub scopes: String,
}

/// Parses and validates a create-token body: the name is 1 to 64
/// characters after trimming, and every scope must be a known one
/// (unknown or repeated scopes are refused rather than silently
/// dropped - a caller must never believe it minted a scope it did not).
///
/// # Errors
///
/// [`INVALID_CREATE_TOKEN_BODY`], a fixed string that never echoes
/// request bytes.
pub fn parse_create_token(body: &[u8]) -> Result<CreateToken, &'static str> {
    let Ok(parsed) = serde_json::from_slice::<CreateTokenBody>(body) else {
        return Err(INVALID_CREATE_TOKEN_BODY);
    };
    let name = parsed.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(INVALID_CREATE_TOKEN_BODY);
    }
    let mut known = KNOWN_SCOPES.map(|scope| (scope, false));
    for scope in &parsed.scopes {
        let Some(entry) = known.iter_mut().find(|(name, _)| name == scope) else {
            return Err(INVALID_CREATE_TOKEN_BODY);
        };
        if entry.1 {
            return Err(INVALID_CREATE_TOKEN_BODY);
        }
        entry.1 = true;
    }
    let scopes: Vec<&str> = known
        .iter()
        .filter_map(|(name, granted)| granted.then_some(*name))
        .collect();
    Ok(CreateToken {
        name,
        scopes: scopes.join(","),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::quotas_for_plan;

    #[test]
    fn user_json_is_the_documented_shape() {
        assert_eq!(
            user_json(26_405_363, "ken-matsui", "free"),
            r#"{"github_id":26405363,"login":"ken-matsui","plan":"free"}"#
        );
    }

    #[test]
    fn usage_json_carries_counts_and_quotas() {
        let usage = UsageInfo {
            plan: "free".to_owned(),
            package_count: 2,
            stored_bytes: 1_048_576,
            published_today: 3,
            verified_count: 4,
            pending_count: 1,
            rejected_count: 0,
            quotas: quotas_for_plan("free"),
        };
        assert_eq!(
            usage_json(&usage),
            r#"{"plan":"free","package_count":2,"stored_bytes":1048576,"published_today":3,"versions":{"verified":4,"pending":1,"rejected":0},"quotas":{"max_archive_bytes":16777216,"max_total_bytes_per_user":268435456,"max_new_packages_per_day":5,"max_packages_total":50,"max_versions_per_package_per_day":30,"publish_burst":5.0,"publish_refill_per_minute":1.0}}"#
        );
    }

    #[test]
    fn packages_json_groups_adjacent_rows_by_name() {
        let row = |name: &str, version: &str, verification: &str, yanked: bool, downloads: u64| {
            PackageVersionRow {
                name: name.to_owned(),
                version: version.to_owned(),
                verification: verification.to_owned(),
                yanked,
                published_at: "2026-07-10T00:00:00.000Z".to_owned(),
                downloads,
            }
        };
        // `mine/fmt` and `fmtlib/fmt` share a package part; the full
        // canonical name is the grouping key, so they stay two packages.
        let rows = [
            row("fmtlib/fmt", "10.2.1", "verified", false, 42),
            row("fmtlib/fmt", "10.2.0", "rejected", true, 0),
            row("madler/zlib", "1.3.1", "pending", false, 0),
            row("mine/fmt", "0.1.0", "pending", false, 0),
        ];
        assert_eq!(
            packages_json(&rows),
            r#"{"packages":[{"name":"fmtlib/fmt","versions":[{"version":"10.2.1","verification":"verified","yanked":false,"published_at":"2026-07-10T00:00:00.000Z","downloads":42},{"version":"10.2.0","verification":"rejected","yanked":true,"published_at":"2026-07-10T00:00:00.000Z","downloads":0}]},{"name":"madler/zlib","versions":[{"version":"1.3.1","verification":"pending","yanked":false,"published_at":"2026-07-10T00:00:00.000Z","downloads":0}]},{"name":"mine/fmt","versions":[{"version":"0.1.0","verification":"pending","yanked":false,"published_at":"2026-07-10T00:00:00.000Z","downloads":0}]}]}"#
        );
    }

    #[test]
    fn packages_json_renders_no_packages_as_an_empty_list() {
        assert_eq!(packages_json(&[]), r#"{"packages":[]}"#);
    }

    #[test]
    fn members_json_is_the_documented_shape() {
        let rows = [
            MemberRow {
                github_id: 26_405_363,
                login: "ken-matsui".to_owned(),
                role: "owner".to_owned(),
            },
            MemberRow {
                github_id: 583_231,
                login: "octocat".to_owned(),
                role: "member".to_owned(),
            },
        ];
        assert_eq!(
            members_json(&rows),
            r#"{"members":[{"github_id":26405363,"login":"ken-matsui","role":"owner"},{"github_id":583231,"login":"octocat","role":"member"}]}"#
        );
        assert_eq!(members_json(&[]), r#"{"members":[]}"#);
    }

    #[test]
    fn member_mutation_bodies_are_the_documented_shapes() {
        assert_eq!(
            member_added_json(583_231, "member", true),
            r#"{"ok":true,"github_id":583231,"role":"member","changed":true}"#
        );
        assert_eq!(
            member_removed_json(583_231, false),
            r#"{"ok":true,"github_id":583231,"changed":false}"#
        );
    }

    #[test]
    fn add_member_bodies_parse_and_validate_strictly() {
        assert_eq!(
            parse_add_member(br#"{"github_id":583231,"role":"member"}"#),
            Ok(AddMember {
                github_id: 583_231,
                role: "member".to_owned()
            })
        );
        assert_eq!(
            parse_add_member(br#"{"role":"owner","github_id":1}"#),
            Ok(AddMember {
                github_id: 1,
                role: "owner".to_owned()
            })
        );
        for body in [
            &b"not json"[..],
            b"{}",
            br#"{"github_id":583231}"#,
            br#"{"role":"member"}"#,
            br#"{"github_id":0,"role":"member"}"#,
            br#"{"github_id":-1,"role":"member"}"#,
            br#"{"github_id":1.5,"role":"member"}"#,
            br#"{"github_id":"583231","role":"member"}"#,
            br#"{"github_id":583231,"role":"admin"}"#,
            br#"{"github_id":583231,"role":"OWNER"}"#,
            br#"{"github_id":583231,"role":"member","extra":1}"#,
        ] {
            assert_eq!(
                parse_add_member(body),
                Err(INVALID_ADD_MEMBER_BODY),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn tokens_json_lists_metadata_and_never_a_secret() {
        let rows = [
            TokenRow {
                id: "abc".to_owned(),
                name: "ci".to_owned(),
                scopes: "publish,yank".to_owned(),
                created_at: "2026-07-10T00:00:00.000Z".to_owned(),
                last_used_at: None,
                revoked: false,
            },
            TokenRow {
                id: "def".to_owned(),
                name: "old".to_owned(),
                scopes: String::new(),
                created_at: "2026-01-01T00:00:00.000Z".to_owned(),
                last_used_at: Some("2026-06-01T00:00:00.000Z".to_owned()),
                revoked: true,
            },
        ];
        assert_eq!(
            tokens_json(&rows),
            r#"{"tokens":[{"id":"abc","name":"ci","scopes":["publish","yank"],"created_at":"2026-07-10T00:00:00.000Z","last_used_at":null,"revoked":false},{"id":"def","name":"old","scopes":[],"created_at":"2026-01-01T00:00:00.000Z","last_used_at":"2026-06-01T00:00:00.000Z","revoked":true}]}"#
        );
    }

    #[test]
    fn token_created_json_carries_the_plaintext_once() {
        assert_eq!(
            token_created_json("abc", "ci", "publish", "cabin_secret"),
            r#"{"id":"abc","name":"ci","scopes":["publish"],"token":"cabin_secret"}"#
        );
    }

    #[test]
    fn create_token_bodies_parse_and_normalize() {
        let parsed = parse_create_token(br#"{"name":" ci ","scopes":["yank","publish"]}"#).unwrap();
        assert_eq!(parsed.name, "ci");
        // Canonical stored order regardless of request order.
        assert_eq!(parsed.scopes, "publish,yank");

        let parsed = parse_create_token(br#"{"name":"read-only","scopes":[]}"#).unwrap();
        assert_eq!(parsed.scopes, "");
    }

    #[test]
    fn create_token_bodies_are_validated_strictly() {
        for body in [
            &b"not json"[..],
            b"{}",
            br#"{"name":"ci"}"#,
            br#"{"scopes":[]}"#,
            br#"{"name":"","scopes":[]}"#,
            br#"{"name":"   ","scopes":[]}"#,
            br#"{"name":"ci","scopes":["admin"]}"#,
            br#"{"name":"ci","scopes":["publish","publish"]}"#,
            br#"{"name":"ci","scopes":["PUBLISH"]}"#,
            br#"{"name":"ci","scopes":[],"extra":1}"#,
        ] {
            assert_eq!(
                parse_create_token(body),
                Err(INVALID_CREATE_TOKEN_BODY),
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
        // 64 chars is allowed, 65 is not (counted in chars, not bytes).
        let name64 = "x".repeat(64);
        let ok = format!(r#"{{"name":"{name64}","scopes":[]}}"#);
        assert!(parse_create_token(ok.as_bytes()).is_ok());
        let name65 = "ü".repeat(65);
        let too_long = format!(r#"{{"name":"{name65}","scopes":[]}}"#);
        assert!(parse_create_token(too_long.as_bytes()).is_err());
    }
}
