//! Read-route matching and path-component validation.
//!
//! Validation happens here, before any D1 or R2 lookup: a path that does not
//! parse is an ordinary 404 and never reaches storage.

/// A matched, validated read route. Package identity is always the
/// scoped pair: `scope` is the registry-native namespace entity and
/// `name` the package part of the canonical `<scope>/<name>` name.
#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Healthz,
    Config,
    Package {
        scope: &'a str,
        name: &'a str,
    },
    Artifact {
        scope: &'a str,
        name: &'a str,
        version: &'a str,
    },
}

/// Matches `path` (percent-encoded, no query string) against the read
/// routes. Any percent-escape fails the component charsets below, so encoded
/// traversal never parses.
pub fn match_route(path: &str) -> Option<Route<'_>> {
    if path == "/healthz" {
        return Some(Route::Healthz);
    }
    if path == "/config.json" {
        return Some(Route::Config);
    }
    if let Some(rest) = path.strip_prefix("/packages/") {
        let (scope, name) = rest.split_once('/')?;
        let name = name.strip_suffix(".json")?;
        return (is_valid_scope(scope) && is_valid_name(name))
            .then_some(Route::Package { scope, name });
    }
    if let Some(rest) = path.strip_prefix("/artifacts/") {
        let (scope, rest) = rest.split_once('/')?;
        let (name, file) = rest.split_once('/')?;
        // The filename embeds the scope (a downloaded tarball stays
        // self-identifying outside the directory tree). Stripping the
        // literal `<scope>-<name>-` prefix stays unambiguous even though
        // scopes and names may themselves contain hyphens, because both
        // strings are already fixed by the directory segments; a filename
        // disagreeing with them fails parsing here.
        let version = file
            .strip_prefix(scope)?
            .strip_prefix('-')?
            .strip_prefix(name)?
            .strip_prefix('-')?
            .strip_suffix(".tar.gz")?;
        return (is_valid_scope(scope) && is_valid_name(name) && is_valid_version(version))
            .then_some(Route::Artifact {
                scope,
                name,
                version,
            });
    }
    None
}

/// A matched write (API) route. Unlike the read routes, the `scope` /
/// `name` / `version` segments are only split here, not validated:
/// publish validates them as part of its documented `400` sequence
/// (`crate::publish`), and yank and the admin verdict answer unknown
/// triples with an ordinary authenticated 404 straight from D1 (behind
/// the scope-membership gate, for yank). No segment ever becomes a path
/// or storage key by itself.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiRoute<'a> {
    Publish {
        scope: &'a str,
        name: &'a str,
        version: &'a str,
    },
    Yank {
        scope: &'a str,
        name: &'a str,
        version: &'a str,
    },
    /// `GET /api/v1/admin/versions?status=...`: the verifier's listing.
    AdminVersions,
    /// `PATCH /api/v1/admin/versions/<scope>/<name>/<version>`: a verdict.
    AdminVerdict {
        scope: &'a str,
        name: &'a str,
        version: &'a str,
    },
}

/// Matches `path` against the API routes:
/// `/api/v1/packages/<scope>/<name>/<version>`,
/// `/api/v1/packages/<scope>/<name>/<version>/yank`, and the admin
/// plane's `/api/v1/admin/versions[/<scope>/<name>/<version>]`.
pub fn match_api_route(path: &str) -> Option<ApiRoute<'_>> {
    if let Some(rest) = path.strip_prefix("/api/v1/admin/versions") {
        if rest.is_empty() {
            return Some(ApiRoute::AdminVersions);
        }
        let (scope, rest) = rest.strip_prefix('/')?.split_once('/')?;
        let (name, version) = rest.split_once('/')?;
        if scope.is_empty() || name.is_empty() || version.is_empty() || version.contains('/') {
            return None;
        }
        return Some(ApiRoute::AdminVerdict {
            scope,
            name,
            version,
        });
    }
    let rest = path.strip_prefix("/api/v1/packages/")?;
    let (scope, rest) = rest.split_once('/')?;
    let (name, rest) = rest.split_once('/')?;
    let (version, is_yank) = match rest.strip_suffix("/yank") {
        Some(version) => (version, true),
        None => (rest, false),
    };
    if scope.is_empty() || name.is_empty() || version.is_empty() || version.contains('/') {
        return None;
    }
    Some(if is_yank {
        ApiRoute::Yank {
            scope,
            name,
            version,
        }
    } else {
        ApiRoute::Publish {
            scope,
            name,
            version,
        }
    })
}

/// A matched OAuth (browser-plane) route, served on the website origin
/// only. These never accept bearer tokens, and the data routes above
/// never accept the session cookie.
#[derive(Debug, PartialEq, Eq)]
pub enum WebRoute<'a> {
    Login,
    Callback,
    /// `GET /claim/<scope>`: start a scope claim's dedicated OAuth
    /// roundtrip. The scope is validated here, before a cookie is
    /// minted or a redirect built.
    Claim {
        scope: &'a str,
    },
    /// `GET /callback/claim`: the claim flow's OAuth callback. A
    /// subdirectory of `/callback` on purpose: GitHub only accepts a
    /// `redirect_uri` at or under the OAuth app's registered callback
    /// URL, and the `cabinpkg.com/callback*` zone route already covers
    /// it.
    ClaimCallback,
}

/// Matches `path` against the OAuth routes.
pub fn match_web_route(path: &str) -> Option<WebRoute<'_>> {
    match path {
        "/login" => Some(WebRoute::Login),
        "/callback" => Some(WebRoute::Callback),
        "/callback/claim" => Some(WebRoute::ClaimCallback),
        _ => {
            let scope = path.strip_prefix("/claim/")?;
            is_valid_scope(scope).then_some(WebRoute::Claim { scope })
        }
    }
}

/// Where `/callback` sends the browser after a successful sign-in and on
/// a refused one, and where `/callback/claim` sends it after a granted
/// and a refused claim. All are relative paths rendered by the website;
/// they are never derived from request input, so neither callback can be
/// turned into an open redirect.
pub const POST_LOGIN_REDIRECT: &str = "/dashboard";
pub const LOGIN_DENIED_REDIRECT: &str = "/login/denied";
pub const CLAIM_GRANTED_REDIRECT: &str = "/dashboard?claim=granted";
pub const CLAIM_DENIED_REDIRECT: &str = "/dashboard?claim=denied";

/// A matched session-plane route: the JSON user API under
/// `/api/v1/user`, session-cookie authenticated on the website origin.
/// This subtree is session-only; every other `/api/` path is
/// Bearer-only ([`is_session_path`] draws the line).
#[derive(Debug, PartialEq, Eq)]
pub enum SessionRoute<'a> {
    /// `GET /api/v1/user`: who the session belongs to.
    User,
    /// `GET /api/v1/user/usage`: usage and quotas.
    Usage,
    /// `GET /api/v1/user/packages`: the user's packages and the
    /// verification/yanked state of every version.
    Packages,
    /// `GET` lists tokens, `POST` creates one.
    Tokens,
    /// `POST /api/v1/user/tokens/<id>/revoke`.
    RevokeToken { id: &'a str },
    /// `GET /api/v1/user/scopes/<scope>/members` lists a scope's
    /// members, `POST` adds one.
    ScopeMembers { scope: &'a str },
    /// `POST /api/v1/user/scopes/<scope>/members/<github_id>/remove`.
    RemoveScopeMember { scope: &'a str, github_id: i64 },
    /// `POST /api/v1/user/logout`: clear the session cookie.
    Logout,
}

/// Matches `path` against the session routes. Token ids
/// (`[A-Za-z0-9_-]+`), scopes (the scope grammar), and GitHub ids
/// (numeric) are validated here before they reach a D1 query.
pub fn match_session_route(path: &str) -> Option<SessionRoute<'_>> {
    match path {
        "/api/v1/user" => Some(SessionRoute::User),
        "/api/v1/user/usage" => Some(SessionRoute::Usage),
        "/api/v1/user/packages" => Some(SessionRoute::Packages),
        "/api/v1/user/tokens" => Some(SessionRoute::Tokens),
        "/api/v1/user/logout" => Some(SessionRoute::Logout),
        _ => {
            if let Some(rest) = path.strip_prefix("/api/v1/user/scopes/") {
                let (scope, rest) = rest.split_once('/')?;
                if !is_valid_scope(scope) {
                    return None;
                }
                if rest == "members" {
                    return Some(SessionRoute::ScopeMembers { scope });
                }
                let id = rest.strip_prefix("members/")?.strip_suffix("/remove")?;
                if !id.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let github_id: i64 = id.parse().ok()?;
                // Only the canonical decimal form GitHub ids are stored
                // as: a zero-padded spelling would dodge exact matches.
                if github_id.to_string() != id {
                    return None;
                }
                return Some(SessionRoute::RemoveScopeMember { scope, github_id });
            }
            let id = path
                .strip_prefix("/api/v1/user/tokens/")?
                .strip_suffix("/revoke")?;
            let valid = !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            valid.then_some(SessionRoute::RevokeToken { id })
        }
    }
}

/// Whether `path` lies in the session-only `/api/v1/user` subtree. The
/// glue routes the whole subtree to the session plane, so a bearer token
/// can never reach it - and a session cookie never reaches the rest of
/// `/api/`.
pub fn is_session_path(path: &str) -> bool {
    path == "/api/v1/user" || path.starts_with("/api/v1/user/")
}

/// The role a hostname serves: one role per hostname, dispatched on the
/// Host header. Unknown hosts get the registry role - the deny-by-default
/// plane where everything but the machine read routes is a uniform 401 -
/// so a stray hostname can never expose the browser plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The machine read plane only: `/config.json`, `/packages/*`,
    /// `/artifacts/*`, `/healthz`.
    Registry,
    /// The website origin: `/login`, `/callback`, and `/api/*` (the
    /// Bearer mutation routes plus the session user API).
    Website,
}

/// Which [`Role`] serves a request, from its Host header (no port) and
/// the host of the `WEB_ORIGIN` env var.
pub fn role_for_host(host: &str, web_host: &str) -> Role {
    if !web_host.is_empty() && host.eq_ignore_ascii_case(web_host) {
        Role::Website
    } else {
        Role::Registry
    }
}

/// Strips the `:port` suffix from a Host header value, keeping IPv6
/// bracket forms intact. The dispatch compares hostnames only: the edge
/// terminates TLS on standard ports, and local `wrangler dev` runs both
/// roles on one port.
pub fn host_without_port(host: &str) -> &str {
    if host.starts_with('[')
        && let Some(end) = host.find(']')
    {
        return &host[..=end];
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Scope names are restricted to `[a-z0-9]([a-z0-9-]*[a-z0-9])?`, at most
/// 39 characters, before they become path or key components. The grammar
/// is GitHub-login-compatible on purpose: a scope is claimed by proving
/// control of the same-named GitHub account (logins are lowercased at
/// claim time), so every claimable login must fit. It is deliberately a
/// small superset (GitHub also forbids consecutive hyphens):
/// claimability is proved by the claim flow's account-control check,
/// not by the charset, and an unclaimable string can never gain members,
/// so it answers the write plane's uniform 403 forever.
pub fn is_valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 39
        && scope
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !scope.starts_with('-')
        && !scope.ends_with('-')
}

/// Package names are restricted to `^[a-z0-9][a-z0-9_-]*$` - the publish
/// grammar - before they become path or key components, so the read and
/// write planes accept exactly the same names.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A valid-looking version: three dot-separated numeric components with an
/// optional `-pre` / `+build` suffix limited to `[A-Za-z0-9.+-]`. Path and
/// key safety is the point, not full `SemVer` pedantry.
pub fn is_valid_version(version: &str) -> bool {
    let core_len = version.find(['-', '+']).unwrap_or(version.len());
    let (core, suffix) = version.split_at(core_len);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_three_read_routes_and_healthz() {
        assert_eq!(match_route("/healthz"), Some(Route::Healthz));
        assert_eq!(match_route("/config.json"), Some(Route::Config));
        assert_eq!(
            match_route("/packages/fmtlib/fmt.json"),
            Some(Route::Package {
                scope: "fmtlib",
                name: "fmt"
            })
        );
        assert_eq!(
            match_route("/artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar.gz"),
            Some(Route::Artifact {
                scope: "fmtlib",
                name: "fmt",
                version: "10.2.1"
            })
        );
        // Hyphens in the scope and the name stay unambiguous: the
        // filename prefix is matched against the directory segments,
        // never re-split.
        assert_eq!(
            match_route("/artifacts/my-org/my_pkg-2/my-org-my_pkg-2-1.0.0-rc.1+build.5.tar.gz"),
            Some(Route::Artifact {
                scope: "my-org",
                name: "my_pkg-2",
                version: "1.0.0-rc.1+build.5"
            })
        );
    }

    #[test]
    fn rejects_unknown_paths() {
        for path in [
            "/",
            "",
            "/config.json/",
            "/packages/fmt.json",
            "/packages/fmtlib/fmt",
            "/packages/fmtlib/fmt.json.json/",
            "/packages/fmtlib/.json",
            "/packages/fmtlib/extra/fmt.json",
            "/artifacts/fmtlib",
            "/artifacts/fmtlib/fmt",
            "/artifacts/fmtlib/fmt/",
            "/artifacts/fmt/fmt-10.2.1.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar.gz/extra",
            "/index.html",
        ] {
            assert_eq!(match_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn rejects_artifact_filenames_disagreeing_with_the_directory() {
        // The filename must embed exactly `<scope>-<name>-`; any other
        // prefix - another scope, another name, or a bare name - fails
        // parsing before any lookup.
        for path in [
            "/artifacts/fmtlib/fmt/fmt-10.2.1.tar.gz",
            "/artifacts/fmtlib/fmt/other-fmt-10.2.1.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-other-10.2.1.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlibfmt-10.2.1.tar.gz",
            "/artifacts/my-org/pkg/my-org-2-pkg-1.0.0.tar.gz",
        ] {
            assert_eq!(match_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn rejects_invalid_path_components() {
        // Uppercase, dots, traversal, and percent-escapes never parse -
        // in either segment.
        for path in [
            "/packages/fmtlib/Fmt.json",
            "/packages/Fmtlib/fmt.json",
            "/packages/fmt.lib/fmt.json",
            "/packages/fmt_lib/fmt.json",
            "/packages/-fmtlib/fmt.json",
            "/packages/fmtlib-/fmt.json",
            "/packages/fmtlib/fmt..json",
            "/packages/fmtlib/..%2fescape.json",
            "/packages/..%2fescape/fmt.json",
            "/packages/fmtlib/fmt%2e.json",
            "/artifacts/../fmt/fmt-1.0.0.tar.gz",
            "/artifacts/fmtlib/../fmtlib-..-1.0.0.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-1.0.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.0.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-v1.0.0.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-1.0.x.tar.gz",
            "/artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0%2f.tar.gz",
        ] {
            assert_eq!(match_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn matches_the_two_api_routes() {
        assert_eq!(
            match_api_route("/api/v1/packages/fmtlib/fmt/10.2.1"),
            Some(ApiRoute::Publish {
                scope: "fmtlib",
                name: "fmt",
                version: "10.2.1"
            })
        );
        assert_eq!(
            match_api_route("/api/v1/packages/fmtlib/fmt/10.2.1/yank"),
            Some(ApiRoute::Yank {
                scope: "fmtlib",
                name: "fmt",
                version: "10.2.1"
            })
        );
        // Segments are split, not validated: garbage still routes and
        // fails later with the documented status.
        assert_eq!(
            match_api_route("/api/v1/packages/Fmtlib/Fmt/not-semver"),
            Some(ApiRoute::Publish {
                scope: "Fmtlib",
                name: "Fmt",
                version: "not-semver"
            })
        );
    }

    #[test]
    fn matches_the_web_routes() {
        assert_eq!(match_web_route("/login"), Some(WebRoute::Login));
        assert_eq!(match_web_route("/callback"), Some(WebRoute::Callback));
        assert_eq!(
            match_web_route("/claim/fmtlib"),
            Some(WebRoute::Claim { scope: "fmtlib" })
        );
        assert_eq!(
            match_web_route("/claim/my-org"),
            Some(WebRoute::Claim { scope: "my-org" })
        );
        // The exact path wins over the claim pattern; "claim" itself
        // stays claimable.
        assert_eq!(
            match_web_route("/callback/claim"),
            Some(WebRoute::ClaimCallback)
        );
        assert_eq!(
            match_web_route("/claim/claim"),
            Some(WebRoute::Claim { scope: "claim" })
        );
    }

    #[test]
    fn rejects_malformed_web_paths() {
        // `/me` is gone on purpose: the pre-1.0 no-deprecation convention
        // means no redirect shim either.
        for path in [
            "/",
            "/login/",
            "/callback/",
            "/me",
            "/me/tokens",
            "/claim",
            "/claim/",
            "/claim/Fmtlib",
            "/claim/fmt.lib",
            "/claim/-fmtlib",
            "/claim/fmtlib/extra",
            "/claim/..%2fescape",
            "/callback/claim/",
            "/callback/other",
        ] {
            assert_eq!(match_web_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn matches_the_session_routes() {
        assert_eq!(
            match_session_route("/api/v1/user"),
            Some(SessionRoute::User)
        );
        assert_eq!(
            match_session_route("/api/v1/user/usage"),
            Some(SessionRoute::Usage)
        );
        assert_eq!(
            match_session_route("/api/v1/user/packages"),
            Some(SessionRoute::Packages)
        );
        assert_eq!(
            match_session_route("/api/v1/user/tokens"),
            Some(SessionRoute::Tokens)
        );
        assert_eq!(
            match_session_route("/api/v1/user/tokens/0aB_-9/revoke"),
            Some(SessionRoute::RevokeToken { id: "0aB_-9" })
        );
        assert_eq!(
            match_session_route("/api/v1/user/logout"),
            Some(SessionRoute::Logout)
        );
        assert_eq!(
            match_session_route("/api/v1/user/scopes/fmtlib/members"),
            Some(SessionRoute::ScopeMembers { scope: "fmtlib" })
        );
        assert_eq!(
            match_session_route("/api/v1/user/scopes/my-org/members/26405363/remove"),
            Some(SessionRoute::RemoveScopeMember {
                scope: "my-org",
                github_id: 26_405_363
            })
        );
    }

    #[test]
    fn rejects_malformed_session_paths() {
        for path in [
            "/",
            "/api/v1/user/",
            "/api/v1/user/tokens/",
            "/api/v1/user/tokens//revoke",
            "/api/v1/user/tokens/abc",
            "/api/v1/user/tokens/abc/revoke/extra",
            "/api/v1/user/tokens/a.b/revoke",
            "/api/v1/user/tokens/a%2f/revoke",
            "/api/v1/user/packages/",
            "/api/v1/user/packages/fmt",
            "/api/v1/user/logout/",
            "/api/v1/users",
            "/api/v1/user/scopes",
            "/api/v1/user/scopes/",
            "/api/v1/user/scopes/fmtlib",
            "/api/v1/user/scopes/fmtlib/",
            "/api/v1/user/scopes/fmtlib/members/",
            "/api/v1/user/scopes/Fmtlib/members",
            "/api/v1/user/scopes/fmt.lib/members",
            "/api/v1/user/scopes//members",
            "/api/v1/user/scopes/fmtlib/members/26405363",
            "/api/v1/user/scopes/fmtlib/members//remove",
            "/api/v1/user/scopes/fmtlib/members/abc/remove",
            "/api/v1/user/scopes/fmtlib/members/-1/remove",
            "/api/v1/user/scopes/fmtlib/members/007/remove",
            "/api/v1/user/scopes/fmtlib/members/99999999999999999999/remove",
            "/api/v1/user/scopes/fmtlib/members/1/remove/extra",
        ] {
            assert_eq!(match_session_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn the_session_subtree_never_overlaps_the_bearer_routes() {
        // The route-level plane split: everything under /api/v1/user is
        // session-only and must be invisible to the bearer matcher, and
        // the bearer routes must sit outside the subtree.
        for path in [
            "/api/v1/user",
            "/api/v1/user/usage",
            "/api/v1/user/tokens",
            "/api/v1/user/tokens/abc/revoke",
            "/api/v1/user/scopes/fmtlib/members",
            "/api/v1/user/scopes/fmtlib/members/1/remove",
            "/api/v1/user/anything/else",
        ] {
            assert!(is_session_path(path), "path: {path:?}");
            assert_eq!(match_api_route(path), None, "path: {path:?}");
        }
        for path in [
            "/api/v1/users",
            "/api/v1/packages/fmt/10.2.1",
            "/api/v1/admin/versions",
        ] {
            assert!(!is_session_path(path), "path: {path:?}");
        }
    }

    #[test]
    fn hosts_map_to_exactly_one_role() {
        assert_eq!(role_for_host("cabinpkg.com", "cabinpkg.com"), Role::Website);
        assert_eq!(role_for_host("CABINPKG.COM", "cabinpkg.com"), Role::Website);
        for host in ["registry.cabinpkg.com", "evil.example.com", ""] {
            assert_eq!(
                role_for_host(host, "cabinpkg.com"),
                Role::Registry,
                "host: {host:?}"
            );
        }
        // A missing WEB_ORIGIN host can never grant the website role.
        assert_eq!(role_for_host("", ""), Role::Registry);
    }

    #[test]
    fn host_header_ports_are_stripped() {
        assert_eq!(host_without_port("cabinpkg.com"), "cabinpkg.com");
        assert_eq!(host_without_port("localhost:8787"), "localhost");
        assert_eq!(host_without_port("[::1]:8787"), "[::1]");
        assert_eq!(host_without_port("[::1]"), "[::1]");
        assert_eq!(host_without_port(""), "");
    }

    #[test]
    fn post_login_redirects_are_relative_paths() {
        // The open-redirect guard: every target is a same-origin relative
        // path, never an absolute URL (and never protocol-relative `//`).
        for target in [
            POST_LOGIN_REDIRECT,
            LOGIN_DENIED_REDIRECT,
            CLAIM_GRANTED_REDIRECT,
            CLAIM_DENIED_REDIRECT,
        ] {
            assert!(target.starts_with('/'), "target: {target:?}");
            assert!(!target.starts_with("//"), "target: {target:?}");
            assert!(!target.contains("://"), "target: {target:?}");
        }
    }

    #[test]
    fn matches_the_admin_routes() {
        assert_eq!(
            match_api_route("/api/v1/admin/versions"),
            Some(ApiRoute::AdminVersions)
        );
        assert_eq!(
            match_api_route("/api/v1/admin/versions/fmtlib/fmt/10.2.1"),
            Some(ApiRoute::AdminVerdict {
                scope: "fmtlib",
                name: "fmt",
                version: "10.2.1"
            })
        );
        // Like publish and yank, segments are split, not validated:
        // garbage routes and 404s from D1.
        assert_eq!(
            match_api_route("/api/v1/admin/versions/Fmtlib/Fmt/not-semver"),
            Some(ApiRoute::AdminVerdict {
                scope: "Fmtlib",
                name: "Fmt",
                version: "not-semver"
            })
        );
    }

    #[test]
    fn rejects_malformed_api_paths() {
        for path in [
            "/api/v1/packages",
            "/api/v1/packages/",
            "/api/v1/packages/fmt",
            "/api/v1/packages/fmt/10.2.1",
            "/api/v1/packages/fmtlib/fmt",
            "/api/v1/packages/fmtlib/fmt/",
            "/api/v1/packages//fmt/10.2.1",
            "/api/v1/packages/fmtlib//10.2.1",
            "/api/v1/packages/fmtlib/fmt/10.2.1/extra",
            "/api/v1/packages/fmtlib/fmt/10.2.1/yank/extra",
            "/api/v1/packages/fmtlib/fmt//yank",
            "/api/v2/packages/fmtlib/fmt/10.2.1",
            "/api/v1/admin/versions/",
            "/api/v1/admin/versions/fmt",
            "/api/v1/admin/versions/fmt/10.2.1",
            "/api/v1/admin/versions/fmtlib/fmt/",
            "/api/v1/admin/versions//fmt/10.2.1",
            "/api/v1/admin/versions/fmtlib//10.2.1",
            "/api/v1/admin/versions/fmtlib/fmt/10.2.1/extra",
            "/api/v1/admin/other",
        ] {
            assert_eq!(match_api_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn name_validation_is_the_documented_charset() {
        for name in ["fmt", "my_pkg", "pkg-2", "0abc"] {
            assert!(is_valid_name(name), "name: {name:?}");
        }
        for name in ["", "Fmt", "pkg.json", "a/b", "a b", "naïve", "_fmt", "-fmt"] {
            assert!(!is_valid_name(name), "name: {name:?}");
        }
    }

    #[test]
    fn scope_validation_is_github_login_compatible() {
        for scope in ["fmtlib", "a", "0", "my-org", "a-b-c", &"x".repeat(39)] {
            assert!(is_valid_scope(scope), "scope: {scope:?}");
        }
        for scope in [
            "",
            "-fmtlib",
            "fmtlib-",
            "fmt_lib",
            "Fmtlib",
            "fmt.lib",
            "a/b",
            "naïve",
            &"x".repeat(40),
        ] {
            assert!(!is_valid_scope(scope), "scope: {scope:?}");
        }
    }

    #[test]
    fn version_validation_accepts_semver_shapes_only() {
        for version in [
            "0.0.0",
            "10.2.1",
            "1.0.0-rc.1",
            "1.0.0+build",
            "1.0.0-rc.1+b-2",
        ] {
            assert!(is_valid_version(version), "version: {version:?}");
        }
        for version in [
            "", "1", "1.0", "1.0.0.0", "v1.0.0", "1..0", "1.0.a", "1.0.0/x", "1.0.0-ü",
        ] {
            assert!(!is_valid_version(version), "version: {version:?}");
        }
    }
}
