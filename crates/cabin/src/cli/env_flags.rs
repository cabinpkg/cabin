//! Orchestration for the conventional C/C++ build-flag
//! environment variables (`CPPFLAGS`, `CFLAGS`, `CXXFLAGS`,
//! `LDFLAGS`).
//!
//! `cabin-env::build_flags` parses each value via POSIX
//! shell-style word splitting and produces a typed
//! [`EnvBuildFlags`].  This module is the single bridge that:
//!
//! 1. captures the four variables at command start (via a
//!    [`Fn(&str) -> Option<OsString>`] closure so tests stay
//!    pure);
//! 2. parses them once, mapping any parse error onto the
//!    typed [`EnvBuildFlagsError`];
//! 3. appends the parsed tokens to every **primary** package's
//!    [`ResolvedProfileFlags`] entry, *after* `pkg-config`
//!    contributions have already been merged.
//!
//! Calling this helper is the documented merge point for the
//! environment layer; every command that resolves a build
//! configuration (`build`, `run`, `test`, `tidy`, `metadata`,
//! `explain`) must call it directly after
//! `crate::cli::system_deps::augment_build_flags_with_system_deps`
//! so the resulting `BuildConfiguration::fingerprint` observes
//! the user's environment.
//!
//! ## Why primary-only
//!
//! Cabin keeps environment flags scoped to the user's own
//! workspace members for the same reason `pkg-config`
//! contributions do: a stray `-Werror` in `CXXFLAGS` should
//! never break a transitive dependency the user did not write.
//! Registry / path dependencies still observe their own
//! `[profile]` declarations and any flag they own, but the
//! environment is the user's, not the dependency's.

use std::collections::HashMap;

use anyhow::Result;

use cabin_core::ResolvedProfileFlags;
use cabin_env::{EnvBuildFlags, EnvBuildFlagsError, parse_env_build_flags};
use cabin_workspace::PackageGraph;

use crate::cli::term_verbosity::Reporter;
use crate::plural;

/// Read `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` from the
/// supplied env-lookup closure, parse each value using POSIX
/// shell-style word splitting, and merge the result into every
/// primary package's [`ResolvedProfileFlags`] entry.
///
/// Returns the augmented map (so call sites can chain it with
/// the system-deps step) plus the parsed [`EnvBuildFlags`] for
/// downstream observers (verbose reporting, future metadata
/// view).
///
/// Empty / whitespace-only / unset variables are no-ops.  A
/// malformed value surfaces as an `anyhow::Error` wrapping
/// the typed [`EnvBuildFlagsError`].
pub(crate) fn augment_build_flags_with_env<F>(
    graph: &PackageGraph,
    mut build_flags: HashMap<usize, ResolvedProfileFlags>,
    env: F,
    reporter: Reporter,
) -> Result<(HashMap<usize, ResolvedProfileFlags>, EnvBuildFlags)>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    let parsed = match parse_env_build_flags(&env) {
        Ok(flags) => flags,
        Err(err) => return Err(format_parse_error(err)),
    };
    if parsed.is_empty() {
        return Ok((build_flags, parsed));
    }
    apply_to_primary_packages(graph, &mut build_flags, &parsed);
    // Verbose mode acknowledges that an environment layer
    // applied without dumping the raw values (they can carry
    // local paths or tokens).  Very-verbose mode prints the
    // parsed argv tokens, matching the policy command-line
    // display already follows.
    if reporter.verbosity().shows_very_verbose() {
        log_very_verbose(&parsed, reporter);
    } else if reporter.verbosity().shows_verbose() {
        log_verbose(&parsed, reporter);
    }
    Ok((build_flags, parsed))
}

fn format_parse_error(err: EnvBuildFlagsError) -> anyhow::Error {
    // The error message already includes the offending variable
    // name and a stable summary of the failure mode.  Wrap as
    // `anyhow::Error` so it flows through the standard CLI error
    // path.
    anyhow::Error::new(err)
}

fn apply_to_primary_packages(
    graph: &PackageGraph,
    build_flags: &mut HashMap<usize, ResolvedProfileFlags>,
    parsed: &EnvBuildFlags,
) {
    for &idx in &graph.primary_packages {
        let entry = build_flags.entry(idx).or_default();
        // CPPFLAGS apply to both C/C++ compile commands, so
        // they land in the language-neutral bucket.
        entry
            .extra_compile_args
            .extend(parsed.cppflags.iter().cloned());
        entry.cflags.extend(parsed.cflags.iter().cloned());
        entry.cxxflags.extend(parsed.cxxflags.iter().cloned());
        entry.ldflags.extend(parsed.ldflags.iter().cloned());
    }
}

// Verbose chatter routes through the reporter's auxiliary stderr
// path, matching `system_deps`'s pattern. `cabin metadata`
// reserves stdout for its JSON document, so any human-readable
// line emitted from the shared build-orchestration path must use
// stderr or it pollutes the machine-readable contract.
fn log_verbose(parsed: &EnvBuildFlags, reporter: Reporter) {
    // One short line per active variable, with arg counts only.
    // The full values can carry local include paths or tokens;
    // very-verbose mode is the documented place to dump them.
    if !parsed.cppflags.is_empty() {
        reporter.aux_verbose(format_args!(
            "cabin: applying CPPFLAGS ({} arg{})",
            parsed.cppflags.len(),
            plural(parsed.cppflags.len()),
        ));
    }
    if !parsed.cflags.is_empty() {
        reporter.aux_verbose(format_args!(
            "cabin: applying CFLAGS ({} arg{})",
            parsed.cflags.len(),
            plural(parsed.cflags.len()),
        ));
    }
    if !parsed.cxxflags.is_empty() {
        reporter.aux_verbose(format_args!(
            "cabin: applying CXXFLAGS ({} arg{})",
            parsed.cxxflags.len(),
            plural(parsed.cxxflags.len()),
        ));
    }
    if !parsed.ldflags.is_empty() {
        reporter.aux_verbose(format_args!(
            "cabin: applying LDFLAGS ({} arg{})",
            parsed.ldflags.len(),
            plural(parsed.ldflags.len()),
        ));
    }
}

fn log_very_verbose(parsed: &EnvBuildFlags, reporter: Reporter) {
    if !parsed.cppflags.is_empty() {
        reporter.aux_very_verbose(format_args!("cabin: CPPFLAGS = {}", join(&parsed.cppflags)));
    }
    if !parsed.cflags.is_empty() {
        reporter.aux_very_verbose(format_args!("cabin: CFLAGS = {}", join(&parsed.cflags)));
    }
    if !parsed.cxxflags.is_empty() {
        reporter.aux_very_verbose(format_args!("cabin: CXXFLAGS = {}", join(&parsed.cxxflags)));
    }
    if !parsed.ldflags.is_empty() {
        reporter.aux_very_verbose(format_args!("cabin: LDFLAGS = {}", join(&parsed.ldflags)));
    }
}

fn join(args: &[String]) -> String {
    args.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{Package, PackageName, ResolvedProfileFlags, Target};
    use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn version() -> semver::Version {
        semver::Version::parse("0.1.0").unwrap()
    }

    fn make_pkg(name: &str) -> WorkspacePackage {
        let package = Package::new(
            PackageName::new(name).unwrap(),
            version(),
            Vec::<Target>::new(),
            Vec::new(),
        )
        .unwrap();
        WorkspacePackage {
            package,
            manifest_dir: PathBuf::from("/tmp"),
            manifest_path: PathBuf::from("/tmp/cabin.toml"),
            kind: PackageKind::Local,
            deps: Vec::new(),
        }
    }

    fn one_primary_graph() -> PackageGraph {
        let pkg = make_pkg("root");
        PackageGraph {
            root_manifest_path: PathBuf::from("/tmp/cabin.toml"),
            root_dir: PathBuf::from("/tmp"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![pkg],
        }
    }

    fn quiet_reporter() -> Reporter {
        Reporter::new(cabin_core::Verbosity::Normal)
    }

    fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn empty_env_leaves_flags_unchanged() {
        let graph = one_primary_graph();
        let mut start = HashMap::new();
        start.insert(0, ResolvedProfileFlags::default());
        let (out, parsed) =
            augment_build_flags_with_env(&graph, start.clone(), |_| None, quiet_reporter())
                .unwrap();
        assert!(parsed.is_empty());
        assert_eq!(out, start);
    }

    #[test]
    fn cppflags_land_in_extra_compile_args() {
        let graph = one_primary_graph();
        let mut start: HashMap<usize, ResolvedProfileFlags> = HashMap::new();
        start.insert(
            0,
            ResolvedProfileFlags {
                extra_compile_args: vec!["-fPIC".into()],
                ..Default::default()
            },
        );
        let env = env_from(&[("CPPFLAGS", "-DFOO=1 -DBAR")]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        let entry = out.get(&0).unwrap();
        assert_eq!(
            entry.extra_compile_args,
            vec!["-fPIC", "-DFOO=1", "-DBAR"],
            "CPPFLAGS append *after* existing language-neutral args"
        );
        assert!(entry.cflags.is_empty());
        assert!(entry.cxxflags.is_empty());
        assert!(entry.ldflags.is_empty());
    }

    #[test]
    fn cflags_only_reach_c_bucket() {
        let graph = one_primary_graph();
        let start: HashMap<usize, ResolvedProfileFlags> =
            HashMap::from_iter([(0, ResolvedProfileFlags::default())]);
        let env = env_from(&[("CFLAGS", "-std=c11 -Wmissing-prototypes")]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        let entry = out.get(&0).unwrap();
        assert!(entry.extra_compile_args.is_empty());
        assert_eq!(entry.cflags, vec!["-std=c11", "-Wmissing-prototypes"],);
        assert!(entry.cxxflags.is_empty());
    }

    #[test]
    fn cxxflags_only_reach_cxx_bucket() {
        let graph = one_primary_graph();
        let start: HashMap<usize, ResolvedProfileFlags> =
            HashMap::from_iter([(0, ResolvedProfileFlags::default())]);
        let env = env_from(&[("CXXFLAGS", "-fno-rtti")]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        let entry = out.get(&0).unwrap();
        assert!(entry.cflags.is_empty());
        assert_eq!(entry.cxxflags, vec!["-fno-rtti".to_owned()]);
    }

    #[test]
    fn ldflags_only_reach_link_bucket() {
        let graph = one_primary_graph();
        let start: HashMap<usize, ResolvedProfileFlags> = HashMap::from_iter([(
            0,
            ResolvedProfileFlags {
                ldflags: vec!["-Wl,--as-needed".into()],
                ..Default::default()
            },
        )]);
        let env = env_from(&[("LDFLAGS", "-L/opt/lib -lfoo")]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        let entry = out.get(&0).unwrap();
        // LDFLAGS append after existing link args; order is
        // load-bearing for the link line.
        assert_eq!(
            entry.ldflags,
            vec!["-Wl,--as-needed", "-L/opt/lib", "-lfoo"],
        );
        assert!(entry.extra_compile_args.is_empty());
        assert!(entry.cflags.is_empty());
        assert!(entry.cxxflags.is_empty());
    }

    #[test]
    fn malformed_quote_error_names_variable() {
        let graph = one_primary_graph();
        let start: HashMap<usize, ResolvedProfileFlags> = HashMap::new();
        let env = env_from(&[("CFLAGS", "-DFOO='hello")]);
        let err = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("CFLAGS"), "{msg}");
        assert!(msg.contains("shell"), "{msg}");
    }

    #[test]
    fn env_flag_append_preserves_pkg_config_order() {
        // pkg-config's contribution is already in
        // ResolvedProfileFlags before this helper is called.
        // Verify env flags land *after* it, preserving
        // pkg-config's link-line order.
        let graph = one_primary_graph();
        let start: HashMap<usize, ResolvedProfileFlags> = HashMap::from_iter([(
            0,
            ResolvedProfileFlags {
                extra_compile_args: vec!["-pthread".into()],
                ldflags: vec!["-L/usr/local/lib".into(), "-lssl".into()],
                ..Default::default()
            },
        )]);
        let env = env_from(&[
            ("CPPFLAGS", "-DENV_CPP=1"),
            ("LDFLAGS", "-L/opt/lib -lextra"),
        ]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        let entry = out.get(&0).unwrap();
        assert_eq!(entry.extra_compile_args, vec!["-pthread", "-DENV_CPP=1"],);
        assert_eq!(
            entry.ldflags,
            vec!["-L/usr/local/lib", "-lssl", "-L/opt/lib", "-lextra"],
        );
    }

    /// Multi-package primary set; ensures the merge touches
    /// every primary index (not just the root).
    #[test]
    fn merge_touches_every_primary_package() {
        let mut graph = one_primary_graph();
        graph.packages.push(make_pkg("worker"));
        graph.primary_packages = vec![0, 1];
        let start: HashMap<usize, ResolvedProfileFlags> = HashMap::new();
        let env = env_from(&[("CFLAGS", "-DSHARED")]);
        let (out, _) = augment_build_flags_with_env(&graph, start, env, quiet_reporter()).unwrap();
        for idx in [0, 1] {
            let e = out.get(&idx).unwrap();
            assert_eq!(e.cflags, vec!["-DSHARED".to_owned()]);
        }
    }
}
