//! The scope-claim grant rules and the parsing of the GitHub responses
//! they read: pure and host-testable; the OAuth roundtrip and the D1
//! writes live in `web_glue`.
//!
//! A scope is granted by proving control of the same-named GitHub
//! account at claim time (`docs/architecture.md`, "Scopes"): either the
//! account is the authenticated user themself (self-claim), or it is an
//! organization the user administers. Every check that crosses two API
//! responses is bound by **numeric id**, never by login - logins can be
//! renamed and reassigned between calls; ids cannot. Every refusal is
//! uniform: the callback redirects to the same denied page whatever
//! failed.

use serde::Deserialize;

/// Whether claiming `scope` is a self-claim for the authenticated GitHub
/// user: the scope must equal the login lowercased (GitHub logins are
/// case-insensitively unique, and scopes are the lowercase spelling).
pub fn is_self_claim(scope: &str, login: &str) -> bool {
    login.to_ascii_lowercase() == scope
}

/// The fields of a `GET /orgs/<scope>/memberships/<login>` response the
/// org-claim check reads: state and role decide, and the nested numeric
/// ids bind the response to the authenticated claimant and to the
/// organization whose id the claim freezes.
#[derive(Deserialize)]
struct OrgMembership {
    state: String,
    role: String,
    user: AccountRef,
    organization: AccountRef,
}

#[derive(Deserialize)]
struct AccountRef {
    id: i64,
}

/// The organization's numeric id iff the membership response proves the
/// claimant's administrative control: the membership must be `active`
/// (not a pending invitation) with the `admin` role, and its `user`
/// must be the authenticated claimant - the request addressed a login,
/// which may have changed hands since `/user` answered. Anything else -
/// including a body that does not parse - refuses.
pub fn org_membership_grant(body: &[u8], claimant_id: i64) -> Option<i64> {
    let membership: OrgMembership = serde_json::from_slice(body).ok()?;
    (membership.state == "active"
        && membership.role == "admin"
        && membership.user.id == claimant_id)
        .then_some(membership.organization.id)
}

/// The one `GET /users/<scope>` field the claim records: the numeric
/// account id the scope string is frozen to. The caller must bind it -
/// to the claimant's own id for a self-claim, to the membership's
/// organization id for an org claim - before recording it.
#[derive(Deserialize)]
struct Account {
    id: i64,
}

/// The numeric account id in a `GET /users/<scope>` response body, if it
/// parses.
pub fn account_id(body: &[u8]) -> Option<i64> {
    serde_json::from_slice::<Account>(body)
        .ok()
        .map(|account| account.id)
}

/// Whether the requested scope must be refused on name-fidelity
/// grounds (`docs/architecture.md`, "Name fidelity"): the reserved
/// vocabulary is absolute, and skeleton equality with any
/// already-claimed scope refuses - the homoglyph-style squat - unless
/// the operator listed this exact scope in the exemption override
/// (`CLAIM_SKELETON_EXEMPT_SCOPES`, for the rare legitimate
/// collision). Skeleton equality only, deliberately no edit distance:
/// a claim refusal is hard and unexplained, and distance-1 collides
/// with real login patterns (`jsmith` / `jsmith1`). Exact re-claims
/// are refused by the permanence pre-check either way.
pub fn scope_refusal(scope: &str, claimed: &[String], exempt: &[String]) -> bool {
    if crate::names::is_reserved(scope) {
        return true;
    }
    if exempt.iter().any(|exempted| exempted == scope) {
        return false;
    }
    let target = crate::names::skeleton(scope);
    claimed
        .iter()
        .any(|name| crate::names::skeleton(name) == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_claims_compare_the_lowercased_login() {
        assert!(is_self_claim("ken-matsui", "ken-matsui"));
        assert!(is_self_claim("ken-matsui", "Ken-Matsui"));
        assert!(!is_self_claim("fmtlib", "ken-matsui"));
        // The scope side is already lowercase by grammar; a non-lowercase
        // scope never reaches here, and would not match anyway.
        assert!(!is_self_claim("Ken-Matsui", "Ken-Matsui"));
    }

    #[test]
    fn org_membership_grants_only_active_admins_bound_by_id() {
        // GitHub answers with many more fields; state and role decide,
        // and the nested ids bind the grant.
        let body = br#"{"state":"active","role":"admin","user":{"login":"octocat","id":583231},"organization":{"login":"fmtlib","id":7280970}}"#;
        assert_eq!(org_membership_grant(body, 583_231), Some(7_280_970));
        // The same response proves nothing for a *different*
        // authenticated user: the login the URL addressed may have been
        // reassigned between the `/user` and membership calls.
        assert_eq!(org_membership_grant(body, 26_405_363), None);
        for body in [
            &br#"{"state":"active","role":"member","user":{"id":583231},"organization":{"id":7280970}}"#[..],
            br#"{"state":"pending","role":"admin","user":{"id":583231},"organization":{"id":7280970}}"#,
            br#"{"state":"active","role":"admin","user":{"id":583231}}"#,
            br#"{"state":"active","role":"admin","organization":{"id":7280970}}"#,
            br#"{"state":"active","role":"admin"}"#,
            br#"{"message":"Not Found"}"#,
            br"not json",
            b"",
        ] {
            assert_eq!(
                org_membership_grant(body, 583_231),
                None,
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn scope_refusal_covers_reserved_and_confusable_scopes() {
        let claimed = vec!["fmtlib".to_owned(), "jsmith".to_owned()];
        let none: Vec<String> = Vec::new();

        // Reserved vocabulary refuses even with nothing claimed, and
        // the exemption override never bypasses it.
        for scope in ["con", "com1", "cabin", "std"] {
            assert!(scope_refusal(scope, &none, &none), "scope: {scope:?}");
            assert!(
                scope_refusal(scope, &none, &[scope.to_owned()]),
                "scope: {scope:?}"
            );
        }

        // Skeleton equality with a claimed scope refuses: the
        // homoglyph squat, the separator-drop, and the exact name
        // (which the permanence pre-check refuses anyway).
        for scope in ["fmtl1b", "fmt-lib", "fmtlib"] {
            assert!(scope_refusal(scope, &claimed, &none), "scope: {scope:?}");
        }
        // Distance-1 login patterns stay claimable: skeleton equality
        // only, no edit distance.
        for scope in ["jsmith1", "jsmiths", "fmtlib2"] {
            assert!(!scope_refusal(scope, &claimed, &none), "scope: {scope:?}");
        }

        // The operator override admits exactly the listed scope.
        let exempt = vec!["fmtl1b".to_owned()];
        assert!(!scope_refusal("fmtl1b", &claimed, &exempt));
        assert!(scope_refusal("fmt-lib", &claimed, &exempt));
    }

    #[test]
    fn account_ids_parse_from_the_users_response() {
        assert_eq!(
            account_id(br#"{"login":"fmtlib","id":7280970,"type":"Organization"}"#),
            Some(7_280_970)
        );
        assert_eq!(
            account_id(br#"{"login":"octocat","id":583231,"type":"User"}"#),
            Some(583_231)
        );
        for body in [&br#"{"login":"fmtlib"}"#[..], br#"{"id":"7280970"}"#, b""] {
            assert_eq!(
                account_id(body),
                None,
                "body: {}",
                String::from_utf8_lossy(body)
            );
        }
    }
}
