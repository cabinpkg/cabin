//! Typed semantic build flags.
//!
//! Cabin recognizes explicit, semantic build flags that compose
//! across four layers, in this order (later layers override or
//! append to earlier ones):
//!
//! 1. Built-in backend defaults (today: the planner adds
//!    `-std=c11` for C compiles and `-std=c++17` for C++
//!    compiles).
//! 2. Per-package general `[profile]` flags from the manifest.
//! 3. Per-package matching `[target.'cfg(...)'.profile]` flags.
//! 4. Workspace-root `[profile.<name>]` flags for the selected
//!    profile.
//!
//! Manifest-declared fields are intentionally explicit: defines,
//! include directories, C-only compile arguments, C++-only compile
//! arguments, and link arguments. The C/C++ argv spaces stay
//! separate all the way to the planner.

use std::collections::BTreeSet;
use std::path::Path;

use camino::Utf8PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::condition::Condition;

/// Manifest-shape build-flag declaration. One per `[profile]` /
/// `[target.'cfg(...)'.profile]` / `[profile.<name>]` table.
///
/// Every field is optional so omission means "no contribution at
/// this layer". The TOML parser rejects unknown fields explicitly
/// so a future field cannot silently slip through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFlags {
    /// Preprocessor macro definitions, one per entry. Each value
    /// is either `"NAME"` (defines without a value) or
    /// `"NAME=value"` (defines with an explicit value). Names are
    /// validated at parse time; the planner emits `-DNAME` /
    /// `-DNAME=value` directly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    /// Additional include directories. Paths are validated at
    /// parse time: absolute paths and any path containing a `..`
    /// component are rejected so include-search can never escape
    /// a published source archive.
    #[serde(
        default,
        rename = "include-dirs",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Escape-hatch list of arguments appended verbatim to every
    /// **C** compile command this layer applies to. Use this for
    /// flags that are valid only when compiling C translation
    /// units (e.g. `-std=c99`). Empty by default.
    #[serde(default, rename = "cflags", skip_serializing_if = "Vec::is_empty")]
    pub cflags: Vec<String>,
    /// Escape-hatch list of arguments appended verbatim to every
    /// **C++** compile command this layer applies to. Use this
    /// for flags that are valid only when compiling C++
    /// translation units (e.g. `-fno-rtti`, `-std=c++20`). Empty
    /// by default.
    #[serde(default, rename = "cxxflags", skip_serializing_if = "Vec::is_empty")]
    pub cxxflags: Vec<String>,
    /// Escape-hatch list of arguments appended verbatim to every
    /// link command this layer applies to.
    #[serde(default, rename = "ldflags", skip_serializing_if = "Vec::is_empty")]
    pub ldflags: Vec<String>,
    /// System libraries this target's objects require, as bare
    /// library names (e.g. `"pthread"`, `"dl"`, `"m"`). Unlike
    /// `ldflags` — which are raw, unvalidated, and applied only to
    /// the declaring package's own link — `link_libs` are validated
    /// safe library names that **propagate** to the final link of
    /// every executable that depends on this target (transitively),
    /// emitted as `-l<name>` after the archives so GNU `ld`'s
    /// left-to-right resolution finds them. Because they are
    /// validated (no leading `-`, no path separators, no spaces)
    /// they cannot inject linker flags, so they are kept even for
    /// untrusted (registry) packages.
    #[serde(default, rename = "link-libs", skip_serializing_if = "Vec::is_empty")]
    pub link_libs: Vec<String>,
}

impl ProfileFlags {
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.cflags.is_empty()
            && self.cxxflags.is_empty()
            && self.ldflags.is_empty()
            && self.link_libs.is_empty()
    }

    /// Run the validation rules that apply at manifest parse time.
    ///
    /// - Defines must be non-empty and must not start with `=`.
    /// - Include directories must be relative and must not contain
    ///   any `..` component.
    /// - Link libraries must be safe bare library names (see
    ///   [`is_safe_link_lib`]).
    ///
    /// # Errors
    /// Returns [`BuildFlagsValidationError::EmptyDefine`] for an empty define,
    /// [`BuildFlagsValidationError::DefineMissingName`] for a define starting
    /// with `=`, [`BuildFlagsValidationError::InvalidLinkLib`] for a malformed
    /// link-library name, and propagates any error from validating an include
    /// directory (a non-relative directory or one containing a `..` component).
    pub fn validate(&self) -> Result<(), BuildFlagsValidationError> {
        for define in &self.defines {
            if define.is_empty() {
                return Err(BuildFlagsValidationError::EmptyDefine);
            }
            if define.starts_with('=') {
                return Err(BuildFlagsValidationError::DefineMissingName {
                    raw: define.clone(),
                });
            }
        }
        for dir in &self.include_dirs {
            validate_include_dir(dir.as_std_path())?;
        }
        for lib in &self.link_libs {
            if !is_safe_link_lib(lib) {
                return Err(BuildFlagsValidationError::InvalidLinkLib { raw: lib.clone() });
            }
        }
        Ok(())
    }
}

/// Whether `name` is a safe bare library name for a `link-libs`
/// entry. The grammar is deliberately strict because `link_libs`
/// propagate to consumers' link lines and are kept even for
/// untrusted dependencies: a value that began with `-` or carried
/// a path / whitespace could smuggle a linker flag (`-Wl,...`,
/// `-fuse-ld=...`) or an arbitrary object path onto the link
/// command. The accepted set — an alphanumeric/underscore first
/// character followed by alphanumerics and `_`, `.`, `+`, `-` —
/// covers real library names like `pthread`, `dl`, `m`, `stdc++`,
/// and `c++` while rejecting everything that could be a flag or a
/// path.
pub fn is_safe_link_lib(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

/// Conditional `[target.'cfg(...)'.profile]` block. Same shape as
/// [`ProfileFlags`] but tagged with the predicate that gates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalProfileFlags {
    pub condition: Condition,
    #[serde(flatten, default, skip_serializing_if = "ProfileFlags::is_empty")]
    pub flags: ProfileFlags,
}

/// Per-package build-flags settings. Holds the unconditional
/// `[profile]` table plus any `[target.'cfg(...)'.profile]`
/// overrides declared in the same manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSettings {
    #[serde(default, skip_serializing_if = "ProfileFlags::is_empty")]
    pub general: ProfileFlags,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional: Vec<ConditionalProfileFlags>,
}

impl ProfileSettings {
    pub fn is_empty(&self) -> bool {
        self.general.is_empty() && self.conditional.is_empty()
    }
}

/// Final, deterministic build-flag set fed to the planner.
///
/// `defines` is sorted-and-deduplicated (defines are commutative
/// for our purposes); include and argv lists keep user-visible
/// order, with first-occurrence dedup for include dirs to mirror
/// the existing planner behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfileFlags {
    pub defines: Vec<String>,
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Language-neutral compile-time escape-hatch arguments.
    /// Applied to every compile command, both C/C++.
    pub extra_compile_args: Vec<String>,
    /// C-only compile-time escape-hatch arguments. Applied only
    /// when the compile command produces an object from a `.c`
    /// translation unit.
    pub cflags: Vec<String>,
    /// C++-only compile-time escape-hatch arguments. Applied only
    /// when the compile command produces an object from a C++
    /// translation unit (`.cc` / `.cpp` / `.cxx` / `.c++` /
    /// `.C`).
    pub cxxflags: Vec<String>,
    pub ldflags: Vec<String>,
    /// Validated bare system-library names that propagate to the
    /// link of every executable depending on this package. The
    /// build planner walks the dependency closure, collects these,
    /// and emits `-l<name>` (after the archives) on the consumer's
    /// link command.
    pub link_libs: Vec<String>,
}

impl ResolvedProfileFlags {
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.extra_compile_args.is_empty()
            && self.cflags.is_empty()
            && self.cxxflags.is_empty()
            && self.ldflags.is_empty()
            && self.link_libs.is_empty()
    }

    /// Compact JSON view used by `cabin metadata`.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "defines": self.defines,
            "include_dirs": self
                .include_dirs
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect::<Vec<_>>(),
            "extra_compile_args": self.extra_compile_args,
            "cflags": self.cflags,
            "cxxflags": self.cxxflags,
            "ldflags": self.ldflags,
            "link_libs": self.link_libs,
        })
    }
}

/// Resolve build flags by merging the per-package and
/// per-profile layers, in order.
///
/// `package` is the package's own `[profile]` /
/// `[target.'cfg(...)'.profile]` settings. `profile` is the
/// **already-merged-across-inherits-chain** per-profile
/// `ProfileFlags` produced by
/// [`crate::profile::resolve_profile`] — *not* the lone overlay
/// from the selected profile's `[profile.<name>]` table. The
/// inherits-chain merge has already happened upstream via
/// `ProfileFlags::append_layer`, so this layer simply lands
/// on top of `package.general` and the matching conditional
/// flags. `host_platform` is what the conditional layer
/// evaluates against — passing the same `TargetPlatform` Cabin
/// uses elsewhere keeps the cfg semantics consistent with
/// target dependencies.
///
/// `package_trusted` says whether `package` comes from code the
/// user controls — the workspace root, a member, or a `path`
/// dependency. When it is `false` (a registry / downloaded
/// dependency) the `cflags` / `cxxflags` / `ldflags` arrays that
/// `package` declares for its own sources are dropped before the
/// trusted `profile` layer is applied: those arrays are
/// unvalidated and a `-fplugin=` / `-B<dir>` / `-specs=` /
/// `-Xclang -load` / `-fuse-ld=<path>` entry would make the
/// compiler or linker execute attacker-supplied code at build
/// time. `defines` / `include_dirs` are validated at parse time
/// (see [`ProfileFlags::validate`]) and are kept regardless of
/// trust, and the `profile` layer — the trusted, root-derived
/// flags — always applies so an untrusted dependency still builds
/// with the user's selected profile.
pub fn resolve_build_flags(
    package: &ProfileSettings,
    profile: Option<&ProfileFlags>,
    host_platform: &crate::condition::TargetPlatform,
    enabled_features: &BTreeSet<String>,
    package_trusted: bool,
) -> ResolvedProfileFlags {
    let mut out = ResolvedProfileFlags::default();

    apply_layer(&mut out, &package.general);
    for conditional in &package.conditional {
        if conditional
            .condition
            .evaluate(host_platform, enabled_features)
        {
            apply_layer(&mut out, &conditional.flags);
        }
    }
    if !package_trusted {
        // Untrusted (registry) dependency: discard the compiler /
        // linker flag arrays it declared for its own sources. These
        // are unvalidated, so a `-fplugin=` / `-B<dir>` / `-specs=`
        // / `-Xclang -load` entry would run attacker code inside the
        // compiler or linker during `cabin build`. `defines` /
        // `include_dirs` / `link_libs` are validated elsewhere
        // (see `ProfileFlags::validate` and `is_safe_link_lib`) and
        // stay — a `link_libs` entry cannot be a flag or a path, so
        // it cannot inject; the trusted `profile` layer below still
        // applies.
        out.cflags.clear();
        out.cxxflags.clear();
        out.ldflags.clear();
    }
    if let Some(prof) = profile {
        apply_layer(&mut out, prof);
    }

    finalize(&mut out);
    out
}

/// Append every field of a [`ProfileFlags`] layer into a target
/// whose fields are structurally identical to `ProfileFlags` —
/// either a [`ProfileFlags`] accumulator (used by the
/// inherits-chain merge in
/// [`crate::profile::resolve_profile`]) or a
/// [`ResolvedProfileFlags`] accumulator (used by
/// [`resolve_build_flags`]'s package / conditional / profile
/// layer chain).
///
/// One canonical field list lives here so a future array field
/// added to [`ProfileFlags`] needs exactly one update site.
/// Both [`ProfileFlags::append_layer`] and [`apply_layer`]
/// delegate to this macro; do not duplicate the per-field walk
/// elsewhere.
macro_rules! append_profile_flag_layer {
    ($target:expr, $layer:expr) => {{
        let target = $target;
        let layer = $layer;
        // `defines` are appended verbatim here. `resolve_build_flags`'s
        // `finalize` step sort-and-dedups them once at the end, so a
        // second normalization path inside the per-layer append would
        // be a double-pass on the resolved side and break the
        // semantics expected by the inherits-chain merge accumulator,
        // which is itself a layer for that same finalize step.
        target.defines.extend(layer.defines.iter().cloned());
        for inc in &layer.include_dirs {
            if !target.include_dirs.iter().any(|existing| existing == inc) {
                target.include_dirs.push(inc.clone());
            }
        }
        target.cflags.extend(layer.cflags.iter().cloned());
        target.cxxflags.extend(layer.cxxflags.iter().cloned());
        target.ldflags.extend(layer.ldflags.iter().cloned());
        // Link libraries dedup by first occurrence and keep order:
        // `-l` resolution is order-sensitive, and a duplicate `-lm`
        // is noise, so we mirror the include-dir treatment rather
        // than the append-verbatim used for the raw flag arrays.
        for lib in &layer.link_libs {
            if !target.link_libs.iter().any(|existing| existing == lib) {
                target.link_libs.push(lib.clone());
            }
        }
    }};
}

impl ProfileFlags {
    /// Append every field of `layer` into `self`, using the
    /// same per-field semantics as the package / conditional /
    /// profile layer chain in [`resolve_build_flags`].
    ///
    /// The merged accumulator is what
    /// [`crate::profile::resolve_profile`] builds when it walks
    /// a custom profile's `inherits` chain root → selected.
    /// Consumers downstream feed the resulting `ProfileFlags`
    /// to [`resolve_build_flags`] as the `profile` parameter,
    /// where it lands on top of the per-package general /
    /// conditional layers via the same macro.
    pub(crate) fn append_layer(&mut self, layer: &ProfileFlags) {
        append_profile_flag_layer!(self, layer);
    }
}

fn apply_layer(target: &mut ResolvedProfileFlags, layer: &ProfileFlags) {
    append_profile_flag_layer!(target, layer);
}

fn finalize(target: &mut ResolvedProfileFlags) {
    // Defines are commutative: `-DA -DB` and `-DB -DA` produce the
    // same preprocessor state, so a stable sort + dedup gives us a
    // deterministic shape that does not depend on declaration
    // order across layers.
    let dedup: BTreeSet<String> = target.defines.drain(..).collect();
    target.defines = dedup.into_iter().collect();
    // Include dirs already deduplicated by `apply_layer` while
    // preserving first-seen order; nothing more to do here.
    // Argument lists preserve user order; no sorting.
}

/// Errors produced while validating a manifest-side build-flags
/// declaration.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BuildFlagsValidationError {
    #[error("[profile] declares an empty define entry")]
    EmptyDefine,
    #[error("[profile] define entry {raw:?} is missing a name")]
    DefineMissingName { raw: String },
    #[error(
        "[profile] link library {raw:?} is not a valid library name; use a bare name like \"pthread\" (no leading `-`, path separators, or whitespace)"
    )]
    InvalidLinkLib { raw: String },
    #[error(
        "[profile] include directory {path:?} must be a relative path; absolute paths are not allowed"
    )]
    AbsoluteIncludeDir { path: String },
    #[error(
        "[profile] include directory {path:?} must not contain `..`; include search paths cannot escape the package root"
    )]
    IncludeDirHasParent { path: String },
    #[error("[profile] include directory {path:?} contains a non-UTF-8 component")]
    NonUtf8IncludeDir { path: String },
}

fn validate_include_dir(dir: &Path) -> Result<(), BuildFlagsValidationError> {
    if dir.is_absolute() {
        return Err(BuildFlagsValidationError::AbsoluteIncludeDir {
            path: display_path(dir),
        });
    }
    for component in dir.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(BuildFlagsValidationError::IncludeDirHasParent {
                    path: display_path(dir),
                });
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err(BuildFlagsValidationError::AbsoluteIncludeDir {
                    path: display_path(dir),
                });
            }
            std::path::Component::Normal(part) => {
                if part.to_str().is_none() {
                    return Err(BuildFlagsValidationError::NonUtf8IncludeDir {
                        path: display_path(dir),
                    });
                }
            }
            std::path::Component::CurDir => {}
        }
    }
    Ok(())
}

fn display_path(dir: &Path) -> String {
    dir.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{ConditionKey, TargetPlatform};

    fn host_for(os: &str) -> TargetPlatform {
        let mut p = TargetPlatform::current();
        p.os = os.to_owned();
        p
    }

    #[test]
    fn empty_settings_resolve_to_empty_flags() {
        let p = ProfileSettings::default();
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert!(r.is_empty());
    }

    #[test]
    fn defines_merge_dedup_and_sort() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["B".into(), "A".into(), "B".into()];
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(r.defines, vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn include_dirs_keep_first_occurrence_order() {
        let mut p = ProfileSettings::default();
        p.general.include_dirs = vec![
            Utf8PathBuf::from("include"),
            Utf8PathBuf::from("third_party/include"),
            Utf8PathBuf::from("include"),
        ];
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(
            r.include_dirs,
            vec![
                Utf8PathBuf::from("include"),
                Utf8PathBuf::from("third_party/include"),
            ]
        );
    }

    #[test]
    fn matching_conditional_layer_is_applied() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["BASE".into()];
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::KeyValue {
                key: ConditionKey::Os,
                value: "linux".into(),
            },
            flags: ProfileFlags {
                defines: vec!["LINUX_ONLY".into()],
                ..Default::default()
            },
        });
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(r.defines, vec!["BASE".to_owned(), "LINUX_ONLY".to_owned()]);
    }

    #[test]
    fn non_matching_conditional_layer_is_skipped() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["BASE".into()];
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::KeyValue {
                key: ConditionKey::Os,
                value: "macos".into(),
            },
            flags: ProfileFlags {
                defines: vec!["MAC_ONLY".into()],
                ..Default::default()
            },
        });
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(r.defines, vec!["BASE".to_owned()]);
    }

    #[test]
    fn profile_layer_appends_after_target_conditional() {
        let mut p = ProfileSettings::default();
        p.general.cxxflags = vec!["-fPIC".into()];
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::KeyValue {
                key: ConditionKey::Os,
                value: "linux".into(),
            },
            flags: ProfileFlags {
                cxxflags: vec!["-flto=thin".into()],
                ..Default::default()
            },
        });
        let prof = ProfileFlags {
            cxxflags: vec!["-Wall".into()],
            ..Default::default()
        };
        let r = resolve_build_flags(&p, Some(&prof), &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(
            r.cxxflags,
            vec![
                "-fPIC".to_owned(),
                "-flto=thin".to_owned(),
                "-Wall".to_owned(),
            ]
        );
    }

    #[test]
    fn untrusted_package_drops_command_flags_but_keeps_defines_and_includes() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["DEP_DEFINE".into()];
        p.general.include_dirs = vec![Utf8PathBuf::from("dep/include")];
        p.general.cflags = vec!["-fplugin=evil.so".into()];
        p.general.cxxflags = vec!["-Xclang".into(), "-load".into()];
        p.general.ldflags = vec!["-fuse-ld=/tmp/evil".into()];
        // A matching conditional layer must not be able to sneak flags past
        // the drop either.
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::KeyValue {
                key: ConditionKey::Os,
                value: "linux".into(),
            },
            flags: ProfileFlags {
                cxxflags: vec!["-B.".into()],
                ldflags: vec!["-specs=evil.specs".into()],
                ..Default::default()
            },
        });

        let untrusted = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), false);
        assert!(
            untrusted.cflags.is_empty(),
            "untrusted cflags must be dropped"
        );
        assert!(
            untrusted.cxxflags.is_empty(),
            "untrusted cxxflags must be dropped"
        );
        assert!(
            untrusted.ldflags.is_empty(),
            "untrusted ldflags must be dropped"
        );
        // Validated, non-injection fields survive so dependencies can still
        // declare their own defines / include search paths.
        assert_eq!(untrusted.defines, vec!["DEP_DEFINE".to_owned()]);
        assert_eq!(
            untrusted.include_dirs,
            vec![Utf8PathBuf::from("dep/include")]
        );

        // The very same settings are kept verbatim for a trusted package.
        let trusted = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert_eq!(trusted.cflags, vec!["-fplugin=evil.so".to_owned()]);
        assert_eq!(
            trusted.cxxflags,
            vec!["-Xclang".to_owned(), "-load".to_owned(), "-B.".to_owned()]
        );
        assert_eq!(
            trusted.ldflags,
            vec![
                "-fuse-ld=/tmp/evil".to_owned(),
                "-specs=evil.specs".to_owned()
            ]
        );
    }

    #[test]
    fn untrusted_package_still_receives_trusted_profile_layer() {
        let mut p = ProfileSettings::default();
        p.general.cxxflags = vec!["-fplugin=evil.so".into()];
        let prof = ProfileFlags {
            cxxflags: vec!["-O2".into()],
            ldflags: vec!["-s".into()],
            ..Default::default()
        };
        let r = resolve_build_flags(&p, Some(&prof), &host_for("linux"), &BTreeSet::new(), false);
        // The dependency's own flag is dropped, but the trusted root profile
        // layer is still applied so the dependency builds with the user's
        // selected flags.
        assert_eq!(r.cxxflags, vec!["-O2".to_owned()]);
        assert_eq!(r.ldflags, vec!["-s".to_owned()]);
    }

    #[test]
    fn feature_conditional_layer_gated_by_enabled_features() {
        // `[target.'cfg(feature = "single-threaded")'.profile]
        //  defines = ["SQLITE_THREADSAFE=0"]` applies iff the feature
        // is enabled — the sqlite threadsafe-toggle wiring.
        let mut p = ProfileSettings::default();
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::Feature("single-threaded".into()),
            flags: ProfileFlags {
                defines: vec!["SQLITE_THREADSAFE=0".into()],
                ..Default::default()
            },
        });
        let enabled: BTreeSet<String> = BTreeSet::from(["single-threaded".to_owned()]);
        let on = resolve_build_flags(&p, None, &host_for("linux"), &enabled, true);
        assert_eq!(on.defines, vec!["SQLITE_THREADSAFE=0".to_owned()]);
        let off = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), true);
        assert!(
            off.defines.is_empty(),
            "feature-off must not apply the layer: {:?}",
            off.defines
        );
    }

    #[test]
    fn link_libs_merge_dedup_preserving_order() {
        let mut p = ProfileSettings::default();
        p.general.link_libs = vec!["pthread".into(), "m".into()];
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::KeyValue {
                key: ConditionKey::Family,
                value: "unix".into(),
            },
            flags: ProfileFlags {
                link_libs: vec!["dl".into(), "m".into()],
                ..Default::default()
            },
        });
        let mut host = host_for("linux");
        host.family = "unix".into();
        let r = resolve_build_flags(&p, None, &host, &BTreeSet::new(), true);
        assert_eq!(
            r.link_libs,
            vec!["pthread".to_owned(), "m".to_owned(), "dl".to_owned()]
        );
    }

    #[test]
    fn link_libs_survive_untrusted_packages() {
        // Unlike ldflags, validated link_libs are kept for untrusted
        // (registry) packages because they cannot smuggle a flag.
        let mut p = ProfileSettings::default();
        p.general.link_libs = vec!["pthread".into()];
        p.general.ldflags = vec!["-fuse-ld=/tmp/evil".into()];
        let r = resolve_build_flags(&p, None, &host_for("linux"), &BTreeSet::new(), false);
        assert_eq!(r.link_libs, vec!["pthread".to_owned()]);
        assert!(r.ldflags.is_empty(), "untrusted ldflags must be dropped");
    }

    #[test]
    fn validate_rejects_flag_like_link_lib() {
        for bad in ["-lm", "-Wl,--foo", "../escape", "a/b", "has space", ""] {
            let decl = ProfileFlags {
                link_libs: vec![bad.into()],
                ..Default::default()
            };
            assert!(
                matches!(
                    decl.validate(),
                    Err(BuildFlagsValidationError::InvalidLinkLib { .. })
                ),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn validate_accepts_real_link_lib_names() {
        let decl = ProfileFlags {
            link_libs: vec!["pthread".into(), "dl".into(), "m".into(), "stdc++".into()],
            ..Default::default()
        };
        assert!(decl.validate().is_ok());
    }

    #[test]
    fn validate_rejects_absolute_include_dir() {
        let decl = ProfileFlags {
            include_dirs: vec![Utf8PathBuf::from("/etc/include")],
            ..Default::default()
        };
        let err = decl.validate().unwrap_err();
        assert!(matches!(
            err,
            BuildFlagsValidationError::AbsoluteIncludeDir { .. }
        ));
    }

    #[test]
    fn validate_rejects_parent_traversal_include_dir() {
        let decl = ProfileFlags {
            include_dirs: vec![Utf8PathBuf::from("../sneaky")],
            ..Default::default()
        };
        let err = decl.validate().unwrap_err();
        assert!(matches!(
            err,
            BuildFlagsValidationError::IncludeDirHasParent { .. }
        ));
    }

    #[test]
    fn validate_rejects_empty_define() {
        let decl = ProfileFlags {
            defines: vec![String::new()],
            ..Default::default()
        };
        assert!(matches!(
            decl.validate().unwrap_err(),
            BuildFlagsValidationError::EmptyDefine
        ));
    }

    #[test]
    fn validate_rejects_define_missing_name() {
        let decl = ProfileFlags {
            defines: vec!["=oops".into()],
            ..Default::default()
        };
        assert!(matches!(
            decl.validate().unwrap_err(),
            BuildFlagsValidationError::DefineMissingName { .. }
        ));
    }
}
