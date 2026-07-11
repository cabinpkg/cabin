//! Read-route matching and path-component validation.
//!
//! Validation happens here, before any D1 or R2 lookup: a path that does not
//! parse is an ordinary 404 and never reaches storage.

/// A matched, validated read route.
#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    Healthz,
    Config,
    Package { name: &'a str },
    Artifact { name: &'a str, version: &'a str },
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
        let name = rest.strip_suffix(".json")?;
        return is_valid_name(name).then_some(Route::Package { name });
    }
    if let Some(rest) = path.strip_prefix("/artifacts/") {
        let (name, file) = rest.split_once('/')?;
        let version = file
            .strip_prefix(name)?
            .strip_prefix('-')?
            .strip_suffix(".tar.gz")?;
        return (is_valid_name(name) && is_valid_version(version))
            .then_some(Route::Artifact { name, version });
    }
    None
}

/// A matched write (API) route. Unlike the read routes, the `name` /
/// `version` segments are only split here, not validated: publish
/// validates them as part of its documented `400` sequence
/// (`crate::publish`), and yank and the admin verdict answer unknown
/// pairs with an ordinary authenticated 404 straight from D1. Neither
/// segment ever becomes a path or storage key by itself.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiRoute<'a> {
    Publish {
        name: &'a str,
        version: &'a str,
    },
    Yank {
        name: &'a str,
        version: &'a str,
    },
    /// `GET /api/v1/admin/versions?status=...`: the verifier's listing.
    AdminVersions,
    /// `PATCH /api/v1/admin/versions/<name>/<version>`: a verdict.
    AdminVerdict {
        name: &'a str,
        version: &'a str,
    },
}

/// Matches `path` against the API routes:
/// `/api/v1/packages/<name>/<version>`,
/// `/api/v1/packages/<name>/<version>/yank`, and the admin plane's
/// `/api/v1/admin/versions[/<name>/<version>]`.
pub fn match_api_route(path: &str) -> Option<ApiRoute<'_>> {
    if let Some(rest) = path.strip_prefix("/api/v1/admin/versions") {
        if rest.is_empty() {
            return Some(ApiRoute::AdminVersions);
        }
        let (name, version) = rest.strip_prefix('/')?.split_once('/')?;
        if name.is_empty() || version.is_empty() || version.contains('/') {
            return None;
        }
        return Some(ApiRoute::AdminVerdict { name, version });
    }
    let rest = path.strip_prefix("/api/v1/packages/")?;
    let (name, rest) = rest.split_once('/')?;
    let (version, is_yank) = match rest.strip_suffix("/yank") {
        Some(version) => (version, true),
        None => (rest, false),
    };
    if name.is_empty() || version.is_empty() || version.contains('/') {
        return None;
    }
    Some(if is_yank {
        ApiRoute::Yank { name, version }
    } else {
        ApiRoute::Publish { name, version }
    })
}

/// A matched OAuth (browser-plane) route, served on the website origin
/// only. These never accept bearer tokens, and the data routes above
/// never accept the session cookie.
#[derive(Debug, PartialEq, Eq)]
pub enum WebRoute {
    Login,
    Callback,
}

/// Matches `path` against the OAuth routes.
pub fn match_web_route(path: &str) -> Option<WebRoute> {
    match path {
        "/login" => Some(WebRoute::Login),
        "/callback" => Some(WebRoute::Callback),
        _ => None,
    }
}

/// Where `/callback` sends the browser after a successful sign-in and on
/// a refused one. Both are relative paths rendered by the website; they
/// are never derived from request input, so the callback cannot be turned
/// into an open redirect.
pub const POST_LOGIN_REDIRECT: &str = "/dashboard";
pub const LOGIN_DENIED_REDIRECT: &str = "/login/denied";

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
    /// `POST /api/v1/user/logout`: clear the session cookie.
    Logout,
}

/// Matches `path` against the session routes. Token ids are validated
/// here (`[A-Za-z0-9_-]+`) before they reach a D1 query.
pub fn match_session_route(path: &str) -> Option<SessionRoute<'_>> {
    match path {
        "/api/v1/user" => Some(SessionRoute::User),
        "/api/v1/user/usage" => Some(SessionRoute::Usage),
        "/api/v1/user/packages" => Some(SessionRoute::Packages),
        "/api/v1/user/tokens" => Some(SessionRoute::Tokens),
        "/api/v1/user/logout" => Some(SessionRoute::Logout),
        _ => {
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

/// Package names are restricted to `[a-z0-9_-]+` before they become path or
/// key components.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
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
            match_route("/packages/fmt.json"),
            Some(Route::Package { name: "fmt" })
        );
        assert_eq!(
            match_route("/artifacts/fmt/fmt-10.2.1.tar.gz"),
            Some(Route::Artifact {
                name: "fmt",
                version: "10.2.1"
            })
        );
        assert_eq!(
            match_route("/artifacts/my_pkg-2/my_pkg-2-1.0.0-rc.1+build.5.tar.gz"),
            Some(Route::Artifact {
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
            "/packages/fmt",
            "/packages/fmt.json.json/",
            "/packages/.json",
            "/artifacts/fmt",
            "/artifacts/fmt/",
            "/artifacts/fmt/fmt-10.2.1.tar",
            "/artifacts/fmt/other-10.2.1.tar.gz",
            "/artifacts/fmt/fmt-10.2.1.tar.gz/extra",
            "/index.html",
        ] {
            assert_eq!(match_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn rejects_invalid_path_components() {
        // Uppercase, dots, traversal, and percent-escapes never parse.
        for path in [
            "/packages/Fmt.json",
            "/packages/fmt..json",
            "/packages/..%2fescape.json",
            "/packages/fmt%2e.json",
            "/artifacts/../fmt-1.0.0.tar.gz",
            "/artifacts/fmt/fmt-1.0.tar.gz",
            "/artifacts/fmt/fmt-1.0.0.0.tar.gz",
            "/artifacts/fmt/fmt-v1.0.0.tar.gz",
            "/artifacts/fmt/fmt-1.0.x.tar.gz",
            "/artifacts/fmt/fmt-1.0.0%2f.tar.gz",
        ] {
            assert_eq!(match_route(path), None, "path: {path:?}");
        }
    }

    #[test]
    fn matches_the_two_api_routes() {
        assert_eq!(
            match_api_route("/api/v1/packages/fmt/10.2.1"),
            Some(ApiRoute::Publish {
                name: "fmt",
                version: "10.2.1"
            })
        );
        assert_eq!(
            match_api_route("/api/v1/packages/fmt/10.2.1/yank"),
            Some(ApiRoute::Yank {
                name: "fmt",
                version: "10.2.1"
            })
        );
        // Segments are split, not validated: garbage still routes and
        // fails later with the documented status.
        assert_eq!(
            match_api_route("/api/v1/packages/Fmt/not-semver"),
            Some(ApiRoute::Publish {
                name: "Fmt",
                version: "not-semver"
            })
        );
    }

    #[test]
    fn matches_the_web_routes() {
        assert_eq!(match_web_route("/login"), Some(WebRoute::Login));
        assert_eq!(match_web_route("/callback"), Some(WebRoute::Callback));
    }

    #[test]
    fn rejects_malformed_web_paths() {
        // `/me` is gone on purpose: the pre-1.0 no-deprecation convention
        // means no redirect shim either.
        for path in ["/", "/login/", "/callback/", "/me", "/me/tokens"] {
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
        for host in [
            "dev-registry.cabinpkg.com",
            "registry.cabinpkg.com",
            "evil.example.com",
            "",
        ] {
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
        // The open-redirect guard: both targets are same-origin relative
        // paths, never absolute URLs (and never protocol-relative `//`).
        for target in [POST_LOGIN_REDIRECT, LOGIN_DENIED_REDIRECT] {
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
            match_api_route("/api/v1/admin/versions/fmt/10.2.1"),
            Some(ApiRoute::AdminVerdict {
                name: "fmt",
                version: "10.2.1"
            })
        );
        // Like publish and yank, segments are split, not validated:
        // garbage routes and 404s from D1.
        assert_eq!(
            match_api_route("/api/v1/admin/versions/Fmt/not-semver"),
            Some(ApiRoute::AdminVerdict {
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
            "/api/v1/packages/fmt/",
            "/api/v1/packages//10.2.1",
            "/api/v1/packages/fmt/10.2.1/extra",
            "/api/v1/packages/fmt/10.2.1/yank/extra",
            "/api/v1/packages/fmt//yank",
            "/api/v2/packages/fmt/10.2.1",
            "/api/v1/admin/versions/",
            "/api/v1/admin/versions/fmt",
            "/api/v1/admin/versions/fmt/",
            "/api/v1/admin/versions//10.2.1",
            "/api/v1/admin/versions/fmt/10.2.1/extra",
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
        for name in ["", "Fmt", "pkg.json", "a/b", "a b", "naïve"] {
            assert!(!is_valid_name(name), "name: {name:?}");
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
