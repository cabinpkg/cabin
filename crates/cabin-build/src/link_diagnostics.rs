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
//! The same gap surfaces one step earlier for header-only use: a
//! compile that cannot find an included header, because a dep's
//! include directories reach a target only through `deps`.
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
        /// Which action exposed the gap.
        step: FailedStep,
    },
}

/// The failing action behind a [`LinkDiagnostic`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FailedStep {
    /// The link (or archive) action.
    Link,
    /// A compile that could not find an included header.
    MissingHeader,
}

/// Top-level entry point.  Parses `stderr` from a build lowered for
/// `dialect`, walks the captured failure(s), and consults
/// `lookup_deps` to compute the gap.  Returns the first applicable
/// diagnostic, or `None` if the stderr does not look like a
/// recognizable link failure or missing-header compile failure.
pub fn diagnose<F>(stderr: &str, dialect: Dialect, lookup_deps: F) -> Option<LinkDiagnostic>
where
    F: Fn(&str, &str) -> Option<TargetDepInfo>,
{
    let (candidates, missing_header) = find_failed_target(stderr, dialect)?;
    // The path parse cannot tell a bare package dir from a scope
    // dir, or the planner's `/packages/` anchor from a source dir of
    // that name, so a failure line yields several readings; the first
    // one the workspace graph recognizes wins.
    for (step, package, target) in candidates {
        let Some(info) = lookup_deps(&package, &target) else {
            continue;
        };
        // Any other compile error is the user's code, not a missing dep.
        if step == FailedStep::MissingHeader && !missing_header {
            return None;
        }
        let unlinked: Vec<String> = info
            .package_deps
            .difference(&info.target_deps)
            .cloned()
            .collect();

        if unlinked.is_empty() {
            return None;
        }
        return Some(LinkDiagnostic::DeclaredButUnlinked {
            package,
            target,
            unlinked,
            step,
        });
    }
    None
}

/// Whether a compiler output line is an "included header not found"
/// error: Clang (`fatal error: 'x.h' file not found`), GCC
/// (`fatal error: x.h: No such file or directory`), or MSVC
/// (`fatal error C1083: Cannot open include file: 'x.h': No such file
/// or directory`).
fn is_missing_header(line: &str) -> bool {
    (line.contains("fatal error") && line.contains("file not found"))
        || (line.contains("fatal error:") && line.contains(": No such file or directory"))
        || (line.contains("C1083: Cannot open include file")
            && line.contains("No such file or directory"))
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
        step,
    } = diag;
    let primary = unlinked.join("`, `");
    let first = &unlinked[0];
    let (gap, role) = match step {
        FailedStep::Link => ("does not link it.", "what actually gets linked."),
        FailedStep::MissingHeader => (
            "does not list it in `deps`.",
            "what puts its headers on the include path\n\
             and links it.",
        ),
    };
    let _ = write!(
        out,
        "package `{package}` declares `{primary}` in `[dependencies]`,\n\
         but `[target.{target}]` {gap}\n\
         \n\
         `[dependencies]` makes a package available; each target's\n\
         `deps =` list is {role}\n\
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

/// Parse `stderr` for the first recognizable failed action.
/// Returns the failing target's candidate identities (package +
/// target name) in the order [`diagnose`] tries them: per
/// `/packages/` anchor, the bare reading first, then - when the path
/// is deep enough - the scoped `<scope>/<name>` reading.  The caller
/// resolves the ambiguity against the loaded workspace.
pub fn parse_link_failure_candidates(stderr: &str, dialect: Dialect) -> Vec<LinkFailure> {
    find_failed_target(stderr, dialect)
        .map(|(candidates, _)| candidates)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, package, target)| LinkFailure { package, target })
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

/// One `(step, package, target)` reading of a failed action's path.
type Reading = (FailedStep, String, String);

/// Find the first `FAILED:` line in ninja stderr and pull the
/// `(step, package, target)` readings out of the path it points at,
/// plus whether that action's own output reports a missing header.
///
/// Cabin's planner emits link-action outputs directly in the package
/// build dir - `<build_dir>/<profile>/packages/<package>/<target>`
/// for a bare package and `.../packages/<scope>/<name>/<target>` for
/// a scoped one - and compile outputs under its `obj/<target>/`
/// subdirectory, so the segments after `/packages/` are the
/// load-bearing signal.  Anything before `/packages/` is
/// platform-dependent build-root noise.  A two-component tail is a
/// bare link output; a three-component tail is ambiguous (bare
/// package with a subdir vs scoped package), so both readings are
/// returned, bare first; a deeper tail is an object file.
fn find_failed_target(stderr: &str, dialect: Dialect) -> Option<(Vec<Reading>, bool)> {
    let mut lines = stderr.lines();
    while let Some(line) = lines.next() {
        // The rest of the line is one or more space-separated
        // output paths (ninja's `FAILED:` lists every output of
        // the failing edge); only the planner's own output sits
        // under `/packages/`.
        let Some(rest) = line.trim_start().strip_prefix("FAILED:") else {
            continue;
        };
        for token in rest.split_whitespace() {
            // Normalize Windows-style separators so one split on
            // `/` anchors the parse on every platform.  Cabin's planner emits the same logical
            // layout regardless of OS; only ninja's stderr
            // separator differs.
            let normalized = token.replace('\\', "/");
            let parts: Vec<&str> = normalized.split('/').collect();
            // Read every `packages` component as an anchor, the last
            // one first: `--build-dir` can itself contain one (e.g.
            // `/tmp/packages/out`), an object's source path can contain
            // one after the planner's, and a package can be named one.
            let candidates: Vec<_> = (0..parts.len())
                .rev()
                .filter(|&idx| parts[idx] == "packages")
                .flat_map(|idx| readings(&parts[idx + 1..], dialect))
                .collect();
            if !candidates.is_empty() {
                // The action's own output runs up to the next banner;
                // later ones belong to other jobs of a parallel build.
                let missing_header = lines
                    .take_while(|line| !line.trim_start().starts_with("FAILED:"))
                    .any(is_missing_header);
                return Some((candidates, missing_header));
            }
        }
    }
    None
}

/// The `(step, package, target)` readings of the path segments after
/// a `/packages/` anchor, see [`find_failed_target`].
fn readings(tail: &[&str], dialect: Dialect) -> Vec<Reading> {
    let link = |pkg: String, file: &str| {
        (
            FailedStep::Link,
            pkg,
            extract_target_name(file, dialect).to_owned(),
        )
    };
    let compile = |pkg: String, target: &str| (FailedStep::MissingHeader, pkg, target.to_owned());
    if tail.iter().any(|part| part.is_empty()) {
        return Vec::new();
    }
    match *tail {
        [] | [_] => Vec::new(),
        [pkg, file] => vec![link(pkg.to_owned(), file)],
        [pkg, target, file] => vec![
            link(pkg.to_owned(), target),
            link(format!("{pkg}/{target}"), file),
        ],
        _ => {
            let mut candidates = Vec::new();
            if tail[1] == "obj" {
                candidates.push(compile(tail[0].to_owned(), tail[2]));
            }
            if tail.len() >= 5 && tail[2] == "obj" {
                candidates.push(compile(format!("{}/{}", tail[0], tail[1]), tail[3]));
            }
            candidates
        }
    }
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
            step,
        } = diag;
        assert_eq!(package, "mytest");
        assert_eq!(target, "mytest");
        assert_eq!(unlinked, vec!["zlib"]);
        assert_eq!(step, FailedStep::Link);
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
            step: FailedStep::Link,
        };
        let out = render(&diag);
        assert!(out.contains("`mytest`"));
        assert!(out.contains("`zlib`"));
        assert!(out.contains("[target.mytest]"));
        assert!(out.contains("deps = [\"zlib\"]"));
    }

    // ---- missing-header compile failures ---------------------------

    fn diagnose_compile(output: &str, dialect: Dialect) -> Option<LinkDiagnostic> {
        diagnose(output, dialect, |pkg, target| {
            (pkg == "app" && target == "app").then(|| deps(&["kit"], &[]))
        })
    }

    /// Clang's wording, with the object under the package's
    /// `obj/<target>/` subdirectory.
    #[test]
    fn flags_a_missing_header_in_a_bare_package_compile() {
        let output = "FAILED: [code=1] /abs/build/dev/packages/app/obj/app/src/main.cc.o\n\
             /usr/bin/c++ -c src/main.cc -o ...\n\
             src/main.cc:1:10: fatal error: 'kit.h' file not found\n";
        let diag = diagnose_compile(output, Dialect::GnuLike).unwrap();
        assert_eq!(
            diag,
            LinkDiagnostic::DeclaredButUnlinked {
                package: "app".into(),
                target: "app".into(),
                unlinked: vec!["kit".into()],
                step: FailedStep::MissingHeader,
            }
        );
    }

    /// GCC's wording for a C source, in a scoped package's
    /// `obj/<target>/`.
    #[test]
    fn flags_a_missing_header_in_a_scoped_package_compile() {
        let output = "FAILED: build/dev/packages/acme/app/obj/demo/src/main.c.o\n\
             src/main.c:1:10: fatal error: kit.h: No such file or directory\n";
        let diag = diagnose(output, Dialect::GnuLike, |pkg, target| {
            (pkg == "acme/app" && target == "demo").then(|| deps(&["kit"], &[]))
        })
        .unwrap();
        let LinkDiagnostic::DeclaredButUnlinked {
            package, target, ..
        } = diag;
        assert_eq!(package, "acme/app");
        assert_eq!(target, "demo");
    }

    /// MSVC's `C1083`, with Windows separators.
    #[test]
    fn flags_a_missing_header_from_msvc() {
        let output = "FAILED: build\\dev\\packages\\app\\obj\\app\\src\\main.cc.obj\n\
             src\\main.cc(1): fatal error C1083: Cannot open include file: 'kit.h': \
             No such file or directory\n";
        assert!(diagnose_compile(output, Dialect::Msvc).is_some());
    }

    /// A source directory named `packages` sits after the planner's
    /// own `/packages/` anchor and must not hide it, whether the
    /// source tail parses as nothing or as a link output.
    #[test]
    fn finds_the_planner_anchor_past_a_source_dir_named_packages() {
        for source in ["src/packages/main.cc.o", "src/packages/net/conn.cc.o"] {
            let output = format!(
                "FAILED: /b/dev/packages/app/obj/app/{source}\n\
                 fatal error: 'kit.h' file not found\n"
            );
            assert!(
                diagnose_compile(&output, Dialect::GnuLike).is_some(),
                "{source}"
            );
        }
    }

    /// A package named `packages` sits right after the planner's own
    /// anchor component.
    #[test]
    fn reads_a_package_named_packages() {
        let output = "FAILED: /b/dev/packages/packages/obj/app/src/main.cc.o\n\
             fatal error: 'kit.h' file not found\n";
        let diag = diagnose(output, Dialect::GnuLike, |pkg, target| {
            (pkg == "packages" && target == "app").then(|| deps(&["kit"], &[]))
        });
        assert!(diag.is_some());
    }

    /// Any other compile error is the user's code: no hint, even with
    /// an unlisted dep.
    #[test]
    fn ignores_compile_failures_that_are_not_a_missing_header() {
        let output = "FAILED: /b/dev/packages/app/obj/app/src/main.cc.o\n\
             src/main.cc:3:1: error: expected ';' after expression\n";
        assert!(diagnose_compile(output, Dialect::GnuLike).is_none());
        let msvc = "FAILED: build\\dev\\packages\\app\\obj\\app\\src\\main.cc.obj\n\
             src\\main.cc(1): fatal error C1083: Cannot open source file: 'gen.cc': \
             No such file or directory\n";
        assert!(diagnose_compile(msvc, Dialect::Msvc).is_none());
    }

    /// In a parallel build a missing header reported by a later
    /// failure says nothing about the first failed action.
    #[test]
    fn ignores_a_missing_header_from_another_failed_action() {
        let output = "FAILED: /b/dev/packages/app/obj/app/src/main.cc.o\n\
             src/main.cc:3:1: error: expected ';' after expression\n\
             FAILED: /b/dev/packages/other/obj/other/src/lib.cc.o\n\
             src/lib.cc:1:10: fatal error: 'kit.h' file not found\n";
        assert!(diagnose_compile(output, Dialect::GnuLike).is_none());
    }

    #[test]
    fn render_missing_header_points_at_the_include_path() {
        let diag = LinkDiagnostic::DeclaredButUnlinked {
            package: "app".into(),
            target: "app".into(),
            unlinked: vec!["kit".into()],
            step: FailedStep::MissingHeader,
        };
        let out = render(&diag);
        assert!(out.contains("does not list it in `deps`"));
        assert!(out.contains("include path"));
        assert!(out.contains("deps = [\"kit\"]"));
    }
}
