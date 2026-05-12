//! Typed semantic build flags.
//!
//! Cabin recognises explicit, semantic build flags that compose
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
//! arguments, and link arguments. The C and C++ argv spaces stay
//! separate all the way to the planner.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
    pub include_dirs: Vec<PathBuf>,
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
}

impl ProfileFlags {
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.cflags.is_empty()
            && self.cxxflags.is_empty()
            && self.ldflags.is_empty()
    }

    /// Run the validation rules that apply at manifest parse time.
    ///
    /// - Defines must be non-empty and must not start with `=`.
    /// - Include directories must be relative and must not contain
    ///   any `..` component.
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
            validate_include_dir(dir)?;
        }
        Ok(())
    }
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
/// the existing planner behaviour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfileFlags {
    pub defines: Vec<String>,
    pub include_dirs: Vec<PathBuf>,
    /// Language-neutral compile-time escape-hatch arguments.
    /// Applied to every compile command, both C and C++.
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
}

impl ResolvedProfileFlags {
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.extra_compile_args.is_empty()
            && self.cflags.is_empty()
            && self.cxxflags.is_empty()
            && self.ldflags.is_empty()
    }

    /// Compact JSON view used by `cabin metadata`.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "defines": self.defines,
            "include_dirs": self
                .include_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "extra_compile_args": self.extra_compile_args,
            "cflags": self.cflags,
            "cxxflags": self.cxxflags,
            "ldflags": self.ldflags,
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
pub fn resolve_build_flags(
    package: &ProfileSettings,
    profile: Option<&ProfileFlags>,
    host_platform: &crate::condition::TargetPlatform,
) -> ResolvedProfileFlags {
    let mut out = ResolvedProfileFlags::default();

    apply_layer(&mut out, &package.general);
    for conditional in &package.conditional {
        if conditional.condition.evaluate(host_platform) {
            apply_layer(&mut out, &conditional.flags);
        }
    }
    if let Some(prof) = profile {
        apply_layer(&mut out, prof);
    }

    finalise(&mut out);
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
        // `finalise` step sort-and-dedups them once at the end, so a
        // second normalisation path inside the per-layer append would
        // be a double-pass on the resolved side and break the
        // semantics expected by the inherits-chain merge accumulator,
        // which is itself a layer for that same finalise step.
        target.defines.extend(layer.defines.iter().cloned());
        for inc in &layer.include_dirs {
            if !target.include_dirs.iter().any(|existing| existing == inc) {
                target.include_dirs.push(inc.clone());
            }
        }
        target.cflags.extend(layer.cflags.iter().cloned());
        target.cxxflags.extend(layer.cxxflags.iter().cloned());
        target.ldflags.extend(layer.ldflags.iter().cloned());
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

fn finalise(target: &mut ResolvedProfileFlags) {
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
        let r = resolve_build_flags(&p, None, &host_for("linux"));
        assert!(r.is_empty());
    }

    #[test]
    fn defines_merge_dedup_and_sort() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["B".into(), "A".into(), "B".into()];
        let r = resolve_build_flags(&p, None, &host_for("linux"));
        assert_eq!(r.defines, vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn include_dirs_keep_first_occurrence_order() {
        let mut p = ProfileSettings::default();
        p.general.include_dirs = vec![
            PathBuf::from("include"),
            PathBuf::from("third_party/include"),
            PathBuf::from("include"),
        ];
        let r = resolve_build_flags(&p, None, &host_for("linux"));
        assert_eq!(
            r.include_dirs,
            vec![
                PathBuf::from("include"),
                PathBuf::from("third_party/include"),
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
        let r = resolve_build_flags(&p, None, &host_for("linux"));
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
        let r = resolve_build_flags(&p, None, &host_for("linux"));
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
        let r = resolve_build_flags(&p, Some(&prof), &host_for("linux"));
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
    fn validate_rejects_absolute_include_dir() {
        let decl = ProfileFlags {
            include_dirs: vec![PathBuf::from("/etc/include")],
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
            include_dirs: vec![PathBuf::from("../sneaky")],
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
            defines: vec!["".into()],
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
