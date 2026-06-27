//! Typed semantic build flags.
//!
//! Cabin recognizes explicit, semantic build flags that compose
//! across these layers, in order:
//!
//! 1. Built-in backend defaults (today: the planner adds
//!    `-std=c11` for C compiles and `-std=c++17` for C++
//!    compiles).
//! 2. Per-package general `[profile]` flags from the manifest.
//! 3. Per-package matching `[target.'cfg(...)'.profile]` flags.
//! 4. For each profile in the selected root-to-leaf inheritance
//!    chain, workspace-root `[profile.<name>]` flags followed by
//!    matching package
//!    `[target.'cfg(...)'.profile.<name>]` overlays.
//!
//! Manifest-declared fields are intentionally explicit: defines,
//! include directories, C-only compile arguments, C++-only compile
//! arguments, and link arguments.  The C/C++ argv spaces stay
//! separate all the way to the planner.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use camino::Utf8PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::condition::Condition;
use crate::profile::{ProfileDefinition, ProfileName, ResolvedProfile};

/// Manifest-shape build-flag declaration.  One per `[profile]` /
/// `[target.'cfg(...)'.profile]` / `[profile.<name>]` table.
///
/// Every field is optional so omission means "no contribution at
/// this layer".  The TOML parser rejects unknown fields explicitly
/// so a future field cannot silently slip through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFlags {
    /// Preprocessor macro definitions, one per entry.  Each value
    /// is either `"NAME"` (defines without a value) or
    /// `"NAME=value"` (defines with an explicit value).  Names are
    /// validated at parse time; the planner emits `-DNAME` /
    /// `-DNAME=value` directly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defines: Vec<String>,
    /// Additional include directories.  Paths are validated at
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
    /// **C** compile command this layer applies to.  Use this for
    /// flags that are valid only when compiling C translation
    /// units (e.g. `-std=c99`).  Empty by default.
    #[serde(default, rename = "cflags", skip_serializing_if = "Vec::is_empty")]
    pub cflags: Vec<String>,
    /// Escape-hatch list of arguments appended verbatim to every
    /// **C++** compile command this layer applies to.  Use this
    /// for flags that are valid only when compiling C++
    /// translation units (e.g. `-fno-rtti`, `-std=c++20`).  Empty
    /// by default.
    #[serde(default, rename = "cxxflags", skip_serializing_if = "Vec::is_empty")]
    pub cxxflags: Vec<String>,
    /// Escape-hatch list of arguments appended verbatim to every
    /// link command this layer applies to.
    #[serde(default, rename = "ldflags", skip_serializing_if = "Vec::is_empty")]
    pub ldflags: Vec<String>,
    /// System libraries this target's objects require, as bare
    /// library names (e.g. `"pthread"`, `"dl"`, `"m"`).  Unlike
    /// `ldflags` - which are raw, unvalidated, and applied only to
    /// the declaring package's own link - `link_libs` are validated
    /// safe library names that **propagate** to the final link of
    /// every executable that depends on this target (transitively),
    /// emitted as `-l<name>` after the archives so GNU `ld`'s
    /// left-to-right resolution finds them.  Because they are
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
/// entry.  The grammar is deliberately strict because `link_libs`
/// propagate to consumers' link lines and are kept even for
/// untrusted dependencies: a value that began with `-` or carried
/// a path / whitespace could smuggle a linker flag (`-Wl,...`,
/// `-fuse-ld=...`) or an arbitrary object path onto the link
/// command.  The accepted set - an alphanumeric/underscore first
/// character followed by alphanumerics and `_`, `.`, `+`, `-` -
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

/// Conditional `[target.'cfg(...)'.profile]` or
/// `[target.'cfg(...)'.profile.<name>]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalProfileFlags {
    pub condition: Condition,
    /// `None` for a general target profile layer; `Some` for a named
    /// overlay that applies when the selected profile chain contains it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileName>,
    #[serde(flatten, default, skip_serializing_if = "ProfileFlags::is_empty")]
    pub flags: ProfileFlags,
}

/// Per-package build-flags settings.  Holds the unconditional
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
    /// Include directories the compile commands mark as *system*
    /// search paths (`-isystem` in the GCC/Clang dialect), so
    /// diagnostics inside their headers are suppressed.  Populated
    /// from third-party contributions the user does not control -
    /// today the `pkg-config` probe of `system = true`
    /// dependencies - never from the package's own manifest
    /// declarations, which stay in [`Self::include_dirs`].
    pub system_include_dirs: Vec<Utf8PathBuf>,
    /// Language-neutral compile-time escape-hatch arguments.
    /// Applied to every compile command, both C/C++.
    pub extra_compile_args: Vec<String>,
    /// C-only compile-time escape-hatch arguments.  Applied only
    /// when the compile command produces an object from a `.c`
    /// translation unit.
    pub cflags: Vec<String>,
    /// C++-only compile-time escape-hatch arguments.  Applied only
    /// when the compile command produces an object from a C++
    /// translation unit (`.cc` / `.cpp` / `.cxx` / `.c++` /
    /// `.C`).
    pub cxxflags: Vec<String>,
    pub ldflags: Vec<String>,
    /// Validated bare system-library names that propagate to the
    /// link of every executable depending on this package.  The
    /// build planner walks the dependency closure, collects these,
    /// and emits `-l<name>` (after the archives) on the consumer's
    /// link command.
    pub link_libs: Vec<String>,
}

impl ResolvedProfileFlags {
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.system_include_dirs.is_empty()
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
            "system_include_dirs": self
                .system_include_dirs
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

/// Resolve build flags by merging package-owned layers and the selected
/// workspace profile chain in documented order.
///
/// `profile` is the resolved selection and `definitions` is the same
/// workspace-root definition map used to resolve it. Passing `None` skips
/// ordinary and named profile-specific layers.
///
/// `ctx` is the platform / feature / detected-compiler context the
/// conditional layer evaluates against - passing the same
/// [`crate::ConditionContext`] inputs Cabin uses elsewhere keeps
/// the cfg semantics consistent with target dependencies.
///
/// `package_trusted` says whether `package` comes from code the
/// user controls - the workspace root, a member, or a `path`
/// dependency.  When it is `false` (a registry / downloaded
/// dependency) the `cflags` / `cxxflags` / `ldflags` arrays that
/// `package` declares for its own sources are dropped from every
/// package-owned layer: those arrays are
/// unvalidated and a `-fplugin=` / `-B<dir>` / `-specs=` /
/// `-Xclang -load` / `-fuse-ld=<path>` entry would make the
/// compiler or linker execute attacker-supplied code at build
/// time. `defines` / `include_dirs` are validated at parse time
/// (see [`ProfileFlags::validate`]) and are kept regardless of trust.
/// Workspace-root profile definitions are trusted and always apply.
pub fn resolve_build_flags(
    package: &ProfileSettings,
    profile: Option<&ResolvedProfile>,
    definitions: &BTreeMap<ProfileName, ProfileDefinition>,
    ctx: &crate::condition::ConditionContext<'_>,
    package_trusted: bool,
) -> ResolvedProfileFlags {
    let mut out = ResolvedProfileFlags::default();

    apply_package_layer(&mut out, &package.general, package_trusted);
    for conditional in package
        .conditional
        .iter()
        .filter(|layer| layer.profile.is_none() && layer.condition.evaluate(ctx))
    {
        apply_package_layer(&mut out, &conditional.flags, package_trusted);
    }
    if let Some(profile) = profile {
        for name in &profile.inherits_chain {
            if let Some(flags) = definitions
                .get(name)
                .and_then(|definition| definition.build.as_ref())
            {
                apply_layer(&mut out, flags);
            }
            for conditional in package.conditional.iter().filter(|layer| {
                layer.profile.as_ref() == Some(name) && layer.condition.evaluate(ctx)
            }) {
                apply_package_layer(&mut out, &conditional.flags, package_trusted);
            }
        }
    }

    finalize(&mut out);
    out
}

fn apply_package_layer(
    out: &mut ResolvedProfileFlags,
    layer: &ProfileFlags,
    package_trusted: bool,
) {
    if package_trusted {
        apply_layer(out, layer);
        return;
    }
    let mut safe = layer.clone();
    safe.cflags.clear();
    safe.cxxflags.clear();
    safe.ldflags.clear();
    apply_layer(out, &safe);
}

/// Append every field of a [`ProfileFlags`] layer into a target
/// whose fields are structurally identical to `ProfileFlags` -
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
    /// [`resolve_build_flags`] walks the chain and original
    /// definitions separately so named conditional overlays can
    /// be interleaved at each profile step.
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
    use crate::compiler::{CompilerIdentity, CompilerKind, CompilerVersion};
    use crate::condition::{ConditionContext, ConditionKey, TargetPlatform};
    use crate::profile::{
        ProfileDefinition, ProfileName, ProfileSelection, ResolvedProfile, resolve_profile,
    };
    use std::collections::BTreeMap;

    fn host_for(os: &str) -> TargetPlatform {
        let mut p = TargetPlatform::current();
        p.os = os.to_owned();
        p
    }

    fn profile_name(value: &str) -> ProfileName {
        ProfileName::new(value).unwrap()
    }

    fn profile_definition(
        name: &str,
        inherits: Option<&str>,
        ldflags: &[&str],
    ) -> (ProfileName, ProfileDefinition) {
        let name = profile_name(name);
        (
            name.clone(),
            ProfileDefinition {
                name,
                inherits: inherits.map(profile_name),
                debug: None,
                opt_level: None,
                assertions: None,
                build: Some(ProfileFlags {
                    ldflags: ldflags.iter().map(|flag| (*flag).to_owned()).collect(),
                    ..Default::default()
                }),
            },
        )
    }

    fn profile_definitions() -> BTreeMap<ProfileName, ProfileDefinition> {
        BTreeMap::from([
            profile_definition("release", None, &["release"]),
            profile_definition("static", Some("release"), &["static"]),
        ])
    }

    fn selected_profile(
        name: &str,
        definitions: &BTreeMap<ProfileName, ProfileDefinition>,
    ) -> ResolvedProfile {
        resolve_profile(
            &ProfileSelection::from_name(profile_name(name)),
            definitions,
        )
        .unwrap()
    }

    fn os_condition(os: &str) -> Condition {
        Condition::KeyValue {
            key: ConditionKey::Os,
            value: os.to_owned(),
        }
    }

    fn resolve_without_profile(
        package: &ProfileSettings,
        ctx: &ConditionContext<'_>,
        package_trusted: bool,
    ) -> ResolvedProfileFlags {
        resolve_build_flags(package, None, &BTreeMap::new(), ctx, package_trusted)
    }

    #[test]
    fn named_target_profile_layers_interleave_with_profile_chain() {
        let definitions = profile_definitions();
        let selected = selected_profile("static", &definitions);
        let mut settings = ProfileSettings::default();
        settings.general.ldflags = vec!["base".into()];
        settings.conditional = vec![
            ConditionalProfileFlags {
                condition: os_condition("linux"),
                profile: None,
                flags: ProfileFlags {
                    ldflags: vec!["linux-base".into()],
                    ..Default::default()
                },
            },
            ConditionalProfileFlags {
                condition: os_condition("linux"),
                profile: Some(profile_name("release")),
                flags: ProfileFlags {
                    ldflags: vec!["linux-release".into()],
                    ..Default::default()
                },
            },
            ConditionalProfileFlags {
                condition: os_condition("linux"),
                profile: Some(profile_name("static")),
                flags: ProfileFlags {
                    ldflags: vec!["linux-static".into()],
                    ..Default::default()
                },
            },
        ];

        let resolved = resolve_build_flags(
            &settings,
            Some(&selected),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert_eq!(
            resolved.ldflags,
            vec![
                "base",
                "linux-base",
                "release",
                "linux-release",
                "static",
                "linux-static",
            ],
        );
    }

    #[test]
    fn named_target_profile_layers_require_profile_and_target_matches() {
        let definitions = profile_definitions();
        let mut settings = ProfileSettings::default();
        settings.conditional = vec![
            ConditionalProfileFlags {
                condition: os_condition("linux"),
                profile: Some(profile_name("release")),
                flags: ProfileFlags {
                    ldflags: vec!["linux-release".into()],
                    ..Default::default()
                },
            },
            ConditionalProfileFlags {
                condition: os_condition("linux"),
                profile: Some(profile_name("undeclared")),
                flags: ProfileFlags {
                    ldflags: vec!["inert".into()],
                    ..Default::default()
                },
            },
        ];

        let release = selected_profile("release", &definitions);
        let release_linux = resolve_build_flags(
            &settings,
            Some(&release),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert_eq!(release_linux.ldflags, vec!["release", "linux-release"]);

        let static_profile = selected_profile("static", &definitions);
        let static_linux = resolve_build_flags(
            &settings,
            Some(&static_profile),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert_eq!(
            static_linux.ldflags,
            vec!["release", "linux-release", "static"],
        );

        let dev = selected_profile("dev", &definitions);
        let dev_linux = resolve_build_flags(
            &settings,
            Some(&dev),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert!(dev_linux.ldflags.is_empty());

        let static_macos = resolve_build_flags(
            &settings,
            Some(&static_profile),
            &definitions,
            &ConditionContext::platform_only(&host_for("macos")),
            true,
        );
        assert_eq!(static_macos.ldflags, vec!["release", "static"]);
    }

    #[test]
    fn matching_named_target_profile_layers_keep_manifest_order() {
        let definitions = profile_definitions();
        let release = selected_profile("release", &definitions);
        let mut host = host_for("linux");
        host.arch = "x86_64".into();
        let settings = ProfileSettings {
            conditional: vec![
                ConditionalProfileFlags {
                    condition: os_condition("linux"),
                    profile: Some(profile_name("release")),
                    flags: ProfileFlags {
                        ldflags: vec!["linux".into()],
                        ..Default::default()
                    },
                },
                ConditionalProfileFlags {
                    condition: Condition::KeyValue {
                        key: ConditionKey::Arch,
                        value: "x86_64".into(),
                    },
                    profile: Some(profile_name("release")),
                    flags: ProfileFlags {
                        ldflags: vec!["x86_64".into()],
                        ..Default::default()
                    },
                },
            ],
            ..Default::default()
        };

        let resolved = resolve_build_flags(
            &settings,
            Some(&release),
            &definitions,
            &ConditionContext::platform_only(&host),
            true,
        );
        assert_eq!(resolved.ldflags, vec!["release", "linux", "x86_64"]);
    }

    #[test]
    fn empty_settings_resolve_to_empty_flags() {
        let p = ProfileSettings::default();
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert!(r.is_empty());
    }

    #[test]
    fn defines_merge_dedup_and_sort() {
        let mut p = ProfileSettings::default();
        p.general.defines = vec!["B".into(), "A".into(), "B".into()];
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
            profile: None,
            flags: ProfileFlags {
                defines: vec!["LINUX_ONLY".into()],
                ..Default::default()
            },
        });
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
            profile: None,
            flags: ProfileFlags {
                defines: vec!["MAC_ONLY".into()],
                ..Default::default()
            },
        });
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
            profile: None,
            flags: ProfileFlags {
                cxxflags: vec!["-flto=thin".into()],
                ..Default::default()
            },
        });
        let prof = ProfileFlags {
            cxxflags: vec!["-Wall".into()],
            ..Default::default()
        };
        let release_name = profile_name("release");
        let definitions = BTreeMap::from([(
            release_name.clone(),
            ProfileDefinition {
                name: release_name,
                inherits: None,
                debug: None,
                opt_level: None,
                assertions: None,
                build: Some(prof),
            },
        )]);
        let selected = selected_profile("release", &definitions);
        let r = resolve_build_flags(
            &p,
            Some(&selected),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
            profile: None,
            flags: ProfileFlags {
                cxxflags: vec!["-B.".into()],
                ldflags: vec!["-specs=evil.specs".into()],
                ..Default::default()
            },
        });

        let untrusted = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            false,
        );
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
        let trusted = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
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
        let release_name = profile_name("release");
        let definitions = BTreeMap::from([(
            release_name.clone(),
            ProfileDefinition {
                name: release_name,
                inherits: None,
                debug: None,
                opt_level: None,
                assertions: None,
                build: Some(prof),
            },
        )]);
        let selected = selected_profile("release", &definitions);
        let r = resolve_build_flags(
            &p,
            Some(&selected),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            false,
        );
        // The dependency's own flag is dropped, but the trusted root profile
        // layer is still applied so the dependency builds with the user's
        // selected flags.
        assert_eq!(r.cxxflags, vec!["-O2".to_owned()]);
        assert_eq!(r.ldflags, vec!["-s".to_owned()]);
    }

    #[test]
    fn untrusted_named_overlay_drops_command_flags_but_keeps_safe_fields() {
        let mut package = ProfileSettings::default();
        package.conditional.push(ConditionalProfileFlags {
            condition: os_condition("linux"),
            profile: Some(profile_name("release")),
            flags: ProfileFlags {
                defines: vec!["SAFE_NAMED_OVERLAY".into()],
                cxxflags: vec!["-B.".into()],
                ldflags: vec!["-specs=evil.specs".into()],
                ..Default::default()
            },
        });
        let release_name = profile_name("release");
        let definitions = BTreeMap::from([(
            release_name.clone(),
            ProfileDefinition {
                name: release_name,
                inherits: None,
                debug: None,
                opt_level: None,
                assertions: None,
                build: Some(ProfileFlags {
                    cxxflags: vec!["-O2".into()],
                    ldflags: vec!["-s".into()],
                    ..Default::default()
                }),
            },
        )]);
        let selected = selected_profile("release", &definitions);

        let resolved = resolve_build_flags(
            &package,
            Some(&selected),
            &definitions,
            &ConditionContext::platform_only(&host_for("linux")),
            false,
        );
        assert_eq!(resolved.defines, vec!["SAFE_NAMED_OVERLAY"]);
        assert_eq!(resolved.cxxflags, vec!["-O2"]);
        assert_eq!(resolved.ldflags, vec!["-s"]);
    }

    #[test]
    fn feature_conditional_layer_gated_by_enabled_features() {
        // `[target.'cfg(feature = "single-threaded")'.profile]
        // defines = ["SQLITE_THREADSAFE=0"]` applies iff the feature
        // is enabled - the sqlite threadsafe-toggle wiring.
        let mut p = ProfileSettings::default();
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::Feature("single-threaded".into()),
            profile: None,
            flags: ProfileFlags {
                defines: vec!["SQLITE_THREADSAFE=0".into()],
                ..Default::default()
            },
        });
        let enabled: BTreeSet<String> = BTreeSet::from(["single-threaded".to_owned()]);
        let on = resolve_without_profile(
            &p,
            &ConditionContext::with_features(&host_for("linux"), &enabled),
            true,
        );
        assert_eq!(on.defines, vec!["SQLITE_THREADSAFE=0".to_owned()]);
        let off = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            true,
        );
        assert!(
            off.defines.is_empty(),
            "feature-off must not apply the layer: {:?}",
            off.defines
        );
    }

    #[test]
    fn compiler_conditional_layer_gated_by_detected_identity() {
        let mut p = ProfileSettings::default();
        p.conditional.push(ConditionalProfileFlags {
            condition: Condition::parse_inner(r#"all(cxx = "clang", cxx_version = ">=18")"#)
                .unwrap(),
            profile: None,
            flags: ProfileFlags {
                cxxflags: vec!["-stdlib=libc++".into()],
                ..Default::default()
            },
        });
        let host = host_for("linux");
        let clang18 = CompilerIdentity {
            kind: CompilerKind::Clang,
            version: CompilerVersion::parse("18.1.3"),
            target: None,
            raw_version_line: "clang version 18.1.3".into(),
        };
        let gcc13 = CompilerIdentity {
            kind: CompilerKind::Gcc,
            version: CompilerVersion::parse("13.3.0"),
            target: None,
            raw_version_line: "g++ 13.3.0".into(),
        };

        let matching = ConditionContext::platform_only(&host).with_compilers(None, Some(&clang18));
        let on = resolve_without_profile(&p, &matching, true);
        assert_eq!(on.cxxflags, vec!["-stdlib=libc++".to_owned()]);

        let other = ConditionContext::platform_only(&host).with_compilers(None, Some(&gcc13));
        let off = resolve_without_profile(&p, &other, true);
        assert!(off.cxxflags.is_empty());

        // No detection at all (fail-soft commands): the layer stays off.
        let undetected = ConditionContext::platform_only(&host);
        assert!(
            resolve_without_profile(&p, &undetected, true)
                .cxxflags
                .is_empty()
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
            profile: None,
            flags: ProfileFlags {
                link_libs: vec!["dl".into(), "m".into()],
                ..Default::default()
            },
        });
        let mut host = host_for("linux");
        host.family = "unix".into();
        let r = resolve_without_profile(&p, &ConditionContext::platform_only(&host), true);
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
        let r = resolve_without_profile(
            &p,
            &ConditionContext::platform_only(&host_for("linux")),
            false,
        );
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

    #[test]
    fn resolved_flags_with_only_system_include_dirs_are_not_empty() {
        // System include dirs (e.g. a pkg-config contribution) must
        // count as a non-empty flag set, or consumers keyed on
        // `is_empty` would drop them from metadata views.
        let flags = ResolvedProfileFlags {
            system_include_dirs: vec![Utf8PathBuf::from("/opt/dep/include")],
            ..Default::default()
        };
        assert!(!flags.is_empty());
    }

    #[test]
    fn resolved_flags_json_includes_system_include_dirs() {
        let flags = ResolvedProfileFlags {
            system_include_dirs: vec![Utf8PathBuf::from("/opt/dep/include")],
            ..Default::default()
        };
        assert_eq!(
            flags.as_json()["system_include_dirs"],
            serde_json::json!(["/opt/dep/include"]),
        );
    }
}
