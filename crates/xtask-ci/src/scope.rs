//! Which surfaces changed relative to `origin/main`, so the expensive
//! checks run only where they can fail.
//!
//! The dangerous direction here is scoping a check OUT of a run that
//! would have failed it - that reports green on a change CI lands red.
//! Every judgment below therefore defaults to "changed" when it
//! cannot tell.

/// Which of the gate's two expensive surfaces a change set touches.
#[derive(Debug, PartialEq, Eq)]
pub struct Surfaces {
    pub rust: bool,
    pub website: bool,
}

impl Surfaces {
    /// Everything, which is what an unknown base means: with no merge
    /// base there is nothing to diff against, so the gate runs whole
    /// rather than guessing.
    #[must_use]
    pub fn all() -> Self {
        Self {
            rust: true,
            website: true,
        }
    }
}

/// The surfaces a list of changed paths touches.
///
/// The prefixes are matched at the start of each path: a `crates/`
/// nested deeper (`registry/crates/x`) belongs to another workspace
/// and is not a Rust change.
#[must_use]
pub fn surfaces(changed: &[String]) -> Surfaces {
    let any = |prefixes: &[&str]| {
        changed
            .iter()
            .any(|path| prefixes.iter().any(|prefix| path.starts_with(prefix)))
    };
    Surfaces {
        rust: any(&[
            "crates/",
            "examples/",
            "Cargo.",
            ".cargo/",
            "rust-toolchain",
        ]),
        // The website build also loads the foundation ports of the
        // cabin-ports/ports commit `ports-revision` pins.
        website: any(&["website/", "docs/", "ports-revision"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(paths: &[&str]) -> Surfaces {
        surfaces(&paths.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_rust_surface_is_matched_by_anchored_prefixes() {
        for path in [
            "crates/cabin/src/lib.rs",
            "examples/hello-c/cabin.toml",
            "Cargo.toml",
            "Cargo.lock",
            ".cargo/config.toml",
            "rust-toolchain.toml",
        ] {
            assert!(of(&[path]).rust, "{path} should be a Rust change");
        }
        for path in [
            "registry/crates/x/src/lib.rs",
            "registry/src/lib.rs",
            "website/src/pages/index.astro",
            "README.md",
            ".github/workflows/rust.yml",
        ] {
            assert!(!of(&[path]).rust, "{path} should not be a Rust change");
        }
    }

    #[test]
    fn the_website_surface_includes_docs_and_the_ports_pin() {
        for path in ["website/package.json", "docs/manifest.md", "ports-revision"] {
            assert!(of(&[path]).website, "{path} should be a website change");
        }
        assert!(!of(&["crates/cabin/src/lib.rs"]).website);
        assert!(!of(&["CONTRIBUTING.md"]).website);
    }

    #[test]
    fn an_unknown_base_runs_the_whole_gate() {
        assert_eq!(
            Surfaces::all(),
            Surfaces {
                rust: true,
                website: true,
            }
        );
    }
}
