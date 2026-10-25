//! Post-link-failure diagnostics.
//!
//! When ninja fails because the linker couldn't resolve a symbol,
//! cabin has structured information the raw linker output lacks:
//! which target failed (from ninja's `FAILED:` line), which package
//! owns it, what that package's `[dependencies]` declares, and what
//! the target's own `deps =` list links.  The mismatch between
//! "declared at the package level" and "linked at the target level"
//! is the most common newcomer gotcha: `[dependencies]` makes a
//! dep *available*; `[target.X.deps]` is what gets passed
//! to the linker.  Cabin can spot the gap precisely.
//!
//! This module is the parser + matcher.  The CLI calls
//! [`diagnose`] with captured ninja stderr and a closure that
//! resolves a `(package, target)` pair to its dependency picture;
//! the resulting [`LinkDiagnostic`] is rendered into a hint.

use std::collections::BTreeSet;

use cabin_driver::Dialect;

/// Parsed picture of one failing link action.  Only the *first*
/// failure ninja reports is extracted; secondary failures in a
/// parallel build are not surfaced (the user fixes one, re-runs,
/// gets the next).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LinkFailure {
    /// Package name owning the target, derived from the
    /// `/packages/<package>/` segment cabin's planner embeds in
    /// every output path.
    pub package: String,
    /// Target name - the binary or library that failed to link.
    pub target: String,
}

/// What the dep walk in the loaded workspace tells us about a
/// failing target.  Built by the CLI from the loaded `PackageGraph`
/// and passed to [`diagnose`] via the lookup closure.
#[derive(Debug, Clone)]
pub struct TargetDepInfo {
    /// Names appearing in the package's `[dependencies]` table.
    pub package_deps: BTreeSet<String>,
    /// Names appearing in `[target.<name>].deps` for the failing
    /// target.
    pub target_deps: BTreeSet<String>,
}

/// Diagnostic shape.  The CLI calls [`render`] to format it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LinkDiagnostic {
    /// `[dependencies]` declares one or more names the failing
    /// target's `deps =` doesn't link.  The #1 newcomer gotcha:
    /// "I added it to `[dependencies]`, why isn't it linking?"
    DeclaredButUnlinked {
        package: String,
        target: String,
        /// Every declared name absent from `target.deps`, sorted.
        unlinked: Vec<String>,
    },
}

/// Top-level entry point.  Parses `stderr` from a build lowered for
/// `dialect`, walks the captured failure(s), and consults
/// `lookup_deps` to compute the gap.  Returns the first applicable
/// diagnostic, or `None` if the stderr does not look like a
/// recognizable link failure.
pub fn diagnose<F>(stderr: &str, dialect: Dialect, lookup_deps: F) -> Option<LinkDiagnostic>
where
    F: Fn(&str, &str) -> Option<TargetDepInfo>,
{
    // The path parse cannot tell a bare package dir from a scope
    // dir, so each failure line yields up to two readings; the first
    // one the workspace graph recognizes wins.
    for failure in parse_link_failure_candidates(stderr, dialect) {
        let Some(info) = lookup_deps(&failure.package, &failure.target) else {
            continue;
        };
        let unlinked: Vec<String> = info
            .package_deps
            .difference(&info.target_deps)
            .cloned()
            .collect();

        if unlinked.is_empty() {
            return None;
        }
        return Some(LinkDiagnostic::DeclaredButUnlinked {
            package: failure.package,
            target: failure.target,
            unlinked,
        });
    }
    None
}

/// Render a diagnostic as the multi-line text cabin emits to
/// stderr.  The CLI's reporter prepends the styled `help:` lead-in
/// and indents continuation lines, so this body is intentionally
/// plain: one logical paragraph per blank-line-separated block,
/// each line short enough to live under a six-column indent
/// without re-wrapping on an 80-column terminal.
///
/// Code blocks (TOML snippets) are indented four spaces inside
/// the body; combined with the reporter's six-space indent that
/// puts them at column ten, matching the Rust compiler's spacing
/// for help-attached suggestions.
pub fn render(diag: &LinkDiagnostic) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let LinkDiagnostic::DeclaredButUnlinked {
        package,
        target,
        unlinked,
    } = diag;
    let primary = unlinked.join("`, `");
    let first = &unlinked[0];
    let _ = write!(
        out,
        "package `{package}` declares `{primary}` in `[dependencies]`,\n\
         but `[target.{target}]` does not link it.\n\
         \n\
         `[dependencies]` makes a package available; each target's\n\
         `deps =` list is what actually gets linked.\n\
         \n\
         Add the dep to the target:\n\
         \n\
         \x20   [target.{target}]\n\
         \x20   # ...existing fields...\n\
         \x20   deps = [\"{first}\"]\n"
    );
    out
}

// -------------------------------------------------------------
// Parsing
// -------------------------------------------------------------

/// Parse `stderr` for the first recognizable link failure.
/// Returns the failing target's candidate identities (package +
/// target name): the bare reading first, then - when the path is
/// deep enough - the scoped `<scope>/<name>` reading.  The caller
/// resolves the ambiguity against the loaded workspace.
pub fn parse_link_failure_candidates(stderr: &str, dialect: Dialect) -> Vec<LinkFailure> {
    find_failed_target(stderr, dialect)
        .into_iter()
        .map(|(package, target)| LinkFailure { package, target })
        .collect()
}

/// Recover the declared target name from a link-action output
/// filename by inverting [`Dialect::static_library_name`] and
/// [`Dialect::executable_name`]:
///
/// * GCC/Clang: `lib<name>.a` for static libraries, `<name>` (no
///   extension) for executables
/// * MSVC: `<name>.lib` for static libraries, `<name>.exe` for
///   executables
///
/// Only the dialect's own wrapper is stripped, and only once - a
/// literal target named `tool.a` or `tool.exe` would otherwise be
/// misread as `tool` and the workspace-graph lookup would miss it,
/// silently suppressing the link hint.  Names that do not match
/// the dialect's patterns round-trip unchanged.
fn extract_target_name(filename: &str, dialect: Dialect) -> &str {
    let stem = match dialect {
        Dialect::GnuLike => filename
            .strip_prefix("lib")
            .and_then(|s| s.strip_suffix(".a")),
        Dialect::Msvc => filename
            .strip_suffix(".exe")
            .or_else(|| filename.strip_suffix(".lib")),
    };
    stem.filter(|s| !s.is_empty()).unwrap_or(filename)
}

/// Find the first `FAILED:` line in ninja stderr and pull the
/// `(package, target)` readings out of the path it points at.
///
/// Cabin's planner emits link-action outputs under
/// `<build_dir>/<profile>/packages/<package>/<target>` for a bare
/// package and `.../packages/<scope>/<name>/<target>` for a scoped
/// one, so the segment after `/packages/` is the load-bearing
/// signal.  Anything before `/packages/` is platform-dependent
/// build-root noise; anything after is the package dir(s) plus the
/// target executable or library file.  A two-component tail has one
/// reading; a deeper tail is ambiguous (bare package with an object
/// subdir vs scoped package), so both readings are returned, bare
/// first.
fn find_failed_target(stderr: &str, dialect: Dialect) -> Vec<(String, String)> {
    for line in stderr.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("FAILED:") {
            continue;
        }
        // The rest of the line is one or more space-separated
        // output paths (ninja's `FAILED:` lists every output of
        // the failing edge).  The link action has exactly one
        // output - the binary or static archive - and that's
        // the only path under `/packages/`.
        let rest = trimmed.trim_start_matches("FAILED:").trim_start();
        for token in rest.split_whitespace() {
            // Normalize Windows-style separators so a single
            // `/packages/` probe anchors the parse on every
            // platform.  Cabin's planner emits the same logical
            // layout regardless of OS; only ninja's stderr
            // separator differs.
            let normalized = token.replace('\\', "/");
            // Anchor on the *last* `/packages/` segment.
            // `--build-dir` can itself contain a `packages`
            // component (e.g. `/tmp/packages/out`), and `find`
            // would lock onto that prefix and split off the
            // wrong package/target pair.  The planner-owned
            // suffix is always the last `packages` segment, so
            // `rfind` is the load-bearing anchor.
            if let Some(idx) = normalized.rfind("/packages/") {
                let tail = &normalized[idx + "/packages/".len()..];
                let mut parts = tail.splitn(4, '/');
                let (Some(pkg), Some(target)) = (parts.next(), parts.next()) else {
                    continue;
                };
                if pkg.is_empty() || target.is_empty() {
                    continue;
                }
                let mut candidates = vec![(
                    pkg.to_owned(),
                    extract_target_name(target, dialect).to_owned(),
                )];
                if let Some(scoped_target) = parts.next().filter(|s| !s.is_empty()) {
                    candidates.push((
                        format!("{pkg}/{target}"),
                        extract_target_name(scoped_target, dialect).to_owned(),
                    ));
                }
                return candidates;
            }
        }
    }
    Vec::new()
}

// -------------------------------------------------------------
// Tests
// -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(package: &[&str], target: &[&str]) -> TargetDepInfo {
        TargetDepInfo {
            package_deps: package.iter().map(|s| (*s).to_owned()).collect(),
            target_deps: target.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    // ---- parser ---------------------------------------------------

    #[test]
    fn parses_macos_failed_line() {
        let stderr = "FAILED: [code=1] /abs/build/dev/packages/mytest/mytest\n\
                      /usr/bin/c++ ...\n";
        let failure = parse_link_failure_candidates(stderr, Dialect::GnuLike)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.package, "mytest");
        assert_eq!(failure.target, "mytest");
    }

    /// A static-library archive failure emits the wrapped
    /// `lib<name>.a` filename cabin's planner produces.  The parser
    /// must unwrap that exact shape so the workspace-graph lookup
    /// sees `<name>` - the form the user declared in
    /// `[target.<name>]`.
    #[test]
    fn parses_linux_failed_line_with_library_wrapper() {
        let stderr = "FAILED: build/dev/packages/mylib/libmylib.a\n/usr/bin/ar ...\n";
        let failure = parse_link_failure_candidates(stderr, Dialect::GnuLike)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.package, "mylib");
        assert_eq!(failure.target, "mylib");
    }

    /// Ninja on Windows emits backslash-separated paths.  The
    /// parser must still find `\packages\<package>\<target>`
    /// and recover the same identity it would on POSIX, so the
    /// downstream "declared-but-unlinked" hint reaches the
    /// Windows user too.
    #[test]
    fn parses_windows_failed_line_with_backslashes() {
        let stderr = "FAILED: build\\dev\\packages\\mytest\\mytest.exe\n\
                      link.exe ...\n";
        let failure = parse_link_failure_candidates(stderr, Dialect::Msvc)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.package, "mytest");
        assert_eq!(failure.target, "mytest");
    }

    /// `--build-dir` may itself contain a `packages` segment
    /// (e.g. `/tmp/packages/out`).  The parser must anchor on the
    /// planner-owned *trailing* `/packages/` segment, not the
    /// first occurrence in the path - otherwise the recovered
    /// `(package, target)` pair points at the wrong directory
    /// and the link hint is silently lost.
    #[test]
    fn anchors_on_last_packages_segment() {
        let stderr = "FAILED: /tmp/packages/out/dev/packages/realpkg/realtarget\nld: ...\n";
        let failure = parse_link_failure_candidates(stderr, Dialect::GnuLike)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.package, "realpkg");
        assert_eq!(failure.target, "realtarget");
    }

    /// A target spelled with a literal dot (e.g. `tool.v2`) is
    /// not an extension and must round-trip through the parser
    /// unchanged; the parser only unwraps the dialect's own
    /// artifact shapes, so anything else (including names that
    /// happen to look like an extension) survives verbatim.
    #[test]
    fn preserves_target_names_with_internal_dots() {
        let stderr = "FAILED: build/dev/packages/mypkg/tool.v2\nld: ...\n";
        let failure = parse_link_failure_candidates(stderr, Dialect::GnuLike)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(failure.package, "mypkg");
        assert_eq!(failure.target, "tool.v2");
    }

    /// Target names ending in `.a`, `.exe`, `.so`, `.dylib`,
    /// `.dll`, or `.lib` must round-trip unchanged on GCC/Clang.
    /// That dialect only produces `lib<name>.a` (libraries) and
    /// `<name>` (executables, no extension), so any dot in the
    /// FAILED path is part of the user's declared target name -
    /// stripping it would silently drop the link hint for
    /// projects whose targets happen to be spelled that way.
    #[test]
    fn preserves_target_names_ending_in_non_emitted_extensions() {
        for name in [
            "tool.a",
            "tool.exe",
            "tool.so",
            "plugin.dll",
            "filter.dylib",
            "shim.lib",
        ] {
            let stderr = format!("FAILED: build/dev/packages/mypkg/{name}\nld: ...\n");
            let failure = parse_link_failure_candidates(&stderr, Dialect::GnuLike)
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(failure.package, "mypkg");
            assert_eq!(failure.target, name);
        }
    }

    /// MSVC names outputs `<name>.exe` and `<name>.lib`.  The parser
    /// strips exactly one such wrapper and leaves the GCC/Clang
    /// `lib<name>.a` shape alone, since under MSVC that can only be
    /// a literal target name.
    #[test]
    fn unwraps_msvc_executable_and_library_names() {
        for (file, target) in [
            ("app.exe", "app"),
            ("greet.lib", "greet"),
            ("tool.exe.exe", "tool.exe"),
            ("cfg.lib.exe", "cfg.lib"),
            ("libfoo.a", "libfoo.a"),
        ] {
            let stderr = format!("FAILED: build\\dev\\packages\\mypkg\\{file}\nlink.exe ...\n");
            let failure = parse_link_failure_candidates(&stderr, Dialect::Msvc)
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(failure.package, "mypkg");
            assert_eq!(failure.target, target, "{file}");
        }
    }

    #[test]
    fn no_failed_line_returns_none() {
        assert!(
            parse_link_failure_candidates("ninja: no work to do\n", Dialect::GnuLike).is_empty()
        );
    }

    // ---- diagnose ------------------------------------------------

    fn ninja_link_failure() -> &'static str {
        "FAILED: [code=1] /abs/build/dev/packages/mytest/mytest\n\
         /usr/bin/c++ obj/main.cc.o -o /abs/.../mytest\n\
         ld: symbol(s) not found for architecture arm64\n"
    }

    #[test]
    fn flags_declared_but_unlinked() {
        let diag = diagnose(ninja_link_failure(), Dialect::GnuLike, |pkg, target| {
            assert_eq!(pkg, "mytest");
            assert_eq!(target, "mytest");
            // User declared `zlib` at the package level but
            // forgot to add it to the binary's `deps =`.
            Some(deps(&["zlib"], &[]))
        })
        .unwrap();
        let LinkDiagnostic::DeclaredButUnlinked {
            package,
            target,
            unlinked,
        } = diag;
        assert_eq!(package, "mytest");
        assert_eq!(target, "mytest");
        assert_eq!(unlinked, vec!["zlib"]);
    }

    /// A scoped package's link failure path carries two readings;
    /// the parser yields the bare one first and the scoped
    /// `<scope>/<name>` one second.
    #[test]
    fn parses_scoped_failed_line_into_both_candidates() {
        let stderr = "FAILED: build/dev/packages/fmtlib/fmt/demo\nld: ...\n";
        let candidates = parse_link_failure_candidates(stderr, Dialect::GnuLike);
        assert_eq!(
            candidates,
            vec![
                LinkFailure {
                    package: "fmtlib".to_owned(),
                    target: "fmt".to_owned(),
                },
                LinkFailure {
                    package: "fmtlib/fmt".to_owned(),
                    target: "demo".to_owned(),
                },
            ]
        );
    }

    /// When only the scoped reading names a known package, the
    /// diagnostic attributes the failure to `<scope>/<name>` and the
    /// real target - not to a phantom package named after the scope.
    #[test]
    fn diagnose_resolves_the_scoped_reading() {
        let stderr = "FAILED: [code=1] /abs/build/dev/packages/fmtlib/fmt/demo\n\
             /usr/bin/c++ obj/main.cc.o -o /abs/.../demo\n\
             ld: symbol(s) not found for architecture arm64\n";
        let diag = diagnose(stderr, Dialect::GnuLike, |pkg, target| {
            (pkg == "fmtlib/fmt" && target == "demo").then(|| deps(&["zlib"], &[]))
        })
        .unwrap();
        let LinkDiagnostic::DeclaredButUnlinked {
            package, target, ..
        } = diag;
        assert_eq!(package, "fmtlib/fmt");
        assert_eq!(target, "demo");
    }

    #[test]
    fn returns_none_when_target_already_links_dep() {
        // `zlib` is both declared AND linked - the failure must
        // be something else (a real bug in the user's code, a
        // missing system lib, etc.).  Cabin has no useful hint;
        // surface nothing.
        let diag = diagnose(ninja_link_failure(), Dialect::GnuLike, |_, _| {
            Some(deps(&["zlib"], &["zlib"]))
        });
        assert!(diag.is_none());
    }

    #[test]
    fn returns_none_when_nothing_is_declared() {
        // No deps declared at the package level - the gap-based
        // diagnostic has nothing to say.
        let diag = diagnose(ninja_link_failure(), Dialect::GnuLike, |_, _| {
            Some(deps(&[], &[]))
        });
        assert!(diag.is_none());
    }

    #[test]
    fn returns_none_when_package_lookup_fails() {
        // Closure returns None - the failing target isn't in the
        // graph.  Don't pretend to know anything.
        let diag = diagnose(ninja_link_failure(), Dialect::GnuLike, |_, _| None);
        assert!(diag.is_none());
    }

    #[test]
    fn returns_none_on_unparsable_stderr() {
        let diag = diagnose("just some random output\n", Dialect::GnuLike, |_, _| {
            Some(deps(&["zlib"], &[]))
        });
        assert!(diag.is_none());
    }

    #[test]
    fn lists_every_unlinked_dep_when_multiple_declared() {
        // Package declares zlib + fmt + spdlog; target links
        // none.  All three appear in `unlinked`.
        let diag = diagnose(ninja_link_failure(), Dialect::GnuLike, |_, _| {
            Some(deps(&["fmt", "spdlog", "zlib"], &[]))
        })
        .unwrap();
        let LinkDiagnostic::DeclaredButUnlinked { unlinked, .. } = &diag;
        assert_eq!(unlinked, &["fmt", "spdlog", "zlib"]);
    }

    // ---- render --------------------------------------------------

    #[test]
    fn render_declared_but_unlinked_names_target_and_dep() {
        let diag = LinkDiagnostic::DeclaredButUnlinked {
            package: "mytest".into(),
            target: "mytest".into(),
            unlinked: vec!["zlib".into()],
        };
        let out = render(&diag);
        assert!(out.contains("`mytest`"));
        assert!(out.contains("`zlib`"));
        assert!(out.contains("[target.mytest]"));
        assert!(out.contains("deps = [\"zlib\"]"));
    }
}
