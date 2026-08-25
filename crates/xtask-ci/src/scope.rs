//! Which surfaces changed relative to `origin/main`, so the expensive
//! checks run only where they can fail (AGENTS.md, "run only the
//! checks that match the touched surface").
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
/// The prefixes are matched at the start of each path, as the shell's
/// `grep -qE '^(...)'` matched them per line: a `crates/` nested
/// deeper (`registry/crates/x`) is deliberately not a Rust change.
#[must_use]
pub fn surfaces(changed: &[String]) -> Surfaces {
    let any = |prefixes: &[&str]| {
        changed
            .iter()
            .any(|path| prefixes.iter().any(|prefix| path.starts_with(prefix)))
    };
    Surfaces {
        // `ports/` counts as a Rust surface: the publisher's
        // committed-tree guard catches incomplete version directories,
        // so a ports-only change still has to run the Rust gate.
        rust: any(&[
            "crates/",
            "examples/",
            "ports/",
            "Cargo.",
            ".cargo/",
            "rust-toolchain",
        ]),
        // The website build also loads the foundation ports
        // (`website/src/lib/ports.ts` reads `ports/`).
        website: any(&["website/", "docs/", "ports/"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(paths: &[&str]) -> Surfaces {
        surfaces(&paths.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_rust_surface_matches_what_the_shell_matched() {
        for path in [
            "crates/cabin/src/lib.rs",
            "examples/hello-c/cabin.toml",
            "ports/zlib/1.3.1/cabin.toml",
            "Cargo.toml",
            "Cargo.lock",
            ".cargo/config.toml",
            "rust-toolchain.toml",
        ] {
            assert!(of(&[path]).rust, "{path} should be a Rust change");
        }
        for path in [
            // Anchored at the start, as the shell's `^` was: a nested
            // `crates/` belongs to another workspace.
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
    fn the_website_surface_includes_docs_and_ports() {
        for path in [
            "website/package.json",
            "docs/architecture.md",
            "ports/zlib/1.3.1/cabin.toml",
        ] {
            assert!(of(&[path]).website, "{path} should be a website change");
        }
        assert!(!of(&["crates/cabin/src/lib.rs"]).website);
        assert!(!of(&["CONTRIBUTING.md"]).website);
    }

    /// One path on a shared surface pulls in every check that surface
    /// feeds, which is why `ports/` sets both.
    #[test]
    fn ports_touch_both_the_rust_and_website_surfaces() {
        let touched = of(&["ports/zlib/1.3.1/cabin.toml"]);
        assert_eq!(
            touched,
            Surfaces {
                rust: true,
                website: true,
            }
        );
    }

    /// No merge base means no diff, and a gate that cannot tell what
    /// changed runs everything.
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
