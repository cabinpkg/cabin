//! JSON composition and body validation for the session-plane user API
//! (`/api/v1/user/*`): pure and host-testable, mirroring
//! [`crate::documents`] for the read plane. Response shapes are
//! deterministic; token hashes never appear in any of them.

use serde::Deserialize;

use crate::quota::ClassQuotas;

/// `GET /api/v1/user`: who the session belongs to.
pub fn user_json(github_id: i64, login: &str, quota_class: &str) -> String {
    serde_json::json!({
        "github_id": github_id,
        "login": login,
        "quota_class": quota_class,
    })
    .to_string()
}

/// The usage block: the user's quota class, current consumption, and the
/// class's quota values. `stored_bytes` excludes rejected versions
/// (their bytes are refunded); the per-status counts cover everything
/// the user ever published.
pub struct UsageInfo {
    pub quota_class: String,
    pub package_count: u64,
    pub stored_bytes: u64,
    pub published_today: u64,
    pub verified_count: u64,
    pub pending_count: u64,
    pub rejected_count: u64,
    pub quotas: ClassQuotas,
}

/// `GET /api/v1/user/usage`.
pub fn usage_json(usage: &UsageInfo) -> String {
    serde_json::json!({
        "quota_class": usage.quota_class,
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

pub const INVALID_SEARCH_QUERY: &str = "the q query parameter must be 1 to 64 characters";

/// The search result cap: a hard limit, not a page size (there is no
/// pagination; a query matching more packages simply truncates).
pub const SEARCH_LIMIT: usize = 20;

/// Validates a search term: 1 to 64 characters after trimming
/// (counted in chars, like token names). The term is ASCII-lowercased:
/// the search statement's `instr` compares bytes exactly
/// ([`crate::sql::SEARCH_VERIFIED_VERSIONS`] explains why it is not a
/// `LIKE`), and names are lowercase by grammar, so the fold is what
/// makes search ASCII-case-insensitive - and what keeps the host-side
/// ranking consistent with what the SQL matched.
///
/// # Errors
///
/// [`INVALID_SEARCH_QUERY`], a fixed string that never echoes request
/// bytes.
pub fn parse_search_query(q: Option<&str>) -> Result<String, &'static str> {
    let trimmed = q.unwrap_or_default().trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return Err(INVALID_SEARCH_QUERY);
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// One verified version row feeding the search response, as
/// [`crate::sql::SEARCH_VERIFIED_VERSIONS`] returns them (any order).
pub struct SearchVersionRow {
    pub scope: String,
    pub name: String,
    pub version: String,
    pub yanked: bool,
    pub published_at: String,
    pub downloads: u64,
}

/// `GET /api/v1/user/search?q=<term>`: groups the verified rows by
/// package and ranks: exact canonical-name match, then prefix, then
/// substring; ties by total downloads descending, then name
/// ascending; truncated to [`SEARCH_LIMIT`]. Each hit carries the
/// newest verified version (latest `published_at`, version string
/// breaking ties) with its yanked flag, and the package's total
/// downloads over verified versions (yanked included - they stay
/// downloadable).
pub fn search_json(rows: &[SearchVersionRow], query: &str) -> String {
    struct Hit<'a> {
        rank: u8,
        full_name: String,
        scope: &'a str,
        name: &'a str,
        newest: &'a SearchVersionRow,
        downloads: u64,
    }
    let mut hits: Vec<Hit<'_>> = Vec::new();
    for row in rows {
        if let Some(hit) = hits
            .iter_mut()
            .find(|hit| hit.scope == row.scope && hit.name == row.name)
        {
            hit.downloads += row.downloads;
            if (row.published_at.as_str(), row.version.as_str())
                > (
                    hit.newest.published_at.as_str(),
                    hit.newest.version.as_str(),
                )
            {
                hit.newest = row;
            }
        } else {
            let full_name = format!("{}/{}", row.scope, row.name);
            let rank = if full_name == query {
                0
            } else if full_name.starts_with(query) {
                1
            } else {
                2
            };
            hits.push(Hit {
                rank,
                full_name,
                scope: &row.scope,
                name: &row.name,
                newest: row,
                downloads: row.downloads,
            });
        }
    }
    hits.sort_by(|a, b| {
        (a.rank, std::cmp::Reverse(a.downloads), &a.full_name).cmp(&(
            b.rank,
            std::cmp::Reverse(b.downloads),
            &b.full_name,
        ))
    });
    hits.truncate(SEARCH_LIMIT);
    let results: Vec<serde_json::Value> = hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "scope": hit.scope,
                "name": hit.name,
                "version": hit.newest.version,
                "yanked": hit.newest.yanked,
                "downloads": hit.downloads,
            })
        })
        .collect();
    serde_json::json!({ "results": results }).to_string()
}

/// One verified version row of a dependent package, as
/// [`crate::sql::REVERSE_DEPENDENCIES`] returns them (any order).
pub struct DependentVersionRow {
    pub scope: String,
    pub name: String,
    pub version: String,
    pub published_at: String,
}

/// `GET /api/v1/user/package/<scope>/<name>/reverse-dependencies`:
/// the distinct packages with at least one verified version whose
/// `dependencies` map contains the target, each with the count of
/// such versions and the newest matching version string (latest
/// `published_at`, version string breaking ties). Ordered by scope,
/// then name.
pub fn reverse_dependencies_json(rows: &[DependentVersionRow]) -> String {
    struct Dependent<'a> {
        scope: &'a str,
        name: &'a str,
        matching: u64,
        newest: &'a DependentVersionRow,
    }
    let mut dependents: Vec<Dependent<'_>> = Vec::new();
    for row in rows {
        match dependents
            .iter_mut()
            .find(|dependent| dependent.scope == row.scope && dependent.name == row.name)
        {
            Some(dependent) => {
                dependent.matching += 1;
                if (row.published_at.as_str(), row.version.as_str())
                    > (
                        dependent.newest.published_at.as_str(),
                        dependent.newest.version.as_str(),
                    )
                {
                    dependent.newest = row;
                }
            }
            None => dependents.push(Dependent {
                scope: &row.scope,
                name: &row.name,
                matching: 1,
                newest: row,
            }),
        }
    }
    dependents.sort_by_key(|dependent| (dependent.scope, dependent.name));
    let dependents: Vec<serde_json::Value> = dependents
        .iter()
        .map(|dependent| {
            serde_json::json!({
                "scope": dependent.scope,
                "name": dependent.name,
                "matching_versions": dependent.matching,
                "newest_matching_version": dependent.newest.version,
            })
        })
        .collect();
    serde_json::json!({ "dependents": dependents }).to_string()
}

/// One verified version row of the package a detail request targets,
/// as [`crate::sql::VERIFIED_VERSION_DETAILS`] returns them (any
/// order).
pub struct PackageDetailRow {
    pub version: String,
    pub metadata_json: String,
    pub yanked: bool,
    pub published_at: String,
    pub downloads: u64,
}

/// `GET /api/v1/user/package/<scope>/<name>`: the package's verified
/// versions, newest first (latest `published_at`, version string
/// breaking ties), plus the newest version's runtime `dependencies`
/// as a `name -> requirement` map with sorted keys (dev- and
/// system-dependencies are deliberately absent, matching the
/// reverse-dependencies contract). A rich stored entry contributes
/// its `version` field; verified metadata is canonical by the
/// verifier's equality check, so other shapes only arise from an
/// invariant break and render as an empty requirement rather than
/// failing the whole response.
///
/// # Errors
///
/// When `rows` is empty (the caller answers 404 before composing) or
/// a stored entry is not a JSON object - an internal invariant break
/// the caller reports as a 500, never a client error.
pub fn package_detail_json(
    scope: &str,
    name: &str,
    rows: &[PackageDetailRow],
) -> Result<String, String> {
    let mut rows: Vec<&PackageDetailRow> = rows.iter().collect();
    rows.sort_by(|a, b| {
        (b.published_at.as_str(), b.version.as_str())
            .cmp(&(a.published_at.as_str(), a.version.as_str()))
    });
    let Some(newest) = rows.first() else {
        return Err(format!("no verified rows to compose for {scope}/{name}"));
    };
    let metadata: serde_json::Value =
        serde_json::from_str(&newest.metadata_json).map_err(|err| {
            format!(
                "stored metadata for {scope}/{name}@{} is not valid JSON: {err}",
                newest.version
            )
        })?;
    if !metadata.is_object() {
        return Err(format!(
            "stored metadata for {scope}/{name}@{} is not a JSON object",
            newest.version
        ));
    }
    let mut dependencies: Vec<(&String, &serde_json::Value)> = metadata
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
        .map(|map| map.iter().collect())
        .unwrap_or_default();
    dependencies.sort_by_key(|(dependency, _)| *dependency);
    let mut dependency_map = serde_json::Map::new();
    for (dependency, entry) in dependencies {
        let requirement = entry
            .as_str()
            .or_else(|| entry.get("version").and_then(serde_json::Value::as_str))
            .unwrap_or_default();
        dependency_map.insert(dependency.clone(), requirement.into());
    }
    let versions: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "version": row.version,
                "yanked": row.yanked,
                "published_at": row.published_at,
                "downloads": row.downloads,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "scope": scope,
        "name": name,
        "versions": versions,
        "newest_version": newest.version,
        "dependencies": dependency_map,
    })
    .to_string())
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
    use crate::quota::quotas_for_class;

    #[test]
    fn user_json_is_the_documented_shape() {
        assert_eq!(
            user_json(26_405_363, "ken-matsui", "default"),
            r#"{"github_id":26405363,"login":"ken-matsui","quota_class":"default"}"#
        );
    }

    #[test]
    fn usage_json_carries_counts_and_quotas() {
        let usage = UsageInfo {
            quota_class: "default".to_owned(),
            package_count: 2,
            stored_bytes: 1_048_576,
            published_today: 3,
            verified_count: 4,
            pending_count: 1,
            rejected_count: 0,
            quotas: quotas_for_class("default"),
        };
        assert_eq!(
            usage_json(&usage),
            r#"{"quota_class":"default","package_count":2,"stored_bytes":1048576,"published_today":3,"versions":{"verified":4,"pending":1,"rejected":0},"quotas":{"max_archive_bytes":16777216,"max_total_bytes_per_user":134217728,"max_new_packages_per_day":5,"max_packages_total":50,"max_versions_per_package_per_day":30,"publish_burst":5.0,"publish_refill_per_minute":1.0}}"#
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
    fn search_queries_are_trimmed_lowercased_and_length_checked() {
        assert_eq!(parse_search_query(Some("  Fmt ")), Ok("fmt".to_owned()));
        assert_eq!(
            parse_search_query(Some(&"x".repeat(64))),
            Ok("x".repeat(64))
        );
        for q in [None, Some(""), Some("   "), Some("x".repeat(65).as_str())] {
            assert_eq!(parse_search_query(q), Err(INVALID_SEARCH_QUERY), "q: {q:?}");
        }
        // Length is counted in chars, not bytes.
        assert!(parse_search_query(Some(&"ü".repeat(64))).is_ok());
        assert!(parse_search_query(Some(&"ü".repeat(65))).is_err());
    }

    fn search_row(
        scope: &str,
        name: &str,
        version: &str,
        yanked: bool,
        published_at: &str,
        downloads: u64,
    ) -> SearchVersionRow {
        SearchVersionRow {
            scope: scope.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            yanked,
            published_at: published_at.to_owned(),
            downloads,
        }
    }

    #[test]
    fn search_json_ranks_exact_then_prefix_then_substring() {
        // All three match the substring "fmt"; prefix matches beat
        // bare substrings regardless of download counts, which only
        // break ties within a rank.
        let rows = [
            search_row(
                "lib",
                "afmt",
                "1.0.0",
                false,
                "2026-07-01T00:00:00.000Z",
                999,
            ),
            search_row(
                "fmt",
                "extra",
                "1.0.0",
                false,
                "2026-07-01T00:00:00.000Z",
                500,
            ),
            search_row(
                "fmtlib",
                "zzz",
                "9.0.0",
                false,
                "2026-07-01T00:00:00.000Z",
                700,
            ),
        ];
        assert_eq!(
            search_json(&rows, "fmt"),
            r#"{"results":[{"scope":"fmtlib","name":"zzz","version":"9.0.0","yanked":false,"downloads":700},{"scope":"fmt","name":"extra","version":"1.0.0","yanked":false,"downloads":500},{"scope":"lib","name":"afmt","version":"1.0.0","yanked":false,"downloads":999}]}"#
        );
    }

    #[test]
    fn search_json_exact_match_beats_everything() {
        let rows = [
            search_row(
                "fmtlib",
                "fmt",
                "10.2.1",
                false,
                "2026-07-01T00:00:00.000Z",
                1,
            ),
            search_row(
                "fmtlib",
                "fmt-extras",
                "1.0.0",
                false,
                "2026-07-01T00:00:00.000Z",
                999,
            ),
        ];
        let body = search_json(&rows, "fmtlib/fmt");
        assert_eq!(
            body,
            r#"{"results":[{"scope":"fmtlib","name":"fmt","version":"10.2.1","yanked":false,"downloads":1},{"scope":"fmtlib","name":"fmt-extras","version":"1.0.0","yanked":false,"downloads":999}]}"#
        );
    }

    #[test]
    fn search_json_sums_downloads_and_reports_the_newest_version() {
        // Two versions of one package: downloads sum, the hit carries
        // the newest version (later published_at) and its yanked flag.
        let rows = [
            search_row(
                "fmtlib",
                "fmt",
                "10.2.0",
                false,
                "2026-07-01T00:00:00.000Z",
                30,
            ),
            search_row(
                "fmtlib",
                "fmt",
                "10.2.1",
                true,
                "2026-07-02T00:00:00.000Z",
                12,
            ),
        ];
        assert_eq!(
            search_json(&rows, "fmt"),
            r#"{"results":[{"scope":"fmtlib","name":"fmt","version":"10.2.1","yanked":true,"downloads":42}]}"#
        );
        // Equal published_at: the version string breaks the tie.
        let rows = [
            search_row(
                "fmtlib",
                "fmt",
                "10.2.0",
                false,
                "2026-07-01T00:00:00.000Z",
                0,
            ),
            search_row(
                "fmtlib",
                "fmt",
                "10.2.1",
                false,
                "2026-07-01T00:00:00.000Z",
                0,
            ),
        ];
        assert!(search_json(&rows, "fmt").contains(r#""version":"10.2.1""#));
    }

    #[test]
    fn search_json_breaks_rank_ties_by_downloads_then_name() {
        let rows = [
            search_row("b", "pkg", "1.0.0", false, "2026-07-01T00:00:00.000Z", 5),
            search_row("a", "pkg", "1.0.0", false, "2026-07-01T00:00:00.000Z", 5),
            search_row("c", "pkg", "1.0.0", false, "2026-07-01T00:00:00.000Z", 9),
        ];
        let body = search_json(&rows, "pkg");
        let order: Vec<usize> = ["c", "a", "b"]
            .iter()
            .map(|scope| body.find(&format!(r#""scope":"{scope}""#)).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "body: {body}");
    }

    #[test]
    fn search_json_truncates_to_the_limit() {
        let rows: Vec<SearchVersionRow> = (0..30)
            .map(|i| {
                search_row(
                    "scope",
                    &format!("pkg-{i:02}"),
                    "1.0.0",
                    false,
                    "2026-07-01T00:00:00.000Z",
                    0,
                )
            })
            .collect();
        let body = search_json(&rows, "pkg");
        assert_eq!(body.matches(r#""scope":"scope""#).count(), SEARCH_LIMIT);
        assert!(body.contains("pkg-19") && !body.contains("pkg-20"));
    }

    #[test]
    fn search_json_renders_no_hits_as_an_empty_list() {
        assert_eq!(search_json(&[], "ghost"), r#"{"results":[]}"#);
    }

    fn dependent_row(
        scope: &str,
        name: &str,
        version: &str,
        published_at: &str,
    ) -> DependentVersionRow {
        DependentVersionRow {
            scope: scope.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            published_at: published_at.to_owned(),
        }
    }

    #[test]
    fn reverse_dependencies_json_groups_counts_and_orders() {
        let rows = [
            dependent_row("gabime", "spdlog", "1.13.0", "2026-07-02T00:00:00.000Z"),
            dependent_row("acme", "logger", "0.3.0", "2026-07-01T00:00:00.000Z"),
            dependent_row("gabime", "spdlog", "1.14.0", "2026-07-03T00:00:00.000Z"),
        ];
        assert_eq!(
            reverse_dependencies_json(&rows),
            r#"{"dependents":[{"scope":"acme","name":"logger","matching_versions":1,"newest_matching_version":"0.3.0"},{"scope":"gabime","name":"spdlog","matching_versions":2,"newest_matching_version":"1.14.0"}]}"#
        );
        assert_eq!(reverse_dependencies_json(&[]), r#"{"dependents":[]}"#);
    }

    fn detail_row(
        version: &str,
        metadata_json: &str,
        yanked: bool,
        published_at: &str,
        downloads: u64,
    ) -> PackageDetailRow {
        PackageDetailRow {
            version: version.to_owned(),
            metadata_json: metadata_json.to_owned(),
            yanked,
            published_at: published_at.to_owned(),
            downloads,
        }
    }

    #[test]
    fn package_detail_json_is_the_documented_shape() {
        // Versions arrive in arbitrary order; the payload runs newest
        // first and the dependency map comes from the newest version
        // only, keys sorted, rich entries contributing their version
        // requirement.
        let rows = [
            detail_row(
                "10.2.0",
                r#"{"dependencies":{"old/dep":"^1"}}"#,
                true,
                "2026-07-01T00:00:00.000Z",
                30,
            ),
            detail_row(
                "10.2.1",
                r#"{"dependencies":{"madler/zlib":{"version":"^1.3","optional":true},"fmtlib/fmt":"^10"}}"#,
                false,
                "2026-07-02T00:00:00.000Z",
                12,
            ),
        ];
        assert_eq!(
            package_detail_json("gabime", "spdlog", &rows).unwrap(),
            r#"{"scope":"gabime","name":"spdlog","versions":[{"version":"10.2.1","yanked":false,"published_at":"2026-07-02T00:00:00.000Z","downloads":12},{"version":"10.2.0","yanked":true,"published_at":"2026-07-01T00:00:00.000Z","downloads":30}],"newest_version":"10.2.1","dependencies":{"fmtlib/fmt":"^10","madler/zlib":"^1.3"}}"#
        );
    }

    #[test]
    fn package_detail_json_tolerates_odd_dependency_entries() {
        // Verified metadata is canonical, so a non-string, non-table
        // entry only arises from an invariant break; it renders as an
        // empty requirement instead of failing the response.
        let rows = [detail_row(
            "1.0.0",
            r#"{"dependencies":{"weird/dep":42}}"#,
            false,
            "2026-07-01T00:00:00.000Z",
            0,
        )];
        let body = package_detail_json("fmtlib", "fmt", &rows).unwrap();
        assert!(
            body.contains(r#""dependencies":{"weird/dep":""}"#),
            "{body}"
        );
        // A missing dependencies field is an empty map.
        let rows = [detail_row(
            "1.0.0",
            "{}",
            false,
            "2026-07-01T00:00:00.000Z",
            0,
        )];
        let body = package_detail_json("fmtlib", "fmt", &rows).unwrap();
        assert!(body.contains(r#""dependencies":{}"#), "{body}");
    }

    #[test]
    fn package_detail_json_rejects_invariant_breaks() {
        assert!(package_detail_json("fmtlib", "fmt", &[]).is_err());
        let rows = [detail_row(
            "1.0.0",
            "not json",
            false,
            "2026-07-01T00:00:00.000Z",
            0,
        )];
        let err = package_detail_json("fmtlib", "fmt", &rows).unwrap_err();
        assert!(err.contains("fmt@1.0.0"), "err: {err}");
        let rows = [detail_row(
            "1.0.0",
            "[]",
            false,
            "2026-07-01T00:00:00.000Z",
            0,
        )];
        assert!(package_detail_json("fmtlib", "fmt", &rows).is_err());
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
