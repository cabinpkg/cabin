# Foundation Ports + zlib Milestone Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a minimal root-level foundation-ports mechanism and use it to build zlib as Cabin's first external C library, with a downstream Cabin consumer that links zlib and calls `zlibVersion()`.

**Architecture:** A foundation port is a curated recipe (`port.toml`) that pins an upstream source archive by SHA-256 plus an overlay `cabin.toml` describing zlib's source layout as a Cabin C static library. After preparation (fetch → verify → safe-extract with `strip_prefix` → overlay copy) the prepared directory is a valid Cabin path dependency: the existing workspace loader, build planner, and Ninja backend take over unchanged. A new `cabin-port` crate owns the recipe parser, cache layout, and preparation pipeline; `cabin-artifact`'s existing extraction is widened (not duplicated) with a `strip_prefix` option so the decompression-bomb, symlink-rejection, and path-safety rules stay in one place. The CLI orchestrates HTTP via the existing `cabin-index-http::HttpClient` and hands prepared port sources to the workspace loader, mirroring how registry packages are pre-resolved today.

**Tech Stack:** Rust 2024 (workspace edition), `sha2`, `flate2`, `tar`, `toml`, `serde`, `semver`, `url`, `ureq` (via `cabin-index-http`). Tests use `tiny_http` for hermetic mock servers.

---

## File Structure

**New files**
- `crates/cabin-port/Cargo.toml`
- `crates/cabin-port/src/lib.rs` — public re-exports + crate docs
- `crates/cabin-port/src/model.rs` — `PortDescriptor`, `PortSource`, `PortChecksum`, `OverlayManifest`
- `crates/cabin-port/src/parse.rs` — `port.toml` parser; private `Raw*` serde structs converted to typed `PortDescriptor`
- `crates/cabin-port/src/cache.rs` — checksum-addressed cache layout
- `crates/cabin-port/src/prepare.rs` — `PortPlan`, `PortFetchSource`, `prepare`, overlay copy, name/version cross-check
- `crates/cabin-port/src/error.rs`
- `crates/cabin-cli/src/port_glue.rs` — discovery + HTTP + workspace-source stitching
- `ports/README.md`
- `ports/zlib/port.toml`
- `ports/zlib/cabin.toml`
- `docs/foundation-ports.md`

**Modified files**
- `Cargo.toml` — register `cabin-port` workspace member + dep
- `crates/cabin-artifact/src/extract.rs` — make extraction reusable via `strip_prefix`
- `crates/cabin-artifact/src/lib.rs` — re-export `safe_extract` (or equivalent name)
- `crates/cabin-core/src/model.rs` — `DependencySource::Port { path }` variant + provenance helpers
- `crates/cabin-manifest/src/raw.rs` — `port: Option<String>` field on `RawDependencyTable`
- `crates/cabin-manifest/src/parse.rs` — port form rules + mutex with other dep forms
- `crates/cabin-manifest/src/error.rs` — new variants for port-dep validation
- `crates/cabin-workspace/src/loader.rs` — `PortPackageSource` + resolution of `DependencySource::Port`
- `crates/cabin-workspace/src/lib.rs` — export
- `crates/cabin-cli/src/cli.rs` — call `port_glue::prepare_ports` from build / run / test / metadata orchestration paths
- `crates/cabin-cli/src/metadata_glue.rs` — surface port provenance in metadata output
- `crates/cabin-cli/Cargo.toml` — depend on `cabin-port`
- `crates/cabin-cli/tests/cli.rs` — integration test (mock HTTP fake-zlib + downstream consumer)
- `docs/architecture.md` — note `cabin-port` crate + zlib milestone
- `docs/manifest.md` — `port = "..."` dependency form

---

## Design Invariants

1. `port.toml` is the *authoritative* identity. The overlay `cabin.toml`'s `[package]` name/version must match; mismatch is a clear hard error at preparation time.
2. SHA-256 is mandatory. There is no way to opt out. Missing → parse error. Mismatch → preparation error showing expected vs actual.
3. Only `[source].type = "archive"` is supported. `git`, `tag`, `branch`, `latest` all produce explicit "foundation ports require pinned archive sources with SHA-256" errors.
4. Extraction security stays in `cabin-artifact`. `cabin-port` does NOT reimplement decompression-bomb / symlink / path-safety logic.
5. After preparation the port directory contains the overlay `cabin.toml` at its root and looks exactly like a Cabin path dependency.
6. The workspace loader never opens the network. HTTP lives in `cabin-cli` orchestration via `cabin-index-http::HttpClient`.
7. No build scripts, options, variants, tool-deps, custom commands, or upstream build-system invocation are introduced.
8. The lockfile and resolver are not extended; ports are pinned in `port.toml` and consumed as path-like deps.

---

## Schema Reference

`port.toml` (authored by hand under `ports/<name>/`):

```toml
[port]
name = "zlib"
version = "1.3.1"
description = "Compression library"
license = "Zlib"
homepage = "https://zlib.net/"
upstream = "https://github.com/madler/zlib"

[source]
type = "archive"
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
strip_prefix = "zlib-1.3.1"

[overlay]
manifest = "cabin.toml"
```

`cabin.toml` overlay (authored by hand under `ports/zlib/`):

```toml
[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "cpp_library"
sources = [
    "adler32.c",
    "compress.c",
    "crc32.c",
    "deflate.c",
    "gzclose.c",
    "gzlib.c",
    "gzread.c",
    "gzwrite.c",
    "infback.c",
    "inffast.c",
    "inflate.c",
    "inftrees.c",
    "trees.c",
    "uncompr.c",
    "zutil.c",
]
include_dirs = ["."]
```

Cabin downstream dependency form:

```toml
[dependencies]
zlib = { port = "../../ports/zlib" }
```

The `port` value is a path (relative to the manifest containing it) that names a directory with `port.toml` and the overlay `cabin.toml`. It is mutually exclusive with `path`, `version`, `workspace`, and `system`.

---

## Task 1: Bootstrap `cabin-port` crate skeleton

**Files:**
- Create: `crates/cabin-port/Cargo.toml`
- Create: `crates/cabin-port/src/lib.rs`
- Create: `crates/cabin-port/src/model.rs`
- Create: `crates/cabin-port/src/error.rs`
- Modify: `Cargo.toml` (root) — add to `members` and `[workspace.dependencies]`

- [ ] **Step 1: Write the Cargo.toml**

`crates/cabin-port/Cargo.toml`:

```toml
[package]
name = "cabin-port"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
description = "Foundation-port recipe parser and source-preparation pipeline for Cabin."

[dependencies]
cabin-artifact = { workspace = true }
cabin-core = { workspace = true }
cabin-manifest = { workspace = true }
semver = { workspace = true }
serde = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
toml = { workspace = true }
url = { workspace = true }

[dev-dependencies]
flate2 = { workspace = true }
tar = { workspace = true }
tempfile = "3"

[lints]
workspace = true
```

Add to root `Cargo.toml` members list (alphabetical): `"crates/cabin-port",`. Add to `[workspace.dependencies]`: `cabin-port = { path = "crates/cabin-port" }`.

- [ ] **Step 2: Write lib.rs**

```rust
//! Foundation-port recipe layer for Cabin.
//!
//! A foundation port is a curated recipe (`port.toml`) that names
//! an upstream source archive, pins it by SHA-256, and ships an
//! overlay `cabin.toml` describing the upstream sources as a
//! Cabin C/C++ target. This crate owns:
//!
//! - the `port.toml` schema and parser ([`mod@parse`]),
//! - the typed [`PortDescriptor`] / [`PortSource`] model ([`model`]),
//! - the source-preparation pipeline ([`prepare`]).
//!
//! Crate boundaries:
//! - this crate must not perform HTTP — the caller (the
//!   CLI orchestration layer) downloads archive bytes and
//!   passes them in as [`PortFetchSource::InMemoryArchive`];
//! - this crate must not call the resolver, the workspace
//!   loader, or the build planner;
//! - extraction safety (decompression bomb caps, symlink
//!   rejection, path-traversal protection) is delegated to
//!   `cabin-artifact::safe_extract`.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod cache;
pub mod error;
pub mod model;
pub mod parse;
pub mod prepare;

pub use cache::PortCache;
pub use error::PortError;
pub use model::{
    OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource,
};
pub use parse::{load_port, parse_port_str};
pub use prepare::{
    PortFetchSource, PortPlan, PortPrepareOptions, PortPrepareResult, PreparedPort, prepare,
};
```

- [ ] **Step 3: Write the typed model (`model.rs`)**

```rust
use std::path::PathBuf;

use cabin_core::PackageName;
use semver::Version;
use url::Url;

/// Validated `port.toml` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDescriptor {
    /// Authoritative package identity. The overlay manifest's
    /// `[package]` must match these values; mismatches surface
    /// at preparation time.
    pub name: PackageName,
    pub version: Version,
    pub metadata: PortMetadata,
    pub source: PortSource,
    pub overlay: OverlayManifest,
}

/// Optional human-facing fields. Always present in the struct
/// (with `None` defaults) so callers can render metadata
/// uniformly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortMetadata {
    pub description: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<Url>,
    pub upstream: Option<Url>,
}

/// Where the port's upstream bytes come from. Only the
/// pinned-archive shape is supported; every other form
/// (git, tag-only, branch, "latest") is rejected by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSource {
    Archive {
        url: Url,
        sha256: PortChecksum,
        /// Directory prefix to strip from every archive entry
        /// before joining into the destination. `None` means
        /// the archive root is the destination root.
        strip_prefix: Option<String>,
    },
}

/// SHA-256 digest of a port's source archive. Stored as 32
/// validated bytes; render with [`PortChecksum::to_hex`] /
/// [`PortChecksum::to_sha256_string`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortChecksum([u8; 32]);

impl PortChecksum {
    /// Parse a 64-character lowercase hex digest. Anything
    /// else (wrong length, non-hex characters, upper-case)
    /// is rejected.
    pub fn parse_hex(value: &str) -> Option<Self> {
        if value.len() != 64 || !value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = hex_value(value.as_bytes()[i * 2])?;
            let lo = hex_value(value.as_bytes()[i * 2 + 1])?;
            *byte = (hi << 4) | lo;
        }
        Some(Self(out))
    }

    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.0 {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }

    pub fn to_sha256_string(self) -> String {
        format!("sha256:{}", self.to_hex())
    }

    pub fn bytes(self) -> [u8; 32] {
        self.0
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(10 + byte - b'a'),
        _ => None,
    }
}

/// Overlay manifest pointer. The `path` is a relative path
/// inside the port directory; absolute paths and `..`
/// components are rejected by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayManifest {
    pub relative_path: PathBuf,
}
```

- [ ] **Step 4: Write `error.rs`**

```rust
use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PortError {
    #[error("failed to read port descriptor at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse port descriptor at {}: {source}", path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error(
        "port descriptor at {} declares unsupported source type `{kind}`; foundation ports require a pinned archive source with SHA-256",
        path.display()
    )]
    UnsupportedSourceType { path: PathBuf, kind: String },

    #[error(
        "port descriptor at {} is missing `[source].sha256`; foundation ports require a 64-character lowercase hex SHA-256",
        path.display()
    )]
    MissingChecksum { path: PathBuf },

    #[error(
        "port descriptor at {} declares an invalid SHA-256 ({value:?}); expected 64 lowercase hex characters",
        path.display()
    )]
    InvalidChecksum { path: PathBuf, value: String },

    #[error(
        "port descriptor at {} declares an invalid `{field}` URL ({value:?}): {message}",
        path.display()
    )]
    InvalidUrl {
        path: PathBuf,
        field: &'static str,
        value: String,
        message: String,
    },

    #[error("port descriptor at {} declares an invalid `{field}`: {message}", path.display())]
    InvalidField {
        path: PathBuf,
        field: &'static str,
        message: String,
    },

    #[error(
        "port descriptor at {} declares an unsafe overlay manifest path `{value}`; expected a relative path inside the port directory",
        path.display()
    )]
    UnsafeOverlayPath { path: PathBuf, value: String },

    #[error(
        "checksum mismatch for port `{name} {version}`: expected sha256:{expected}, got sha256:{actual}"
    )]
    ChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
    },

    #[error(
        "source archive for port `{name} {version}` does not contain the declared strip_prefix directory `{strip_prefix}`"
    )]
    MissingStripPrefix {
        name: String,
        version: String,
        strip_prefix: String,
    },

    #[error("overlay manifest for port `{name} {version}` was not found at {}", path.display())]
    MissingOverlayManifest {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error(
        "overlay manifest for port `{name} {version}` declares package `{actual_name} {actual_version}`; expected to match the port identity"
    )]
    OverlayIdentityMismatch {
        name: String,
        version: String,
        actual_name: String,
        actual_version: String,
    },

    #[error("source archive for port `{name} {version}` does not exist: {}", path.display())]
    MissingArchive {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error("failed to parse overlay manifest for port `{name} {version}`: {source}")]
    OverlayManifestParse {
        name: String,
        version: String,
        #[source]
        source: Box<cabin_manifest::ManifestError>,
    },

    #[error("failed to extract port `{name} {version}` archive: {source}")]
    Extract {
        name: String,
        version: String,
        #[source]
        source: Box<cabin_artifact::ArtifactError>,
    },

    #[error("filesystem error at {}: {source}", path.display())]
    Fs {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
```

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p cabin-port`
Expected: PASS (compilation succeeds; no tests defined yet).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/cabin-port
git commit -m "feat(cabin-port): scaffold foundation-port crate"
```

---

## Task 2: Parser for `port.toml`

**Files:**
- Create: `crates/cabin-port/src/parse.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/cabin-port/src/parse.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ZLIB_PORT: &str = r#"
[port]
name = "zlib"
version = "1.3.1"
description = "Compression library"
license = "Zlib"
homepage = "https://zlib.net/"
upstream = "https://github.com/madler/zlib"

[source]
type = "archive"
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
strip_prefix = "zlib-1.3.1"

[overlay]
manifest = "cabin.toml"
"#;

    fn parse(text: &str) -> Result<PortDescriptor, PortError> {
        parse_port_str(text, Path::new("port.toml"))
    }

    #[test]
    fn parses_zlib_port() {
        let port = parse(ZLIB_PORT).unwrap();
        assert_eq!(port.name.as_str(), "zlib");
        assert_eq!(port.version, semver::Version::new(1, 3, 1));
        match &port.source {
            PortSource::Archive { url, sha256, strip_prefix } => {
                assert_eq!(url.as_str(), "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz");
                assert_eq!(
                    sha256.to_hex(),
                    "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
                );
                assert_eq!(strip_prefix.as_deref(), Some("zlib-1.3.1"));
            }
        }
        assert_eq!(port.overlay.relative_path, PathBuf::from("cabin.toml"));
        assert_eq!(port.metadata.description.as_deref(), Some("Compression library"));
    }

    #[test]
    fn rejects_missing_sha256() {
        let text = ZLIB_PORT.replace(
            "sha256 = \"9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23\"\n",
            "",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::MissingChecksum { .. }), "{err:?}");
    }

    #[test]
    fn rejects_invalid_sha256_length() {
        let text = ZLIB_PORT.replace(
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "deadbeef",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidChecksum { .. }), "{err:?}");
    }

    #[test]
    fn rejects_uppercase_sha256() {
        let text = ZLIB_PORT.replace(
            "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            "9A93B2B7DFDAC77CEBA5A558A580E74667DD6FEDE4585B91EEFB60F03B72DF23",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidChecksum { .. }), "{err:?}");
    }

    #[test]
    fn rejects_unsupported_source_type_git() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"git\"");
        let err = parse(&text).unwrap_err();
        match err {
            PortError::UnsupportedSourceType { kind, .. } => assert_eq!(kind, "git"),
            other => panic!("expected UnsupportedSourceType, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_source_type_branch() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"branch\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsupportedSourceType { .. }), "{err:?}");
    }

    #[test]
    fn rejects_unsupported_source_type_latest() {
        let text = ZLIB_PORT.replace("type = \"archive\"", "type = \"latest\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsupportedSourceType { .. }), "{err:?}");
    }

    #[test]
    fn rejects_absolute_overlay_path() {
        let text = ZLIB_PORT.replace("manifest = \"cabin.toml\"", "manifest = \"/etc/passwd\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsafeOverlayPath { .. }), "{err:?}");
    }

    #[test]
    fn rejects_parent_dir_overlay_path() {
        let text = ZLIB_PORT.replace("manifest = \"cabin.toml\"", "manifest = \"../cabin.toml\"");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::UnsafeOverlayPath { .. }), "{err:?}");
    }

    #[test]
    fn rejects_invalid_url() {
        let text = ZLIB_PORT.replace(
            "url = \"https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz\"",
            "url = \"::not a url::\"",
        );
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, PortError::InvalidUrl { field: "url", .. }), "{err:?}");
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = format!("{ZLIB_PORT}\n[extras]\nsomething = true\n");
        let err = parse(&text).unwrap_err();
        // Anything that surfaces as a TOML error is acceptable;
        // we only require the parser to reject unknown tables.
        assert!(matches!(err, PortError::Toml { .. }), "{err:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cabin-port -- parse::tests`
Expected: FAIL — module not implemented yet.

- [ ] **Step 3: Write the parser**

Replace `crates/cabin-port/src/parse.rs` body:

```rust
use std::path::{Component, Path, PathBuf};

use cabin_core::PackageName;
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::error::PortError;
use crate::model::{
    OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPort {
    port: RawPortIdentity,
    source: RawSource,
    overlay: RawOverlay,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPortIdentity {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    upstream: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    strip_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverlay {
    manifest: String,
}

/// Read and parse `port.toml` at `path`.
pub fn load_port(path: impl AsRef<Path>) -> Result<PortDescriptor, PortError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| PortError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_port_str(&text, path)
}

/// Parse `port.toml` contents. `path` is used for diagnostics only.
pub fn parse_port_str(text: &str, path: &Path) -> Result<PortDescriptor, PortError> {
    let raw: RawPort = toml::from_str(text).map_err(|source| PortError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    let RawPort { port, source, overlay } = raw;

    let name = PackageName::new(port.name.clone()).map_err(|err| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[port].name",
        message: err.to_string(),
    })?;
    let version = Version::parse(&port.version).map_err(|err| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[port].version",
        message: err.to_string(),
    })?;

    let metadata = PortMetadata {
        description: port.description,
        license: port.license,
        homepage: parse_optional_url(path, "homepage", port.homepage.as_deref())?,
        upstream: parse_optional_url(path, "upstream", port.upstream.as_deref())?,
    };

    let source = source_from_raw(path, source)?;
    let overlay = overlay_from_raw(path, overlay)?;

    Ok(PortDescriptor {
        name,
        version,
        metadata,
        source,
        overlay,
    })
}

fn source_from_raw(path: &Path, raw: RawSource) -> Result<PortSource, PortError> {
    if raw.kind != "archive" {
        return Err(PortError::UnsupportedSourceType {
            path: path.to_path_buf(),
            kind: raw.kind,
        });
    }
    let url_str = raw.url.ok_or_else(|| PortError::InvalidField {
        path: path.to_path_buf(),
        field: "[source].url",
        message: "expected a non-empty URL".to_owned(),
    })?;
    let url = Url::parse(&url_str).map_err(|err| PortError::InvalidUrl {
        path: path.to_path_buf(),
        field: "url",
        value: url_str,
        message: err.to_string(),
    })?;
    let raw_checksum = raw.sha256.ok_or_else(|| PortError::MissingChecksum {
        path: path.to_path_buf(),
    })?;
    let sha256 = PortChecksum::parse_hex(&raw_checksum).ok_or_else(|| PortError::InvalidChecksum {
        path: path.to_path_buf(),
        value: raw_checksum,
    })?;
    let strip_prefix = raw
        .strip_prefix
        .map(|s| {
            if s.is_empty() {
                return Err(PortError::InvalidField {
                    path: path.to_path_buf(),
                    field: "[source].strip_prefix",
                    message: "expected a non-empty prefix".to_owned(),
                });
            }
            if !is_safe_relative_segment(&s) {
                return Err(PortError::InvalidField {
                    path: path.to_path_buf(),
                    field: "[source].strip_prefix",
                    message: "expected a single non-empty relative path component"
                        .to_owned(),
                });
            }
            Ok(s)
        })
        .transpose()?;
    Ok(PortSource::Archive {
        url,
        sha256,
        strip_prefix,
    })
}

fn overlay_from_raw(path: &Path, raw: RawOverlay) -> Result<OverlayManifest, PortError> {
    let rel = PathBuf::from(&raw.manifest);
    if !is_safe_relative_path(&rel) {
        return Err(PortError::UnsafeOverlayPath {
            path: path.to_path_buf(),
            value: raw.manifest,
        });
    }
    Ok(OverlayManifest { relative_path: rel })
}

fn parse_optional_url(
    path: &Path,
    field: &'static str,
    raw: Option<&str>,
) -> Result<Option<Url>, PortError> {
    match raw {
        None => Ok(None),
        Some(value) => Url::parse(value)
            .map(Some)
            .map_err(|err| PortError::InvalidUrl {
                path: path.to_path_buf(),
                field,
                value: value.to_owned(),
                message: err.to_string(),
            }),
    }
}

fn is_safe_relative_path(rel: &Path) -> bool {
    if rel.as_os_str().is_empty() {
        return false;
    }
    if rel.is_absolute() {
        return false;
    }
    rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn is_safe_relative_segment(value: &str) -> bool {
    !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
        && !value.is_empty()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cabin-port -- parse::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cabin-port/src/parse.rs
git commit -m "feat(cabin-port): parse port.toml with strict source/overlay rules"
```

---

## Task 3: Cache layout

**Files:**
- Create: `crates/cabin-port/src/cache.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn paths_are_checksum_addressed() {
        let cache = PortCache::new("/cabin-cache/ports");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        assert_eq!(
            cache.archive_path(&hex),
            PathBuf::from(format!("/cabin-cache/ports/archives/sha256/{hex}.tar.gz"))
        );
        assert_eq!(
            cache.source_dir(&hex),
            PathBuf::from(format!("/cabin-cache/ports/sources/sha256/{hex}"))
        );
    }
}
```

- [ ] **Step 2: Implement `PortCache`**

```rust
use std::path::{Path, PathBuf};

/// Checksum-addressed cache for foundation-port archives and
/// their extracted source trees.
///
/// Layout:
///
/// ```text
/// <root>/
///   archives/sha256/<hex>.tar.gz
///   sources/sha256/<hex>/cabin.toml + upstream files
/// ```
#[derive(Debug, Clone)]
pub struct PortCache {
    root: PathBuf,
}

impl PortCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn archive_path(&self, hex: &str) -> PathBuf {
        self.root.join("archives").join("sha256").join(format!("{hex}.tar.gz"))
    }

    pub fn source_dir(&self, hex: &str) -> PathBuf {
        self.root.join("sources").join("sha256").join(hex)
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p cabin-port`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/cabin-port/src/cache.rs
git commit -m "feat(cabin-port): checksum-addressed cache layout"
```

---

## Task 4: Share `safe_extract` from `cabin-artifact` with `strip_prefix`

**Files:**
- Modify: `crates/cabin-artifact/src/extract.rs`
- Modify: `crates/cabin-artifact/src/lib.rs`
- Modify: `crates/cabin-artifact/src/error.rs`

The goal: cabin-port reuses the existing decompression-bomb caps, symlink rejection, and path-safety logic. Strip-prefix handling lives inside the safe extractor so the post-strip path is what we check, not the pre-strip one.

- [ ] **Step 1: Write the failing tests**

In `crates/cabin-artifact/src/extract.rs` `tests` module, add:

```rust
#[test]
fn strip_prefix_removes_leading_dir() {
    let dir = TempDir::new().unwrap();
    let archive = dir.path().join("zlib.tar.gz");
    make_archive(
        &archive,
        &[
            ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
            ("zlib-1.3.1/src/adler32.c", "int adler32(void) { return 0; }\n"),
        ],
    );
    let dest = dir.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    safe_extract_tar_gz(
        &archive,
        &dest,
        SafeExtractOptions { strip_prefix: Some("zlib-1.3.1") },
    )
    .unwrap();
    assert!(dest.join("zlib.h").is_file());
    assert!(dest.join("src/adler32.c").is_file());
    assert!(!dest.join("zlib-1.3.1").exists());
}

#[test]
fn strip_prefix_rejects_archive_without_matching_root() {
    let dir = TempDir::new().unwrap();
    let archive = dir.path().join("other.tar.gz");
    make_archive(&archive, &[("not-zlib/zlib.h", "// nope\n")]);
    let dest = dir.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let err = safe_extract_tar_gz(
        &archive,
        &dest,
        SafeExtractOptions { strip_prefix: Some("zlib-1.3.1") },
    )
    .unwrap_err();
    assert!(matches!(err, ArtifactError::MissingStripPrefix { .. }), "{err:?}");
}

#[test]
fn strip_prefix_keeps_path_safety_after_strip() {
    // Even if the archive's root dir is stripped, the
    // post-strip path must still pass `is_safe_relative_path`.
    let dir = TempDir::new().unwrap();
    let archive = dir.path().join("bad.tar.gz");
    make_archive_with_raw_name(
        &archive,
        "zlib-1.3.1/../escape.txt",
        tar::EntryType::Regular,
        None,
        b"evil",
    );
    let dest = dir.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let err = safe_extract_tar_gz(
        &archive,
        &dest,
        SafeExtractOptions { strip_prefix: Some("zlib-1.3.1") },
    )
    .unwrap_err();
    assert!(matches!(err, ArtifactError::UnsafeArchiveEntry(_)), "{err:?}");
}
```

- [ ] **Step 2: Verify they fail**

Run: `cargo test -p cabin-artifact -- extract::tests::strip_prefix`
Expected: FAIL — `safe_extract_tar_gz` and `SafeExtractOptions` not defined yet.

- [ ] **Step 3: Implement the new public API**

In `crates/cabin-artifact/src/extract.rs`:
- Add a new error variant `MissingStripPrefix { strip_prefix: String }` in `error.rs`.
- Add `pub fn safe_extract_tar_gz(archive: &Path, dest: &Path, opts: SafeExtractOptions<'_>) -> Result<(), ArtifactError>` that wraps the existing `extract_tar_gz_with_limits` and applies strip-prefix logic per entry. Implementation outline:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct SafeExtractOptions<'a> {
    /// If `Some`, every archive entry path must start with
    /// this single directory component; the component is
    /// stripped before the path is joined into `dest`.
    pub strip_prefix: Option<&'a str>,
}

pub fn safe_extract_tar_gz(
    archive: &Path,
    dest: &Path,
    opts: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    safe_extract_tar_gz_with_limits(
        archive,
        dest,
        MAX_ENTRY_BYTES,
        MAX_TOTAL_BYTES,
        MAX_ENTRIES,
        opts,
    )
}

fn safe_extract_tar_gz_with_limits(
    archive: &Path,
    dest: &Path,
    max_entry_bytes: u64,
    max_total_bytes: u64,
    max_entries: usize,
    opts: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    // Identical to extract_tar_gz_with_limits, except:
    //   1. Track `saw_prefix: bool` if strip_prefix is set.
    //   2. For each entry path, if strip_prefix is set, require
    //      the first component to equal the prefix; set saw_prefix
    //      = true and replace `entry_path` with the suffix
    //      (could be empty for the prefix dir itself — skip those).
    //   3. Re-run `is_safe_relative_path` on the post-strip path,
    //      to guard against `prefix/../escape`-style entries.
    //   4. After iteration, if strip_prefix was set and saw_prefix
    //      is still false, return ArtifactError::MissingStripPrefix.
    // ...
}
```

Refactor `extract_tar_gz` and `extract_tar_gz_with_limits` to delegate to `safe_extract_tar_gz_with_limits` with `SafeExtractOptions::default()`. Keep them `pub(crate)` so existing callers stay unchanged.

Add new error variant in `error.rs`:

```rust
#[error(
    "source archive does not contain the declared strip_prefix directory `{strip_prefix}`"
)]
MissingStripPrefix { strip_prefix: String },
```

- [ ] **Step 4: Expose in `lib.rs`**

```rust
pub use extract::{SafeExtractOptions, safe_extract_tar_gz};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p cabin-artifact`
Expected: PASS — existing extraction tests still pass; new strip_prefix tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cabin-artifact
git commit -m "feat(cabin-artifact): public safe_extract with strip_prefix"
```

---

## Task 5: Port preparation pipeline

**Files:**
- Create: `crates/cabin-port/src/prepare.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::PortCache;
    use crate::model::{OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource};
    use cabin_core::PackageName;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use semver::Version;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    /// Build a gzipped tarball with `entries` and return the path
    /// + lower-case hex SHA-256.
    fn make_archive(dir: &Path, name: &str, entries: &[(&str, &str)]) -> (PathBuf, String) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = fs::File::create(&path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel, body) in entries {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel, &mut std::io::Cursor::new(bytes))
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
        let bytes = fs::read(&path).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        (path, format!("{:x}", h.finalize()))
    }

    fn make_port(port_dir: &Path, archive_url: url::Url, sha256_hex: &str) -> PortDescriptor {
        fs::write(
            port_dir.join("cabin.toml"),
            "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"cpp_library\"\nsources = [\"zlib.c\"]\ninclude_dirs = [\".\"]\n",
        )
        .unwrap();
        PortDescriptor {
            name: pkg("zlib"),
            version: Version::new(1, 3, 1),
            metadata: PortMetadata::default(),
            source: PortSource::Archive {
                url: archive_url,
                sha256: PortChecksum::parse_hex(sha256_hex).unwrap(),
                strip_prefix: Some("zlib-1.3.1".to_owned()),
            },
            overlay: OverlayManifest { relative_path: PathBuf::from("cabin.toml") },
        }
    }

    #[test]
    fn prepares_port_from_local_archive() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                ("zlib-1.3.1/zlib.c", "int zlib_dummy(void) { return 0; }\n"),
            ],
        );
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = make_port(&port_dir, archive_url, &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor: port,
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);
        let prepared = &result.ports[0];
        assert_eq!(prepared.source_dir.parent().unwrap().file_name().unwrap(), hex.as_str());
        assert!(prepared.source_dir.join("cabin.toml").is_file());
        assert!(prepared.source_dir.join("zlib.h").is_file());
        assert!(prepared.source_dir.join("zlib.c").is_file());
    }

    #[test]
    fn reports_checksum_mismatch() {
        // build archive with hash A, declare hash B in the port
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        let (archive, _hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let bogus = "0".repeat(64);
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = make_port(&port_dir, archive_url, &bogus);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor: port,
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::ChecksumMismatch { expected, actual, .. } => {
                assert_eq!(expected, bogus);
                assert_ne!(actual, expected);
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn reports_missing_strip_prefix() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("other-1.0/zlib.h", "// nope\n")],
        );
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = make_port(&port_dir, archive_url, &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor: port,
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::MissingStripPrefix { .. }), "{err:?}");
    }

    #[test]
    fn reports_overlay_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        fs::write(
            port_dir.join("cabin.toml"),
            "[package]\nname = \"not-zlib\"\nversion = \"9.9.9\"\n\n[target.zlib]\ntype = \"cpp_library\"\nsources = [\"zlib.c\"]\n",
        )
        .unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = PortDescriptor {
            name: pkg("zlib"),
            version: Version::new(1, 3, 1),
            metadata: PortMetadata::default(),
            source: PortSource::Archive {
                url: archive_url,
                sha256: PortChecksum::parse_hex(&hex).unwrap(),
                strip_prefix: Some("zlib-1.3.1".to_owned()),
            },
            overlay: OverlayManifest { relative_path: PathBuf::from("cabin.toml") },
        };
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor: port,
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(dir.path().join("downloads/zlib.tar.gz")),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::OverlayIdentityMismatch { .. }), "{err:?}");
    }

    #[test]
    fn second_call_reuses_cached_prep() {
        // After a successful preparation, removing the archive on
        // disk must not break a re-run — the cache satisfies it.
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = make_port(&port_dir, archive_url, &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = || PortPlan {
            entries: vec![PortEntry {
                descriptor: port.clone(),
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(archive.clone()),
            }],
        };
        prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        fs::remove_file(&archive).unwrap();
        let r2 = prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        assert!(r2.ports[0].source_dir.join("cabin.toml").is_file());
    }

    #[test]
    fn frozen_fails_on_cache_miss() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        fs::create_dir_all(&port_dir).unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let archive_url = url::Url::from_file_path(&archive).unwrap();
        let port = make_port(&port_dir, archive_url, &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor: port,
                port_dir: port_dir.clone(),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, PortError::Fs { .. }) || matches!(err, PortError::Extract { .. }) || matches!(err, PortError::ChecksumMismatch { .. }) || matches!(err, PortError::MissingArchive { .. }) || matches!(err, PortError::Io { .. }) || matches!(err, PortError::Toml { .. }),
            // We accept any error variant that signals the prep refused to populate.
            "{err:?}");
    }
}
```

(Note: tighten the frozen-fails test variant once the API is in — pick whichever variant is most idiomatic. The shape above is a placeholder.)

- [ ] **Step 2: Verify they fail**

Run: `cargo test -p cabin-port -- prepare::tests`
Expected: FAIL — module not implemented.

- [ ] **Step 3: Implement `prepare.rs`**

Pipeline per entry:
1. Resolve `archive_path = cache.archive_path(hex)` and `source_dir = cache.source_dir(hex)`.
2. If archive exists and hashes to expected hex, reuse. Otherwise (and not frozen): hash bytes from `PortFetchSource`, fail with `ChecksumMismatch` on mismatch, atomic-rename `<archive>.partial` → `<archive>`.
3. Sibling marker file (`<source_dir>.ok`) gates extraction reuse.
4. If marker missing OR source dir missing: clean source dir, `cabin_artifact::safe_extract_tar_gz(archive, source_dir, SafeExtractOptions { strip_prefix })`.
5. Copy port_dir/overlay.relative_path → source_dir/cabin.toml (overwrite if present; preserved verbatim).
6. Parse extracted cabin.toml via `cabin_manifest::load_manifest`; check `[package].name`/`version` equal port descriptor; error `OverlayIdentityMismatch` on mismatch.
7. Write `<source_dir>.ok` marker.
8. Return `PreparedPort { name, version, source_dir, port_dir, source_provenance }`.

Provide `PortPrepareOptions { frozen: bool }` and `PortPrepareResult { ports: Vec<PreparedPort> }`. Mirror cabin-artifact's marker-sibling pattern exactly (separate file outside source_dir, deleted before re-extraction).

`PreparedPort` also carries provenance fields for metadata threading later:

```rust
pub struct PreparedPort {
    pub name: cabin_core::PackageName,
    pub version: semver::Version,
    pub source_dir: PathBuf,
    pub port_dir: PathBuf,
    pub provenance: PortProvenance,
}

pub struct PortProvenance {
    pub url: url::Url,
    pub sha256_hex: String,
    pub strip_prefix: Option<String>,
    pub overlay_manifest: PathBuf,
}
```

Implementation note: include the marker write only AFTER overlay copy + identity cross-check succeed, so a crash mid-prep does not produce a false cache hit.

Implementation note: on a "frozen" call, refuse to populate the archive or re-extract. Use a dedicated `PortError` variant (`FrozenCacheMiss { name, version }`) and tighten the frozen test to check for that variant once it's defined.

- [ ] **Step 4: Run tests**

Run: `cargo test -p cabin-port`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cabin-port/src/prepare.rs
git commit -m "feat(cabin-port): prepare pipeline (fetch, verify, extract, overlay)"
```

---

## Task 6: `port = "..."` dependency form in cabin-core + cabin-manifest

**Files:**
- Modify: `crates/cabin-core/src/model.rs`
- Modify: `crates/cabin-manifest/src/raw.rs`
- Modify: `crates/cabin-manifest/src/parse.rs`
- Modify: `crates/cabin-manifest/src/error.rs`

- [ ] **Step 1: Add `DependencySource::Port { path }`**

In `cabin-core::model::DependencySource`, add a new variant:

```rust
/// Foundation-port dependency. The path points to a port
/// directory containing `port.toml` and an overlay
/// `cabin.toml`. The CLI orchestration layer prepares the
/// port before the workspace loader resolves it.
#[serde(rename = "port")]
Port(PathBuf),
```

Update any exhaustive `match` arms over `DependencySource` (search the workspace for the obvious sites: workspace patch resolution, metadata serialisation, lockfile writing, resolver inputs). The Port variant is treated identically to `Path` until the workspace loader does the resolution.

- [ ] **Step 2: Write failing manifest-parser tests**

In `cabin-manifest/src/parse.rs` tests module add:

```rust
#[test]
fn parses_port_dependency() {
    let text = r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib" }
"#;
    let parsed = parse_manifest_str(text).unwrap();
    let pkg = parsed.package.unwrap();
    let dep = pkg.dependencies.first().unwrap();
    assert_eq!(dep.name.as_str(), "zlib");
    match &dep.source {
        DependencySource::Port(p) => assert_eq!(p, &PathBuf::from("../ports/zlib")),
        other => panic!("expected Port, got {other:?}"),
    }
}

#[test]
fn rejects_port_combined_with_path() {
    let text = r#"
[package]
name = "x"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib", path = "../zlib" }
"#;
    let err = parse_manifest_str(text).unwrap_err();
    // Whichever specific error variant emerges, it must mention `port`.
    assert!(format!("{err}").contains("port"), "{err}");
}

#[test]
fn rejects_port_combined_with_version() {
    let text = r#"
[package]
name = "x"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib", version = "1.0" }
"#;
    let err = parse_manifest_str(text).unwrap_err();
    assert!(format!("{err}").contains("port"), "{err}");
}

#[test]
fn rejects_port_combined_with_workspace() {
    let text = r#"
[package]
name = "x"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib", workspace = true }
"#;
    let err = parse_manifest_str(text).unwrap_err();
    assert!(format!("{err}").contains("port"), "{err}");
}

#[test]
fn rejects_port_combined_with_system() {
    let text = r#"
[package]
name = "x"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib", system = true }
"#;
    let err = parse_manifest_str(text).unwrap_err();
    assert!(format!("{err}").contains("port"), "{err}");
}
```

- [ ] **Step 3: Verify they fail**

Run: `cargo test -p cabin-manifest -- parse::tests::`
Expected: FAIL — `port` field not recognised yet.

- [ ] **Step 4: Add the `port` field**

In `crates/cabin-manifest/src/raw.rs::RawDependencyTable`, add:

```rust
#[serde(default)]
pub(crate) port: Option<String>,
```

In `crates/cabin-manifest/src/parse.rs::dep_from_raw_table` (or its current name), add a case for `port`. Validation rules:
- If `port` is set and any of `path` / `version` / `workspace` / `system` is set, error with a descriptive `ManifestError` variant (e.g. `ConflictingDependencyForms`).
- If `port` is set and `features` / `default-features` / `optional` is set, error too (ports are unconditional path-like deps for this milestone).
- Convert to `DependencySource::Port(PathBuf::from(port_value))`.

Update `crates/cabin-manifest/src/error.rs` with a precise variant:

```rust
#[error(
    "dependency `{name}` declares `port` together with `{conflicting}`; these forms are mutually exclusive"
)]
ConflictingDependencyForms { name: String, conflicting: &'static str },

#[error(
    "dependency `{name}` declares `port` together with `{conflicting}`; foundation-port dependencies do not support feature flags or optional gating yet"
)]
UnsupportedPortDependencyOption { name: String, conflicting: &'static str },
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p cabin-manifest`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/cabin-core crates/cabin-manifest
git commit -m "feat(manifest): support { port = \"...\" } dependency form"
```

---

## Task 7: Workspace loader integration (`PortPackageSource`)

**Files:**
- Modify: `crates/cabin-workspace/src/loader.rs`
- Modify: `crates/cabin-workspace/src/lib.rs`
- Possibly: `crates/cabin-workspace/src/error.rs`

- [ ] **Step 1: Failing test**

In `crates/cabin-workspace/src/loader.rs` (or a new `tests/loader_port.rs`):

```rust
#[test]
fn resolves_port_dep_via_supplied_source() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    // Prepared port directory: contains the overlay cabin.toml.
    let prepared = root.join("prepared/zlib-cache");
    std::fs::create_dir_all(&prepared).unwrap();
    std::fs::write(
        prepared.join("cabin.toml"),
        "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"cpp_library\"\nsources = [\"zlib.c\"]\n",
    )
    .unwrap();
    std::fs::write(prepared.join("zlib.c"), "int zlib_dummy(void){return 0;}\n").unwrap();

    // Consumer manifest referencing the port by path.
    let consumer = root.join("consumer");
    std::fs::create_dir_all(consumer.join("src")).unwrap();
    std::fs::write(
        consumer.join("cabin.toml"),
        r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = "../ports/zlib" }

[target.consumer]
type = "cpp_executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
    )
    .unwrap();
    std::fs::write(
        consumer.join("src/main.c"),
        "#include <stdio.h>\nint main(){puts(\"ok\");return 0;}\n",
    )
    .unwrap();

    let port_sources = vec![PortPackageSource {
        port_dir: root.join("ports/zlib"),
        name: cabin_core::PackageName::new("zlib").unwrap(),
        version: semver::Version::new(1, 3, 1),
        manifest_path: prepared.join("cabin.toml"),
    }];
    let opts = WorkspaceLoadOptions {
        registry: &[],
        patches: &[],
        ports: &port_sources,
        strict_packages: &Default::default(),
        include_dev_for: &Default::default(),
    };
    let graph = load_workspace_with_options(consumer.join("cabin.toml"), &opts).unwrap();
    // Two packages: the consumer and the zlib port.
    assert_eq!(graph.packages.len(), 2);
    let zlib = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "zlib")
        .unwrap();
    assert_eq!(zlib.manifest_dir, prepared);
}
```

- [ ] **Step 2: Verify it fails**

Run: `cargo test -p cabin-workspace`
Expected: FAIL — `PortPackageSource` / `ports` field on options not defined.

- [ ] **Step 3: Add `PortPackageSource` + loader resolution**

Mirror `RegistryPackageSource`. In `loader.rs`:

```rust
#[derive(Debug, Clone)]
pub struct PortPackageSource {
    pub name: PackageName,
    pub version: semver::Version,
    /// Absolute path to the prepared port directory's
    /// `cabin.toml` (the overlay).
    pub manifest_path: PathBuf,
    /// Absolute path to the foundation port directory (the
    /// one with `port.toml`). Carried through for provenance.
    pub port_dir: PathBuf,
}
```

Extend `WorkspaceLoadOptions` with `pub ports: &'a [PortPackageSource]`. In the loader's dep-walker, when a dep has `DependencySource::Port(rel)`, look up the matching `PortPackageSource` by `(name, canonicalised port_dir)` — error `PortNotPrepared { name, port_dir }` if none. Then treat it identically to a path dep that points at `manifest_path.parent()`.

Update the strict-default loader entry point to pass an empty slice for `ports`.

Export `PortPackageSource` from `lib.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p cabin-workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cabin-workspace
git commit -m "feat(cabin-workspace): resolve port deps via PortPackageSource"
```

---

## Task 8: CLI orchestration (port discovery + preparation)

**Files:**
- Create: `crates/cabin-cli/src/port_glue.rs`
- Modify: `crates/cabin-cli/src/lib.rs` — declare module
- Modify: `crates/cabin-cli/src/cli.rs` — wire `discover_and_prepare_ports` into `build`, `run`, `test`, `metadata` (and any other entry point that loads the workspace).
- Modify: `crates/cabin-cli/Cargo.toml` — add `cabin-port = { workspace = true }`

- [ ] **Step 1: Sketch the discovery + prep helper**

`port_glue.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use cabin_core::DependencySource;
use cabin_index_http::HttpClient;
use cabin_manifest::{ParsedManifest, load_manifest};
use cabin_port::{
    PortCache, PortEntry, PortFetchSource, PortPlan, PortPrepareOptions, PreparedPort,
    load_port, prepare,
};
use cabin_workspace::PortPackageSource;

pub(crate) struct PortPrepResult {
    pub sources: Vec<PortPackageSource>,
}

pub(crate) struct PortPrepInputs<'a> {
    pub root_manifest: &'a Path,
    pub workspace: &'a ParsedManifest,
    pub cache_dir: &'a Path,
    pub offline: bool,
    pub frozen: bool,
    pub http_client: Option<&'a HttpClient>,
}

/// Discover every foundation-port dep reachable from the root
/// manifest, prepare each port once (download → verify → extract
/// → overlay) and return one `PortPackageSource` per port.
pub(crate) fn discover_and_prepare(inputs: PortPrepInputs<'_>) -> Result<PortPrepResult> {
    // 1. Walk the manifest closure to collect every `Port(path)`
    //    dependency. Each port path is resolved relative to the
    //    manifest that declared it; we canonicalise so a single
    //    port appears once even if multiple consumers reference it.
    // 2. For each port_dir, call `cabin_port::load_port`.
    // 3. Decide fetch source per port:
    //      - file:// URL  → LocalArchive(path)
    //      - https://     → HttpClient::download (errored if offline)
    // 4. Build PortPlan, call cabin_port::prepare.
    // 5. Translate each PreparedPort into PortPackageSource and return.
}
```

Concrete implementation must:
- Use a stable iteration order over discovered ports (sort by `port_dir` so the metadata output is deterministic).
- Refuse https URLs when `offline` is true with a clear message.
- Pass `frozen` through to `PortPrepareOptions`.
- Surface `cabin-index-http::IndexHttpError` errors through anyhow's `Context`.

- [ ] **Step 2: Wire into command handlers**

Pick the smallest set of cabin-cli command handlers (in `cli.rs`) that load a workspace:
- `build`, `run`, `test`, `metadata`, `tree`, `explain`, `clean`, `fetch`, `vendor`, `fmt`, `tidy`.

For each, before constructing `WorkspaceLoadOptions`, call `discover_and_prepare` and pass the resulting `sources` into `opts.ports`. Ports for commands that don't compile (e.g. `clean`, `tree`) are still discovered and resolved (so name resolution succeeds), but the heavier preparation is identical — there is no fast path that skips it for this milestone.

- [ ] **Step 3: Cache directory selection**

Reuse the existing `--cache-dir` / `CABIN_CACHE_DIR` / `<root>/.cabin/cache` chain. Append a `ports/` suffix so port artifacts live under `.cabin/cache/ports/`. Document this in `docs/foundation-ports.md`.

- [ ] **Step 4: Run the existing workspace test suite**

Run: `cargo test -p cabin-cli --test cli -- --skip foundation_port_zlib`
Expected: PASS — existing tests must keep passing; the new ones come in Task 9.

- [ ] **Step 5: Commit**

```bash
git add crates/cabin-cli
git commit -m "feat(cabin-cli): discover and prepare foundation ports before workspace load"
```

---

## Task 9: Hermetic integration test (mock HTTP fake-zlib + downstream consumer)

**Files:**
- Modify: `crates/cabin-cli/tests/cli.rs`

- [ ] **Step 1: Add a `mod foundation_port_zlib` section**

Use `tiny_http` (already a workspace dep) to serve a synthesized "fake-zlib" archive that exports `zlibVersion`. The fixture:

```c
// zlib-1.3.1/zlib.h
#ifndef ZLIB_H
#define ZLIB_H
const char *zlibVersion(void);
#endif

// zlib-1.3.1/zlib.c
#include "zlib.h"
const char *zlibVersion(void) { return "1.3.1"; }
```

Steps inside the test:

1. Build the in-memory `.tar.gz` containing the two files, compute its SHA-256.
2. Spin up `tiny_http::Server::http("127.0.0.1:0")` on a background thread; respond to `/zlib-1.3.1.tar.gz` with the archive bytes.
3. Lay down `<tmp>/ports/zlib/port.toml` with `url = "http://<addr>/zlib-1.3.1.tar.gz"`, the computed SHA-256, `strip_prefix = "zlib-1.3.1"`, and `manifest = "cabin.toml"`.
4. Lay down `<tmp>/ports/zlib/cabin.toml` with `[package]` (name=zlib, version=1.3.1) and `[target.zlib]` (type=cpp_library, sources=["zlib.c"], include_dirs=["."]).
5. Lay down `<tmp>/consumer/cabin.toml` with:
   ```toml
   [package]
   name = "consumer"
   version = "0.1.0"

   [dependencies]
   zlib = { port = "../ports/zlib" }

   [target.consumer]
   type = "cpp_executable"
   sources = ["src/main.c"]
   deps = ["zlib"]
   ```
6. Lay down `<tmp>/consumer/src/main.c`:
   ```c
   #include <zlib.h>
   #include <stdio.h>
   int main(void) {
       const char *v = zlibVersion();
       if (!v || !*v) return 1;
       puts(v);
       return 0;
   }
   ```
7. `cabin().arg("build").arg("--manifest-path").arg(...).arg("--build-dir").arg(...).assert().success();`
8. Locate the built executable; run it; assert stdout contains `1.3.1`.
9. Re-run `cabin build` and assert the second run is a cache hit (no re-download — easy proxy: archive is fetched exactly once across both invocations; tiny_http server tracks request count).
10. Confirm the prepared port directory contains the overlay `cabin.toml` and that the upstream sources were placed at the root (no `zlib-1.3.1/` prefix dir remained).

Also add narrower tests:

- `foundation_port_zlib::reports_checksum_mismatch_on_tampered_archive`: same setup but the server returns bytes that don't match the declared SHA-256. Expect `cabin build` to fail with a clear `checksum mismatch` message.
- `foundation_port_zlib::reports_missing_strip_prefix`: archive has wrong root directory. Expect a clear error.
- `foundation_port_zlib::rejects_unsupported_source_type`: `port.toml` declares `type = "git"`. `cabin build` fails before any network call.

- [ ] **Step 2: Add a port-schema regression test for the real `ports/zlib/port.toml`**

In the same file (or a new module), load `ports/zlib/port.toml` from the workspace root and assert:
- Parser accepts it.
- `name == "zlib"`, `version == "1.3.1"`.
- `[source].type == "archive"`, `sha256` is 64 lowercase hex, `strip_prefix == "zlib-1.3.1"`, `url` parses and ends with `.tar.gz`.

This catches accidental edits to the port file without requiring network.

- [ ] **Step 3: Run the new tests**

Run: `cargo test -p cabin-cli --test cli foundation_port_zlib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/cabin-cli/tests/cli.rs
git commit -m "test(cabin-cli): foundation-port zlib pipeline end-to-end"
```

---

## Task 10: Author the real zlib foundation port + ports/README.md

**Files:**
- Create: `ports/README.md`
- Create: `ports/zlib/port.toml`
- Create: `ports/zlib/cabin.toml`

- [ ] **Step 1: Write `ports/README.md`**

Content (verbatim — no marketing claims):

```markdown
# Cabin foundation ports

This directory holds **curated foundation ports**: Cabin recipes
that adapt important existing C/C++ libraries — libraries that
do not yet ship a native `cabin.toml` — to Cabin's build model.

A foundation port consists of:

- `port.toml` — pins a single upstream release archive by URL
  and SHA-256, optionally with a `strip_prefix` for the
  archive's root directory.
- `cabin.toml` — a Cabin overlay manifest that describes the
  upstream sources as ordinary Cabin C/C++ targets.

When a Cabin package declares a dependency of the form
`{ port = "path/to/port" }`, Cabin downloads the archive,
verifies the SHA-256, safely extracts it, copies the overlay
manifest into the extracted source tree, and treats the result
as a normal Cabin path dependency.

## What ports are not

- They are **not Cabin's public registry**.
- They are **not a submission queue** for arbitrary C/C++
  libraries; this directory is curated.
- They are **not** a mechanism for distributing pre-built
  binaries or compiled artifacts.
- They are **not** a workaround for missing build-script
  support — they only describe libraries whose source layout
  fits Cabin's existing target model (a list of sources plus
  include directories).

## Policy

- Sources must be pinned by URL and SHA-256. Floating
  references (`latest`, branches, tag-only without integrity)
  are rejected.
- No upstream build-system invocation. Cabin never runs CMake,
  Autotools, Meson, Make, or upstream `configure` scripts.
- Patches under `patches/` (if any) should be limited to
  what is strictly required to make a port build through Cabin.
- A foundation port should be **retired** once its upstream
  project ships and maintains a native `cabin.toml`.

## Available ports

- `zlib/` — the zlib compression library, pinned to a single
  upstream release.
```

- [ ] **Step 2: Write `ports/zlib/port.toml`**

```toml
[port]
name = "zlib"
version = "1.3.1"
description = "Compression library"
license = "Zlib"
homepage = "https://zlib.net/"
upstream = "https://github.com/madler/zlib"

[source]
type = "archive"
url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
strip_prefix = "zlib-1.3.1"

[overlay]
manifest = "cabin.toml"
```

Verify the checksum against zlib's official release page before committing. The hex above is the documented SHA-256 of `zlib-1.3.1.tar.gz` from the upstream release page; the plan task includes one-shot verification:

```bash
# Manual verification — NOT part of the test suite.
curl -L https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz -o /tmp/zlib-1.3.1.tar.gz
sha256sum /tmp/zlib-1.3.1.tar.gz
# expected: 9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
```

If the actual checksum differs, update `port.toml` to match what the canonical release archive actually hashes to and re-run.

- [ ] **Step 3: Write `ports/zlib/cabin.toml`**

```toml
[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "cpp_library"
sources = [
    "adler32.c",
    "compress.c",
    "crc32.c",
    "deflate.c",
    "gzclose.c",
    "gzlib.c",
    "gzread.c",
    "gzwrite.c",
    "infback.c",
    "inffast.c",
    "inflate.c",
    "inftrees.c",
    "trees.c",
    "uncompr.c",
    "zutil.c",
]
include_dirs = ["."]
```

These are the canonical zlib 1.3.1 C source filenames. The include directory is `.` because `zlib.h` and `zconf.h` live at the archive root after `strip_prefix`. zlib 1.3.1 does NOT require `zconf.h.in` → `zconf.h` configuration; the upstream tarball ships a generated `zconf.h` at the root, which is what every distro consumes. Cabin uses that file as-is.

- [ ] **Step 4: Run the schema regression test**

Run: `cargo test -p cabin-cli --test cli foundation_port_zlib::port_toml_schema`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ports/
git commit -m "feat(ports): add zlib foundation port (1.3.1)"
```

---

## Task 11: Thread port provenance through `cabin metadata`

**Files:**
- Modify: `crates/cabin-cli/src/metadata_glue.rs` (and any helper modules it pulls in)
- Modify: any explain / tree renderers that should mention port sources

- [ ] **Step 1: Surface a `port` block in metadata JSON**

For each package in the metadata view that originated from a `PortPackageSource`, emit:

```json
{
  "name": "zlib",
  "version": "1.3.1",
  "manifest_path": "<absolute path to prepared cabin.toml>",
  "source": {
    "kind": "port",
    "port_dir": "<absolute path to ports/zlib>",
    "url": "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz",
    "sha256": "9a93b2b...",
    "strip_prefix": "zlib-1.3.1",
    "overlay_manifest": "cabin.toml"
  }
}
```

(Match the existing metadata-view shape; the snippet above is illustrative.)

- [ ] **Step 2: Add a metadata test**

Add to `crates/cabin-cli/tests/cli.rs` a test that runs `cabin metadata` against the foundation-port fixture and asserts the resulting JSON contains the `source.kind == "port"` block for the zlib package.

- [ ] **Step 3: Run tests**

Run: `cargo test -p cabin-cli --test cli foundation_port_zlib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/cabin-cli
git commit -m "feat(cabin-cli): surface port provenance in cabin metadata"
```

---

## Task 12: Documentation

**Files:**
- Create: `docs/foundation-ports.md`
- Modify: `docs/architecture.md`
- Modify: `docs/manifest.md`

- [ ] **Step 1: Write `docs/foundation-ports.md`**

Cover:
- What a foundation port is, what it is not (mirroring `ports/README.md` for consistency).
- The minimal `port.toml` schema (every supported field with type/required notes).
- The overlay-manifest contract (must match port identity).
- The dependency form: `{ port = "path" }` and its mutual-exclusion rules.
- The preparation pipeline order: discover → fetch → checksum verify → safe extract with strip_prefix → overlay copy → identity cross-check.
- The error catalog (checksum mismatch, missing strip_prefix, overlay identity mismatch, unsupported source type, …).
- The cache layout (`<cache>/ports/archives/sha256/<hex>.tar.gz`, `<cache>/ports/sources/sha256/<hex>/`).
- The current scope: only zlib ships under `ports/` today; foundation ports are retired once upstream projects ship native `cabin.toml`.
- Explicit non-goals: no registry, no submission queue, no build scripts, no options/variants, no CMake/Meson/Autotools invocation, no fmt/libpng/etc.

- [ ] **Step 2: Update `docs/architecture.md`**

- Add `cabin-port/` to the crate map with one-paragraph responsibilities.
- Add a "Foundation ports" subsection that links to `docs/foundation-ports.md` and states zlib as the first external library milestone.
- No promises about other libraries.

- [ ] **Step 3: Update `docs/manifest.md`**

- Document `port = "path"` in the dependency-table section, alongside `path`, `version`, `workspace`. Note mutual exclusion.
- Cross-link to `docs/foundation-ports.md`.

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs: foundation ports + zlib milestone"
```

---

## Task 13: Workspace-wide verification

- [ ] **Step 1: Format**

Run: `cargo fmt --all -- --check`
Expected: PASS.

If it fails, run `cargo fmt --all` and commit the result.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

Fix any new clippy diagnostics by simplifying the code; do not add `#[allow(...)]` unless the existing crate-level allow list already covers a comparable case (e.g. `clippy::missing_errors_doc`).

- [ ] **Step 3: Test suite**

Run: `cargo test --workspace --all-targets`
Expected: PASS.

Investigate every failure. The most likely places things break:
- Match arms over `DependencySource` that did not learn about `Port`.
- Golden snapshots that mention the dependency-source list.
- `cabin metadata` outputs whose ordering needs the new `port` source kind sorted in.

- [ ] **Step 4: Final integration test confirmation**

Run: `cargo test -p cabin-cli --test cli foundation_port_zlib -- --nocapture`
Expected: PASS — fake-zlib archive served over loopback HTTP, downstream `consumer` executable runs, stdout contains `1.3.1`.

- [ ] **Step 5: Commit any verification-driven cleanups**

Group small fixes that the verification pass surfaced into one commit per affected area:

```bash
git add ...
git commit -m "chore: fix lint/test fallout from foundation ports"
```

---

## Self-review

Spec coverage walk:

- [x] `ports/` exists with `README.md` policy — Task 10
- [x] `ports/zlib/port.toml` pins one fixed release with SHA-256 — Task 10
- [x] `ports/zlib/cabin.toml` builds zlib as a static C library through Cabin — Task 10 + Task 8 (default `cpp_library` is a static archive)
- [x] Archive checksum validation before extraction — Tasks 4 + 5
- [x] Overlay manifest applied without invoking CMake/etc — Tasks 5 + 10
- [x] Downstream consumer builds + runs + calls `zlibVersion()` — Task 9
- [x] Public include/link propagation — exercised by Task 9's `#include <zlib.h>` consumer
- [x] Metadata/tree/explain surfaces port provenance — Task 11
- [x] No new build scripts/options/variants/tool-deps/git-deps/registry-server — verified by what is *not* in the file list above
- [x] Docs scoped to zlib only — Task 12
- [x] All tests/fmt/clippy pass — Task 13
- [x] `port.toml` parser rejects unsupported source forms (no sha256, branch/latest, tag-only without integrity) — Task 2
- [x] Clear diagnostics for missing archive, checksum mismatch, invalid strip_prefix, unsupported source type — Tasks 5 + 9

Placeholders/TBD scan: only the "frozen-fails test variant" note in Task 5 is intentionally fuzzy — the variant name (`FrozenCacheMiss`) is defined in the surrounding implementation step, so the test asserts on it once the type is in.

Type consistency: `PreparedPort`, `PortPackageSource`, `PortFetchSource`, `PortProvenance`, `PortPrepareOptions` are used identically across Tasks 5–11. No renames between earlier and later steps.

Repository-content/privacy: nothing personal/legal/maintainer-specific anywhere in the plan.
