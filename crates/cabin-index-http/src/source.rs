use std::collections::{BTreeMap, VecDeque};

use cabin_core::registry::{REGISTRY_CONFIG_SCHEMA, REGISTRY_KIND, relative_subdir_is_safe};
use cabin_core::{PackageName, TargetPlatform};
use cabin_index::{IndexEntry, IndexError, IndexPackageDependency, PackageIndex, SourceContext};
use serde::Deserialize;

use crate::client::HttpClient;
use crate::error::IndexHttpError;

/// Parsed-and-validated `<base>/config.json` document. The fields
/// mirror `cabin_registry_file::RegistryConfig`; we re-implement the
/// shape here so `cabin-index-http` does not depend on that crate.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpIndexConfig {
    schema: u32,
    kind: String,
    packages: String,
    artifacts: String,
}

/// HTTP-backed sparse index source.
///
/// Built from a base URL plus an [`HttpClient`]; calls into the
/// client to fetch `config.json` once, and subsequent
/// [`HttpIndex::fetch_package`] / [`HttpIndex::load_package_index`]
/// calls fetch per-package metadata.
#[derive(Debug, Clone)]
pub struct HttpIndex {
    /// Normalized base URL, always with a trailing `/`.
    base: url::Url,
    /// Pre-resolved `<base>/<config.packages>/`. Used as the parent
    /// URL when resolving relative `source.path` values.
    packages_base: url::Url,
    client: HttpClient,
}

impl HttpIndex {
    /// Connect to the registry at `base_url`, fetch and validate
    /// `<base_url>/config.json`. The base URL is normalized so a
    /// trailing slash is optional.
    pub fn open(base_url: &str, client: HttpClient) -> Result<Self, IndexHttpError> {
        let base = parse_base_url(base_url)?;
        let config_url = base
            .join("config.json")
            .map_err(|err| IndexHttpError::InvalidUrl {
                url: base_url.to_owned(),
                message: format!("cannot append config.json: {err}"),
            })?;

        let body = client.get_bytes(config_url.as_str(), "config")?;
        let raw: RawRegistryConfig =
            serde_json::from_slice(&body).map_err(|err| IndexHttpError::InvalidConfig {
                base_url: base.to_string(),
                message: format!("config.json is not valid JSON: {err}"),
            })?;
        let config = HttpIndexConfig::from_raw(raw, &base)?;

        let packages_base = base.join(&format!("{}/", config.packages)).map_err(|err| {
            IndexHttpError::InvalidConfig {
                base_url: base.to_string(),
                message: format!("`packages` produces an invalid URL: {err}"),
            }
        })?;

        Ok(Self {
            base,
            packages_base,
            client,
        })
    }

    /// `GET <base>/<config.packages>/<name>.json` and parse the
    /// document into an [`IndexEntry`]. Source-path resolution is
    /// performed inside this call so the returned entry's
    /// [`cabin_index::SourceLocation::HttpUrl`] is ready to download.
    pub fn fetch_package(&self, name: &PackageName) -> Result<IndexEntry, IndexHttpError> {
        // Defense-in-depth at the URL boundary.
        // `PackageName::new` already rejects unsafe names, but
        // tooling that constructs a `PackageName` via private
        // means or skipped validation must not be able to escape
        // the configured packages directory through this fetch.
        ensure_path_safe(name.as_str())?;
        let package_url = self.package_url(name.as_str())?;
        let body = self.client.get_bytes(package_url.as_str(), name.as_str())?;
        let body_str =
            std::str::from_utf8(&body).map_err(|err| IndexHttpError::InvalidMetadata {
                name: name.as_str().to_owned(),
                message: format!("response body is not valid UTF-8: {err}"),
            })?;

        let resolver = make_source_resolver(package_url);
        let context = SourceContext::HttpUrl(&resolver);
        let entry = cabin_index::parse_package_entry(body_str, Some(name.as_str()), &context, None)
            .map_err(|err| match err {
                IndexError::Json { source, .. } => IndexHttpError::InvalidMetadata {
                    name: name.as_str().to_owned(),
                    message: source.to_string(),
                },
                IndexError::NameMismatch {
                    declared, stem, ..
                } => IndexHttpError::InvalidMetadata {
                    name: name.as_str().to_owned(),
                    message: format!(
                        "package metadata declares {declared:?} but `--index-url` was queried for {stem:?}"
                    ),
                },
                other => IndexHttpError::Index(other),
            })?;
        Ok(entry)
    }

    /// Walk root dependencies (and every package transitively
    /// referenced from them) by name, fetching each `<name>.json`
    /// Over HTTP, and assemble a [`PackageIndex`] that matches the
    /// shape produced by the local file loader.
    ///
    /// The walker only fetches packages that are reachable from
    /// `roots`; a sparse HTTP registry can hold thousands of
    /// packages, but a single `cabin resolve` run only ever
    /// references the closure of its declared dependencies.
    pub fn load_package_index(
        &self,
        roots: &[PackageName],
    ) -> Result<PackageIndex, IndexHttpError> {
        let mut packages: BTreeMap<PackageName, IndexEntry> = BTreeMap::new();
        let mut queue: VecDeque<PackageName> = roots.iter().cloned().collect();
        // Defense-in-depth: re-validate every root name before
        // it reaches the URL builder. `PackageName::new` already
        // rejects unsafe names, but the walker is the boundary
        // that turns a `PackageName` into an HTTP path segment
        // so the explicit gate keeps the rule visible.
        for name in &queue {
            ensure_path_safe(name.as_str())?;
        }
        let platform = TargetPlatform::current();
        while let Some(name) = queue.pop_front() {
            if packages.contains_key(&name) {
                continue;
            }
            let entry = self.fetch_package(&name)?;
            for version_meta in entry.versions.values() {
                // Mirror the resolver's transitive walk: include
                // normal deps (dev deps and system deps are not
                // part of resolve/fetch), skip disabled optional
                // deps, and skip deps whose `cfg(...)` predicate
                // fails on the host platform. Walking every
                // version of every reachable package is necessary
                // because the resolver may select any non-yanked
                // version.
                let kinded = version_meta.dependencies.iter();
                for (dep_name, dep_entry) in kinded {
                    if !active_registry_dep(dep_entry, &platform) {
                        continue;
                    }
                    // Re-check transitive names too: even though
                    // `cabin_index::parse_package_entry` constructs
                    // each `PackageName` through `PackageName::new`,
                    // this check pins the rule at the URL-building
                    // boundary.
                    ensure_path_safe(dep_name.as_str())?;
                    if !packages.contains_key(dep_name) {
                        queue.push_back(dep_name.clone());
                    }
                }
            }
            packages.insert(name, entry);
        }
        Ok(PackageIndex {
            // Use the base URL string as the displayable root.
            root: std::path::PathBuf::from(self.base.as_str()),
            packages,
        })
    }

    fn package_url(&self, name: &str) -> Result<url::Url, IndexHttpError> {
        let relative = format!("{name}.json");
        self.packages_base
            .join(&relative)
            .map_err(|err| IndexHttpError::InvalidUrl {
                url: format!("{}{relative}", self.packages_base),
                message: err.to_string(),
            })
    }
}

/// Shared path-safety gate at the sparse-HTTP fetch boundary.
/// Delegates to the `cabin-core` predicate so this crate cannot
/// drift on the rule. Used both when the user supplies a package
/// name directly (`fetch_package`) and when the walker queues a
/// transitive dependency name parsed from registry metadata
/// (`load_package_index`).
fn ensure_path_safe(name: &str) -> Result<(), IndexHttpError> {
    if !cabin_core::is_path_safe_package_name(name) {
        return Err(IndexHttpError::UnsafePackageName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

/// Whether a registry-package dependency edge participates in
/// the documented normal+build+tool resolution closure on this
/// host. The walker queues only edges that survive this filter,
/// so unenabled optional deps and target-conditioned deps that
/// do not match the host platform never trigger a per-package
/// HTTP fetch. Mirrors the resolver's per-version filter so the
/// sparse-index prefetch and the resolver agree on what reaches
/// the [`PackageIndex`].
fn active_registry_dep(dep: &IndexPackageDependency, platform: &TargetPlatform) -> bool {
    if dep.optional {
        return false;
    }
    if let Some(cond) = &dep.condition
        && !cond.evaluate(platform)
    {
        return false;
    }
    true
}

/// Normalize a base URL: accept the input with or without a trailing
/// slash, reject schemes other than `http(s)`, and reject URLs that
/// carry `userinfo` so credentials never reach the wire or surface
/// in transport errors. This is a defense-in-depth: the config layer
/// rejects credential-bearing `index-url` values, but the HTTP layer
/// is also reachable from the CLI override (`--index-url`), so the
/// check is duplicated here so every entry point fails closed.
pub(crate) fn parse_base_url(input: &str) -> Result<url::Url, IndexHttpError> {
    let mut parsed = url::Url::parse(input).map_err(|err| IndexHttpError::InvalidUrl {
        url: input.to_owned(),
        message: err.to_string(),
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(IndexHttpError::InvalidUrl {
                url: input.to_owned(),
                message: format!("unsupported URL scheme {other:?}"),
            });
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(IndexHttpError::InvalidUrl {
            url: redact_url_userinfo(&parsed),
            message: "URL must not contain credentials (userinfo)".to_owned(),
        });
    }
    if !parsed.path().ends_with('/') {
        let new_path = format!("{}/", parsed.path());
        parsed.set_path(&new_path);
    }
    Ok(parsed)
}

/// Stringify a parsed URL with its `userinfo` replaced by `***`.
/// Used in error messages so an operator can see which URL was
/// rejected without leaking the `user:password` to logs.
fn redact_url_userinfo(parsed: &url::Url) -> String {
    let mut redacted = parsed.clone();
    let _ = redacted.set_username("***");
    let _ = redacted.set_password(None);
    redacted.to_string()
}

/// Resolve a `source.path` value against a package metadata URL.
///
/// Relative paths are joined to `package_url` using RFC 3986 rules —
/// `..` segments work as expected. Absolute and scheme-relative URLs
/// are accepted only when their final resolved URL stays on the same
/// origin as the package metadata URL and carries no `userinfo`.
pub(crate) fn resolve_source_url(
    package_url: &url::Url,
    raw: &str,
) -> Result<String, IndexHttpError> {
    let resolved = package_url
        .join(raw)
        .map_err(|err| IndexHttpError::InvalidMetadata {
            name: "<source>".to_owned(),
            message: format!(
                "cannot resolve {:?} against {}: {err}",
                redact_raw_url_userinfo(raw),
                redact_url_userinfo(package_url)
            ),
        })?;
    validate_source_url(package_url, &resolved)?;
    Ok(resolved.into())
}

fn validate_source_url(package_url: &url::Url, resolved: &url::Url) -> Result<(), IndexHttpError> {
    match resolved.scheme() {
        "http" | "https" => {}
        other => {
            return Err(IndexHttpError::InvalidMetadata {
                name: "<source>".to_owned(),
                message: format!("source URL uses unsupported scheme {other:?}"),
            });
        }
    }

    if !resolved.username().is_empty() || resolved.password().is_some() {
        return Err(IndexHttpError::InvalidMetadata {
            name: "<source>".to_owned(),
            message: format!(
                "source URL `{}` must not contain credentials (userinfo)",
                redact_url_userinfo(resolved)
            ),
        });
    }

    if !same_origin(package_url, resolved) {
        return Err(IndexHttpError::InvalidMetadata {
            name: "<source>".to_owned(),
            message: format!(
                "source URL `{}` must have the same origin as package metadata URL `{}`",
                redact_url_userinfo(resolved),
                redact_url_userinfo(package_url)
            ),
        });
    }

    Ok(())
}

fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn redact_raw_url_userinfo(raw: &str) -> String {
    let authority_start = if raw.starts_with("//") {
        2
    } else if let Some(pos) = raw.find("://") {
        pos + 3
    } else {
        return raw.to_owned();
    };
    let authority_end = raw[authority_start..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |pos| authority_start + pos);
    let authority = &raw[authority_start..authority_end];
    let Some(at) = authority.rfind('@') else {
        return raw.to_owned();
    };
    format!(
        "{}***@{}{}",
        &raw[..authority_start],
        &authority[at + 1..],
        &raw[authority_end..]
    )
}

/// Build a closure suitable for [`SourceContext::HttpUrl`].
fn make_source_resolver(package_url: url::Url) -> impl Fn(&str) -> Result<String, IndexError> {
    move |raw: &str| {
        resolve_source_url(&package_url, raw).map_err(|err| IndexError::InvalidPackageName {
            // `IndexError` does not have a dedicated variant for
            // bad source URLs; reuse `InvalidPackageName`-shape
            // wording so the caller gets a clear message. The
            // `IndexHttpError` wrapper at the source.rs boundary
            // re-classifies anything left over.
            package: "<source>".to_owned(),
            message: err.to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryConfig {
    schema: u32,
    kind: String,
    packages: String,
    artifacts: String,
}

impl HttpIndexConfig {
    fn from_raw(raw: RawRegistryConfig, base: &url::Url) -> Result<Self, IndexHttpError> {
        if raw.schema != REGISTRY_CONFIG_SCHEMA {
            return Err(IndexHttpError::InvalidConfig {
                base_url: base.to_string(),
                message: format!(
                    "unsupported schema version {} (expected {REGISTRY_CONFIG_SCHEMA})",
                    raw.schema
                ),
            });
        }
        if raw.kind != REGISTRY_KIND {
            return Err(IndexHttpError::InvalidConfig {
                base_url: base.to_string(),
                message: format!(
                    "unsupported kind {:?} (expected {REGISTRY_KIND:?})",
                    raw.kind
                ),
            });
        }
        validate_subdir(base, "packages", &raw.packages)?;
        validate_subdir(base, "artifacts", &raw.artifacts)?;
        Ok(Self {
            schema: raw.schema,
            kind: raw.kind,
            packages: raw.packages,
            artifacts: raw.artifacts,
        })
    }
}

fn validate_subdir(base: &url::Url, field: &str, value: &str) -> Result<(), IndexHttpError> {
    if !relative_subdir_is_safe(value) {
        return Err(IndexHttpError::InvalidConfig {
            base_url: base.to_string(),
            message: format!("{field} must be a relative subdirectory, not {value:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn parse_base_url_normalizes_trailing_slash() {
        let with = parse_base_url("http://localhost:8080/registry/").unwrap();
        let without = parse_base_url("http://localhost:8080/registry").unwrap();
        assert_eq!(with.as_str(), without.as_str());
        assert!(with.as_str().ends_with('/'));
    }

    #[test]
    fn parse_base_url_rejects_unsupported_scheme() {
        let err = parse_base_url("file:///tmp").unwrap_err();
        match err {
            IndexHttpError::InvalidUrl { message, .. } => {
                assert!(message.contains("file"));
            }
            other => panic!("expected InvalidUrl, got {other:?}"),
        }
    }

    #[test]
    fn parse_base_url_rejects_garbage() {
        let err = parse_base_url("not a url").unwrap_err();
        assert!(matches!(err, IndexHttpError::InvalidUrl { .. }));
    }

    #[test]
    fn parse_base_url_rejects_credentials_in_url() {
        let err = parse_base_url("https://user:pw@example.com/index/").unwrap_err();
        match err {
            IndexHttpError::InvalidUrl { url, message } => {
                assert!(
                    !url.contains("user:pw") && !message.contains("user:pw"),
                    "credentials must be redacted; url={url:?}, message={message:?}"
                );
                assert!(
                    message.contains("credentials") || message.contains("userinfo"),
                    "error message should mention credentials, got {message:?}"
                );
            }
            other => panic!("expected InvalidUrl, got {other:?}"),
        }
    }

    #[test]
    fn resolve_source_url_handles_relative_dot_dot() {
        let pkg = package_url("http://localhost:8080/registry/packages/fmt.json");
        let resolved = resolve_source_url(&pkg, "../artifacts/fmt/fmt-10.2.1.tar.gz").unwrap();
        assert_eq!(
            resolved,
            "http://localhost:8080/registry/artifacts/fmt/fmt-10.2.1.tar.gz"
        );
    }

    #[test]
    fn resolve_source_url_accepts_same_origin_absolute_url() {
        let pkg = package_url("http://localhost:8080/registry/packages/fmt.json");
        let absolute = "http://localhost:8080/registry/artifacts/fmt/fmt-10.2.1.tar.gz";
        let resolved = resolve_source_url(&pkg, absolute).unwrap();
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn resolve_source_url_rejects_cross_origin_absolute_url() {
        let pkg = package_url("https://registry.example.com/registry/packages/fmt.json");
        let err = resolve_source_url(&pkg, "http://127.0.0.1/artifacts/fmt.tar.gz").unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("same origin"),
            "expected same-origin rejection, got {message:?}"
        );
    }

    #[test]
    fn resolve_source_url_rejects_scheme_relative_cross_origin_url() {
        let pkg = package_url("https://registry.example.com/registry/packages/fmt.json");
        let err = resolve_source_url(&pkg, "//evil.example.net/artifacts/fmt.tar.gz").unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("same origin"),
            "expected same-origin rejection, got {message:?}"
        );
    }

    #[test]
    fn resolve_source_url_rejects_userinfo_in_absolute_url() {
        let pkg = package_url("https://registry.example.com/registry/packages/fmt.json");
        let err = resolve_source_url(
            &pkg,
            "https://user:pw@registry.example.com/registry/artifacts/fmt.tar.gz",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted from error, got {message:?}"
        );
        assert!(
            message.contains("credentials") || message.contains("userinfo"),
            "expected credentials rejection, got {message:?}"
        );
    }

    #[test]
    fn resolve_source_url_rejects_garbage_absolute_url() {
        let pkg = package_url("http://localhost/registry/packages/fmt.json");
        let err = resolve_source_url(&pkg, "https://[::not::a::url::").unwrap_err();
        assert!(matches!(err, IndexHttpError::InvalidMetadata { .. }));
    }

    #[test]
    fn resolve_source_url_redacts_userinfo_when_resolution_fails() {
        let pkg = package_url("https://registry.example.com/registry/packages/fmt.json");
        let err = resolve_source_url(&pkg, "https://user:pw@[::not::a::url::").unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains("user:pw"),
            "credentials must be redacted from error, got {message:?}"
        );
    }

    #[test]
    fn validate_subdir_rejects_traversal() {
        let base = url::Url::parse("http://localhost/").unwrap();
        let err = validate_subdir(&base, "packages", "../escape").unwrap_err();
        assert!(matches!(err, IndexHttpError::InvalidConfig { .. }));
    }

    // HTTP URL boundary path-safety.

    #[test]
    fn ensure_path_safe_rejects_traversal() {
        let err = ensure_path_safe("../evil").unwrap_err();
        match err {
            IndexHttpError::UnsafePackageName { name } => assert_eq!(name, "../evil"),
            other => panic!("expected UnsafePackageName, got {other:?}"),
        }
    }

    #[test]
    fn ensure_path_safe_rejects_path_separator() {
        let err = ensure_path_safe("foo/bar").unwrap_err();
        match err {
            IndexHttpError::UnsafePackageName { name } => assert_eq!(name, "foo/bar"),
            other => panic!("expected UnsafePackageName, got {other:?}"),
        }
    }

    #[test]
    fn ensure_path_safe_rejects_leading_dot() {
        let err = ensure_path_safe(".hidden").unwrap_err();
        assert!(matches!(err, IndexHttpError::UnsafePackageName { .. }));
    }

    /// A leading `-` is rejected so the name cannot be parsed as
    /// a flag by any argv-driven tool the planner threads it into
    /// (`pkg-config`, the linker, `clap` short-option splitting).
    /// The check lives in `cabin-core::is_path_safe_package_name`,
    /// so this is the boundary regression test that pins the
    /// behavior at the sparse-HTTP fetch entry too.
    #[test]
    fn ensure_path_safe_rejects_leading_dash() {
        for raw in ["-foo", "--list-all", "-Lfoo"] {
            let err = ensure_path_safe(raw).unwrap_err();
            assert!(
                matches!(err, IndexHttpError::UnsafePackageName { .. }),
                "{raw:?} should produce UnsafePackageName, got {err:?}"
            );
        }
    }

    #[test]
    fn ensure_path_safe_rejects_drive_prefix() {
        let err = ensure_path_safe("C:foo").unwrap_err();
        assert!(matches!(err, IndexHttpError::UnsafePackageName { .. }));
    }

    #[test]
    fn ensure_path_safe_accepts_simple_name() {
        ensure_path_safe("fmt").unwrap();
        ensure_path_safe("rust_core").unwrap();
        ensure_path_safe("foo-bar-baz").unwrap();
        // `..` substrings inside an otherwise safe name are
        // accepted because no path resolver interprets them as a
        // parent reference.
        ensure_path_safe("foo..bar").unwrap();
    }

    #[test]
    fn package_url_built_for_safe_name() {
        // Build a HttpIndex by hand so the test does not need a
        // running server.
        let base = url::Url::parse("http://localhost/registry/").unwrap();
        let packages_base = url::Url::parse("http://localhost/registry/packages/").unwrap();
        let idx = HttpIndex {
            base,
            packages_base,
            client: HttpClient::new(),
        };
        let url = idx.package_url("fmt").unwrap();
        assert_eq!(url.as_str(), "http://localhost/registry/packages/fmt.json");
    }

    // -----------------------------------------------------------------
    // URL-reserved characters in package names must be
    // rejected at the same boundary so they never reach
    // `Url::join`. This is the regression test the spec calls out
    // explicitly: "Sparse HTTP package lookup must not call
    // url::join with an unsafe raw package name."
    // -----------------------------------------------------------------

    #[test]
    fn ensure_path_safe_rejects_url_reserved_chars() {
        for raw in [
            "foo?bar",   // `?` would split the path from a query string
            "foo#bar",   // `#` would start a fragment
            "foo%2Fbar", // pre-encoded `/` — must not bypass the gate
            "foo:bar",   // `:` confuses scheme detection
            "foo&bar",
            "foo=bar",
            "foo+bar",
            "foo@bar",
        ] {
            let err = ensure_path_safe(raw).unwrap_err();
            assert!(
                matches!(err, IndexHttpError::UnsafePackageName { .. }),
                "{raw:?} should produce UnsafePackageName, got {err:?}"
            );
        }
    }

    #[test]
    fn package_name_constructor_rejects_url_reserved() {
        // The structural reason `Url::join` cannot be reached: every
        // call site funnels names through `PackageName::new`, which
        // applies the same grammar as `ensure_path_safe`. Confirm
        // that path here so a future refactor cannot quietly weaken
        // the upstream validation while leaving the downstream
        // checks intact.
        for raw in ["foo?bar", "foo#bar", "foo%2Fbar", "foo:bar"] {
            assert!(
                PackageName::new(raw).is_err(),
                "PackageName::new({raw:?}) should fail at construction"
            );
        }
    }
}
