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
