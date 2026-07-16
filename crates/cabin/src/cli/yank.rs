//! `cabin yank`: set or clear a published version's yanked flag on a
//! remote registry (`-Z remote-registry`).
//!
//! Yanking only flips the `yanked` marker in the per-package index
//! document: the version disappears from *new* resolution (the
//! resolver already skips yanked versions), but the archive stays
//! downloadable so existing lockfiles keep building.  The registry
//! resolves like the other remote-registry commands (`--index-url`,
//! else the `[registry] index-url` config setting), and the request
//! goes to the API origin the registry's `config.json` declares,
//! through `cabin-registry-api`.

use anyhow::{Result, bail};
use clap::Args;

use cabin_core::{ExperimentalFeature, ExperimentalFeatures, PackageName};

use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct YankArgs {
    /// Package to yank, as `<scope>/<name>@<version>` with an exact
    /// `SemVer` version (no ranges).
    #[arg(value_name = "SPEC")]
    pub spec: String,

    /// Un-yank: clear the version's yanked flag instead of setting it.
    #[arg(long)]
    pub undo: bool,

    /// Sparse HTTP index URL of the registry.  Falls back to the
    /// `[registry] index-url` config setting.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,
}

pub(crate) fn yank(
    args: &YankArgs,
    reporter: Reporter,
    features: &ExperimentalFeatures,
) -> Result<()> {
    if !features.is_enabled(ExperimentalFeature::RemoteRegistry) {
        bail!(cabin_core::registry::remote_registry_field_error(
            "cabin yank"
        ));
    }
    let (name, version) = parse_spec(&args.spec)?;
    // Registry packages are always `<scope>/<name>`: fail a bare name
    // here, before credentials, config.json reads, or the API call.
    if !name.is_scoped() {
        bail!(
            "registry packages must be named `<scope>/<name>`, but `{}` is a bare name; yank the \
             package under its full scoped name, e.g. `<scope>/{}@{version}`",
            name.as_str(),
            name.as_str()
        );
    }
    let index_url = crate::cli::login::effective_registry_index_url(
        args.index_url.as_deref(),
        features,
        "cabin yank",
        "yanked state lives in the remote registry's index",
    )?;

    // Mirror the remote publish flow: one credential lookup serves
    // the config.json read and the API call alike.
    let origin = cabin_credentials::normalize_origin(&index_url)?;
    let lookup = cabin_credentials::lookup_token(&origin)?;
    if let Some(warning) = lookup.permissions_warning {
        reporter.warning(format_args!("{warning}"));
    }
    let token = lookup.token;
    let mut client = cabin_index_http::HttpClient::new();
    if let Some(token) = token.clone() {
        client = client.with_auth(cabin_index_http::RegistryAuth::for_index_url(
            &index_url, token,
        )?);
    }
    let index = cabin_index_http::HttpIndex::open_with_features(&index_url, client, features)?;
    let Some(api) = index.api() else {
        bail!(
            "registry `{origin}` does not declare an `api` URL in its config.json; yanking needs \
             one to locate the registry API origin"
        );
    };

    let api_client = cabin_registry_api::RegistryApi::new(api, token)?;
    api_client.set_yanked(name.as_str(), &version, !args.undo)?;
    // The route is idempotent (a 200 whether or not the state
    // changed), so the report states the resulting state - correct
    // for a fresh yank and for a no-op alike.
    let state = if args.undo {
        "no longer yanked"
    } else {
        "now yanked"
    };
    println!("{}@{version} is {state}", name.as_str());
    Ok(())
}

/// Parse the strict `<name>@<version>` spec: a valid package name and
/// an exact `SemVer` version.  Ranges, requirements, and a missing
/// version are rejected.
fn parse_spec(spec: &str) -> Result<(PackageName, semver::Version)> {
    let Some((name, version)) = spec.split_once('@') else {
        bail!(
            "invalid package spec `{spec}`: expected `<name>@<version>` with an exact SemVer \
             version, e.g. `fmtlib/fmt@10.2.1`"
        );
    };
    let name = PackageName::new(name)
        .map_err(|err| anyhow::anyhow!("invalid package spec `{spec}`: {err}"))?;
    let version = semver::Version::parse(version).map_err(|err| {
        anyhow::anyhow!(
            "invalid package spec `{spec}`: `{version}` is not an exact SemVer version ({err}); \
             version ranges are not accepted"
        )
    })?;
    Ok((name, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spec_accepts_exact_versions() {
        let (name, version) = parse_spec("fmt@10.2.1").unwrap();
        assert_eq!(name.as_str(), "fmt");
        assert_eq!(version, semver::Version::new(10, 2, 1));

        let (name, version) = parse_spec("my-lib@0.1.0-alpha.1").unwrap();
        assert_eq!(name.as_str(), "my-lib");
        assert_eq!(version.to_string(), "0.1.0-alpha.1");

        // A scoped name's `/` sits before the `@`, so the split is
        // unambiguous.
        let (name, version) = parse_spec("fmtlib/fmt@10.2.1").unwrap();
        assert_eq!(name.as_str(), "fmtlib/fmt");
        assert_eq!(version, semver::Version::new(10, 2, 1));
    }

    #[test]
    fn parse_spec_rejects_missing_or_inexact_versions() {
        for (spec, expected) in [
            ("fmt", "expected `<name>@<version>`"),
            ("fmt@", "is not an exact SemVer version"),
            ("fmt@banana", "is not an exact SemVer version"),
            ("fmt@^10.2.1", "is not an exact SemVer version"),
            ("fmt@10.2", "is not an exact SemVer version"),
            ("@10.2.1", "package name must not be empty"),
            ("../evil@10.2.1", "invalid package spec"),
        ] {
            let message = parse_spec(spec).unwrap_err().to_string();
            assert!(
                message.contains(expected) && message.contains("invalid package spec"),
                "{spec}: {message}"
            );
        }
    }
}
