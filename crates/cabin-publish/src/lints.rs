//! Standard-compatibility publish-time lints
//! (`docs/design/standard-compatibility/publish-lints.md`).
//!
//! Every lint is a **pure function** over its inputs - the resolved
//! manifest being published (spec D6 attributes) and, for PL3, the
//! already-published versions read from the registry index.  No I/O,
//! no registry access: the caller loads the inputs and feeds them in,
//! so a future hosted registry can run the identical checks
//! server-side.  Spec identifiers (D1-D14, ...) refer to
//! `docs/design/standard-compatibility/spec.md`.
//!
//! Three lints:
//! - **PL1** (error): a target's declared implementation standard
//!   falls outside its own interface range - below the declared
//!   minimum or above the declared maximum.  Duplicates the
//!   load-time `cabin::language::interface_standard_contradiction` by
//!   design - defense in depth at the publish boundary - and also
//!   covers the header-only direct pair the load-time check never sees.
//! - **PL2** (warning): a header-only target leaves an implemented
//!   language's interface requirement to inference (spec D9 row 3).
//! - **PL3** (warning): a patch release narrows a declared
//!   requirement's accepted set (spec D12) versus the immediately
//!   previous version - a raised minimum, an added or lowered
//!   maximum, a sideways shift, or a flip to `none`; the check is
//!   semantic, not an order comparison, because `⊑` is only partial.

use cabin_core::standard_compatibility::{DependencyKind, dependency_attributes};
use cabin_core::{
    InterfaceRequirement, Package, Requirement, StandardsMetadata, resolve_language_standards,
};

/// Whether a finding rejects the publish (PL1) or only warns (PL2, PL3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintSeverity {
    /// Rejects the publish before any registry write.
    Error,
    /// Printed and let the publish proceed.
    Warning,
}

/// One publish-lint finding: its severity, the lint that produced it
/// (`"PL1"` / `"PL2"` / `"PL3"`), and a rendered, user-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    pub severity: LintSeverity,
    pub code: &'static str,
    pub message: String,
}

impl LintFinding {
    fn error(code: &'static str, message: String) -> Self {
        Self {
            severity: LintSeverity::Error,
            code,
            message,
        }
    }

    fn warning(code: &'static str, message: String) -> Self {
        Self {
            severity: LintSeverity::Warning,
            code,
            message,
        }
    }

    /// Whether this finding rejects the publish.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == LintSeverity::Error
    }
}

/// Split evaluated findings into the warning messages to surface and,
/// when any lint rejects the publish, the rejecting findings.  The
/// caller turns the `Err` payload into a publish error (failing before
/// any registry write) and prints the `Ok` warnings.
///
/// # Errors
/// Returns the rejecting findings (PL1) when at least one is present.
pub fn split(findings: Vec<LintFinding>) -> Result<Vec<String>, Vec<LintFinding>> {
    let (errors, warnings): (Vec<_>, Vec<_>) =
        findings.into_iter().partition(LintFinding::is_error);
    if errors.is_empty() {
        Ok(warnings
            .into_iter()
            .map(|finding| finding.message)
            .collect())
    } else {
        Err(errors)
    }
}

/// Per-language rendering labels: the index/display key (`c` / `c++`)
/// and the manifest field names the message points the author at.
struct Language {
    key: &'static str,
    interface_field: &'static str,
    impl_field: &'static str,
}

const C: Language = Language {
    key: "c",
    interface_field: "interface-c-standard",
    impl_field: "c-standard",
};

const CXX: Language = Language {
    key: "c++",
    interface_field: "interface-cxx-standard",
    impl_field: "cxx-standard",
};

/// PL1 and PL2: pure over the resolved manifest being published.
///
/// Walks every library-like target (spec's library / header-only
/// kinds - the only ones that constrain consumers) in deterministic
/// order (by target name, then `c` before `c++`) and applies the two
/// manifest-only lints to each `(target, language)` cell via the
/// shared spec-D6 attribute mapping, so the values linted are exactly
/// what the published `standards` table would carry.
#[must_use]
pub fn manifest_findings(package: &Package) -> Vec<LintFinding> {
    let resolved = resolve_language_standards(&package.language);
    let mut targets: Vec<_> = package
        .targets
        .iter()
        .filter(|target| target.kind.is_library_like())
        .collect();
    targets.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

    let mut findings = Vec::new();
    for target in targets {
        let attributes = dependency_attributes(target, &resolved, &package.language);
        let name = target.name.as_str();
        cell_findings(
            name,
            &C,
            attributes.kind,
            attributes.decl_c,
            attributes.impl_c,
            &mut findings,
        );
        cell_findings(
            name,
            &CXX,
            attributes.kind,
            attributes.decl_cxx,
            attributes.impl_cxx,
            &mut findings,
        );
    }
    findings
}

/// PL1 + PL2 for one `(target, language)` cell.
fn cell_findings<S: Copy + Ord + std::fmt::Display>(
    target: &str,
    language: &Language,
    kind: DependencyKind,
    declaration: Option<InterfaceRequirement<S>>,
    implementation: Option<S>,
    findings: &mut Vec<LintFinding>,
) {
    // PL1 (error): the implementation standard falls outside the
    // explicitly declared interface range - the target's own
    // translation units (or, header-only, its own headers) could not
    // include the very headers the row advertises.  `"none"`
    // (forbidden) and absence are outside PL1; only a declared range
    // is checked.
    if let (Some(InterfaceRequirement::Requirement(requirement)), Some(implementation)) =
        (declaration, implementation)
    {
        if requirement.min() > implementation {
            findings.push(LintFinding::error(
                "PL1",
                format!(
                    "target `{target}`: `{field}` min `{min}` is newer than its {key} implementation standard `{implementation}`; a published interface minimum must not exceed the standard the target compiles with - lower `{field}` or raise `{impl_field}`",
                    field = language.interface_field,
                    min = requirement.min(),
                    key = language.key,
                    impl_field = language.impl_field,
                ),
            ));
        }
        if let Some(max) = requirement.max()
            && max < implementation
        {
            findings.push(LintFinding::error(
                "PL1",
                format!(
                    "target `{target}`: `{field}` max `{max}` is older than its {key} implementation standard `{implementation}`; the target's own translation units already exceed the published interface maximum - raise the max or lower `{impl_field}`",
                    field = language.interface_field,
                    key = language.key,
                    impl_field = language.impl_field,
                ),
            ));
        }
    }

    // PL2 (warning): a header-only target that implements the language
    // but declares no interface for it - the published requirement is
    // inferred from the implementation standard (spec D9 row 3), which
    // is only an upper bound on what the headers actually need.
    if kind == DependencyKind::HeaderOnly
        && declaration.is_none()
        && let Some(implementation) = implementation
    {
        findings.push(LintFinding::warning(
            "PL2",
            format!(
                "target `{target}`: header-only target declares no `{field}`, so its {key} interface requirement is inferred as `{implementation}` from the implementation standard - declare `{field}` to publish the audited minimum instead of an upper bound",
                field = language.interface_field,
                key = language.key,
            ),
        ));
    }
}

/// PL3: pure over the new declared table and the already-published
/// versions read from the index.
///
/// Warns when this is a patch release whose declared requirement for
/// some `(target, language)` present in both versions accepts fewer
/// consumer standards than the baseline's - its satisfaction set
/// (spec D12) drops a level the baseline accepted: a raised minimum,
/// an added or lowered maximum, a sideways shift, or a flip to
/// `"none"` (forbidden).  The check is semantic rather than a `⊑`
/// comparison because `⊑` is only partial (spec L1): incomparable
/// shifts still lose consumers.  A target present only in the new
/// version is an addition, not a narrowing.
///
/// The baseline is the greatest already-published, non-pre-release
/// version strictly below `new_version` that shares its `major.minor`
/// (the release this one patches, per `docs/registry-design.md`).  When
/// none exists (an `x.y.0`, the first publish of a line, or a
/// pre-release new version), this is not a patch release and PL3 does
/// not fire.  This is the precise reading of "the immediately previous
/// version" for out-of-order publishes.
///
/// This declared-cell comparison cannot see effective-requirement
/// raises caused solely by a public dependency's version-requirement
/// change; see the "Limitation" section of the design doc.
#[must_use]
pub fn patch_release_findings(
    new_version: &semver::Version,
    new_table: &StandardsMetadata,
    published: &[(semver::Version, StandardsMetadata)],
) -> Vec<LintFinding> {
    let Some(baseline) = patch_baseline(new_version, published) else {
        return Vec::new();
    };

    // PL3 compares only targets recorded in both stored tables.  A
    // target absent from the baseline's table is an addition, not a
    // raise - `from_package` records a row for every library-like
    // target, so a target genuinely present in the baseline is never
    // missing.  A baseline with no stored table at all (a version
    // published before the field existed, or one with no library-like
    // targets) therefore yields no comparison: reading it as
    // unconstrained-everywhere would flag a package's first library
    // target added in a patch as a false raise.  See the "baselines
    // with no recorded table" limitation in publish-lints.md.
    let mut findings = Vec::new();
    for (target, new_row) in &new_table.targets {
        let Some(old_row) = baseline.targets.get(target) else {
            continue;
        };
        raise_finding(
            target,
            &C,
            old_row.interface_c,
            new_row.interface_c,
            &mut findings,
        );
        raise_finding(
            target,
            &CXX,
            old_row.interface_cxx,
            new_row.interface_cxx,
            &mut findings,
        );
    }
    findings
}

/// The declared table of the version `new_version` patches, or `None`
/// when `new_version` is not a patch release with a comparable
/// baseline (see [`patch_release_findings`]).
fn patch_baseline<'a>(
    new_version: &semver::Version,
    published: &'a [(semver::Version, StandardsMetadata)],
) -> Option<&'a StandardsMetadata> {
    // A pre-release neither triggers PL3 nor serves as a baseline: its
    // contract is explicitly unstable.
    if !new_version.pre.is_empty() {
        return None;
    }
    published
        .iter()
        .filter(|(version, _)| {
            version.pre.is_empty()
                && version.major == new_version.major
                && version.minor == new_version.minor
                && version < new_version
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, table)| table)
}

/// PL3 for one `(target, language)` cell shared by both versions.
fn raise_finding<S: cabin_core::StandardLevel + std::fmt::Display>(
    target: &str,
    language: &Language,
    old: Requirement<S>,
    new: Requirement<S>,
    findings: &mut Vec<LintFinding>,
) {
    // The strictness order is partial (ranges can shift sideways),
    // so the lint checks the semantics directly: the new
    // requirement narrows the accepted set when some consumer level
    // the baseline accepted is no longer accepted - a raised min, a
    // lowered max, a shifted range, or a flip to forbidden.  A pure
    // widening is never linted.
    let old_sat = old.sat(S::levels());
    let new_sat = new.sat(S::levels());
    if old_sat.iter().any(|level| !new_sat.contains(level)) {
        findings.push(LintFinding::warning(
            "PL3",
            format!(
                "target `{target}`: {key} interface requirement narrowed from {old} to {new} in a patch release; the new requirement excludes consumer standards the baseline accepted - treated as a minor incompatibility, allowed in minor releases, discouraged in patches",
                key = language.key,
                old = describe(old),
                new = describe(new),
            ),
        ));
    }
}

/// Render a requirement for a PL3 message.
fn describe<S: cabin_core::StandardLevel>(requirement: Requirement<S>) -> String {
    match requirement {
        Requirement::Unconstrained => "unconstrained".to_owned(),
        Requirement::Forbidden => "forbidden (`none`)".to_owned(),
        Requirement::Min(_) | Requirement::Bounded(_) => format!("`{requirement}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::index_standards::TargetStandards;
    use cabin_core::model::{Target, TargetKind, TargetName};
    use cabin_core::{
        CStandard, CxxStandard, LanguageStandardSettings, StandardDeclaration, StandardRequirement,
    };
    use camino::Utf8PathBuf;

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement::at_least(min))
    }

    fn target(
        name: &str,
        kind: TargetKind,
        sources: &[&str],
        language: LanguageStandardSettings,
    ) -> Target {
        Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: sources.iter().map(Utf8PathBuf::from).collect(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language,
        }
    }

    fn package(targets: Vec<Target>) -> Package {
        Package::new(
            cabin_core::PackageName::new("demo").unwrap(),
            semver::Version::parse("1.0.0").unwrap(),
            targets,
            Vec::new(),
        )
        .unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn row(
        interface_c: Requirement<CStandard>,
        interface_cxx: Requirement<CxxStandard>,
    ) -> TargetStandards {
        TargetStandards {
            header_only: false,
            gnu_extensions: false,
            interface_c,
            interface_cxx,
        }
    }

    fn table(rows: &[(&str, TargetStandards)]) -> StandardsMetadata {
        StandardsMetadata {
            targets: rows
                .iter()
                .map(|(name, row)| ((*name).to_owned(), *row))
                .collect(),
        }
    }

    // --- PL1 ---------------------------------------------------------

    /// PL1 fires on a compiled C++ target whose declared interface
    /// minimum (`c++20`) is newer than what its sources compile as
    /// (`c++17`).  This is the same predicate as the load-time lint.
    #[test]
    fn pl1_fires_on_compiled_interface_newer_than_impl() {
        let lib = target(
            "cxxlib",
            TargetKind::Library,
            &["src/cxxlib.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(interface_min(
                    CxxStandard::Cxx20,
                ))),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![lib]));
        let errors: Vec<_> = findings.iter().filter(|f| f.is_error()).collect();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert_eq!(errors[0].code, "PL1");
        assert!(errors[0].message.contains("interface-cxx-standard"));
        assert!(errors[0].message.contains("c++20"));
        assert!(errors[0].message.contains("c++17"));
    }

    /// PL1 also fires on the header-only direct pair - a case the
    /// load-time check (which only inspects compiled sources) never
    /// sees: header-only `cxx-standard = c++17`, interface `c++20`.
    #[test]
    fn pl1_fires_on_header_only_direct_pair() {
        let hdr = target(
            "hdr",
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(interface_min(
                    CxxStandard::Cxx20,
                ))),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![hdr]));
        assert!(
            findings.iter().any(|f| f.code == "PL1" && f.is_error()),
            "{findings:?}"
        );
    }

    /// PL1 fires for C too: `interface-c-standard = c17` over a `c11`
    /// implementation.
    #[test]
    fn pl1_fires_for_c() {
        let lib = target(
            "clib",
            TargetKind::Library,
            &["src/clib.c"],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                interface_c_standard: Some(StandardDeclaration::Declared(interface_min(
                    CStandard::C17,
                ))),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![lib]));
        let error = findings.iter().find(|f| f.code == "PL1").expect("PL1");
        assert!(error.message.contains("interface-c-standard"));
        assert!(error.message.contains("c17"));
        assert!(error.message.contains("c11"));
    }

    /// PL1 does not fire when the interface minimum equals (or is
    /// older than) the implementation standard, nor for `"none"`.
    #[test]
    fn pl1_quiet_when_interface_not_newer() {
        let equal = target(
            "eq",
            TargetKind::Library,
            &["src/eq.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(interface_min(
                    CxxStandard::Cxx17,
                ))),
                ..Default::default()
            },
        );
        assert!(
            manifest_findings(&package(vec![equal]))
                .iter()
                .all(|f| f.code != "PL1")
        );
        let none = target(
            "opaque",
            TargetKind::Library,
            &["src/opaque.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_c_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::None,
                )),
                ..Default::default()
            },
        );
        assert!(
            manifest_findings(&package(vec![none]))
                .iter()
                .all(|f| f.code != "PL1")
        );
    }

    // --- PL2 ---------------------------------------------------------

    /// PL2 warns on the residual second-language case: a header-only
    /// target declaring an interface for C but leaving its C++
    /// implementation's interface to inference.
    #[test]
    fn pl2_warns_on_inferred_second_language() {
        let hdr = target(
            "hdr",
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                interface_c_standard: Some(StandardDeclaration::Declared(interface_min(
                    CStandard::C11,
                ))),
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![hdr]));
        let warnings: Vec<_> = findings.iter().filter(|f| f.code == "PL2").collect();
        assert_eq!(warnings.len(), 1, "{findings:?}");
        assert_eq!(warnings[0].severity, LintSeverity::Warning);
        assert!(warnings[0].message.contains("interface-cxx-standard"));
        assert!(warnings[0].message.contains("c++20"));
    }

    /// PL2 stays quiet when the header-only target declares the
    /// interface explicitly (no inference), and never fires for a
    /// compiled target (row 4 imposes nothing).
    #[test]
    fn pl2_quiet_when_declared_or_compiled() {
        let declared = target(
            "hdr",
            TargetKind::HeaderOnly,
            &[],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(interface_min(
                    CxxStandard::Cxx17,
                ))),
                ..Default::default()
            },
        );
        assert!(
            manifest_findings(&package(vec![declared]))
                .iter()
                .all(|f| f.code != "PL2")
        );
        let compiled = target(
            "lib",
            TargetKind::Library,
            &["src/lib.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        );
        assert!(
            manifest_findings(&package(vec![compiled]))
                .iter()
                .all(|f| f.code != "PL2")
        );
    }

    // --- PL3 ---------------------------------------------------------

    /// PL3 warns when a patch release raises a declared minimum for a
    /// target present in both versions.
    #[test]
    fn pl3_warns_on_patch_raise() {
        let previous = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx17)),
        )]);
        let new = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx20)),
        )]);
        let findings = patch_release_findings(&ver("1.2.4"), &new, &[(ver("1.2.3"), previous)]);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, "PL3");
        assert!(findings[0].message.contains("c++17"));
        assert!(findings[0].message.contains("c++20"));
        assert!(findings[0].message.contains("discouraged in patches"));
    }

    /// A newly declared requirement on a previously unconstrained cell
    /// is a strict raise, and so is a flip to `"none"` (forbidden).
    #[test]
    fn pl3_catches_unconstrained_to_min_and_to_forbidden() {
        let previous = table(&[(
            "lib",
            row(Requirement::Unconstrained, Requirement::Unconstrained),
        )]);
        let new = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx17)),
        )]);
        let findings = patch_release_findings(&ver("2.0.1"), &new, &[(ver("2.0.0"), previous)]);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.code == "PL3"));
    }

    /// PL3 does not fire on a lowering (relaxation), on a target only
    /// present in the new version, nor on a minor/major release.
    #[test]
    fn pl3_quiet_on_relaxation_addition_and_minor() {
        let previous = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx20)),
        )]);
        // Lowering c++20 -> c++17 and dropping C's forbidden.
        let relaxed = table(&[(
            "lib",
            row(
                Requirement::Unconstrained,
                Requirement::Min(CxxStandard::Cxx17),
            ),
        )]);
        assert!(
            patch_release_findings(&ver("1.0.1"), &relaxed, &[(ver("1.0.0"), previous.clone())])
                .is_empty()
        );
        // A brand-new target is an addition, not a raise.
        let added = table(&[
            (
                "lib",
                row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx20)),
            ),
            (
                "extra",
                row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx23)),
            ),
        ]);
        assert!(
            patch_release_findings(&ver("1.0.1"), &added, &[(ver("1.0.0"), previous.clone())])
                .is_empty()
        );
        // A minor release raising the requirement is allowed.
        let raised = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx23)),
        )]);
        assert!(
            patch_release_findings(&ver("1.1.0"), &raised, &[(ver("1.0.0"), previous)]).is_empty()
        );
    }

    /// First publish of a line has no baseline, so PL3 never fires -
    /// even for an `x.y.0` sitting above an older minor.
    #[test]
    fn pl3_quiet_on_first_publish() {
        let new = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx23)),
        )]);
        // No published versions at all.
        assert!(patch_release_findings(&ver("1.0.0"), &new, &[]).is_empty());
        // A `1.1.0` with only `1.0.x` below shares no major.minor
        // baseline, so it is a minor release, not a patch.
        let older = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx17)),
        )]);
        assert!(patch_release_findings(&ver("1.1.0"), &new, &[(ver("1.0.9"), older)]).is_empty());
    }

    /// A baseline whose index entry stores no `standards` table (a
    /// version published before the field existed, or one with no
    /// library-like targets) offers no rows to compare, so PL3 makes no
    /// comparison against it - deliberately, so a package's first
    /// library target added in a patch is not flagged as a false raise.
    #[test]
    fn pl3_does_not_compare_against_an_unrecorded_baseline() {
        // An empty baseline table: no `standards` field, which
        // `read_published_standards` maps to an empty table.
        let unrecorded = StandardsMetadata::default();
        let new = table(&[(
            "lib",
            row(
                Requirement::Unconstrained,
                Requirement::Min(CxxStandard::Cxx20),
            ),
        )]);
        assert!(
            patch_release_findings(&ver("1.0.1"), &new, &[(ver("1.0.0"), unrecorded)]).is_empty()
        );
    }

    /// The baseline is the greatest published patch below the new one,
    /// so an out-of-order publish compares against the right version.
    #[test]
    fn pl3_baseline_is_greatest_prior_patch() {
        let v1 = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx17)),
        )]);
        let v3 = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx23)),
        )]);
        let new = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx20)),
        )]);
        // Publishing 1.0.2 with 1.0.1 (c++17) and 1.0.3 (c++23)
        // present: baseline is 1.0.1, and c++17 -> c++20 is a raise.
        let published = vec![(ver("1.0.1"), v1), (ver("1.0.3"), v3)];
        let findings = patch_release_findings(&ver("1.0.2"), &new, &published);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("c++17"));
    }

    /// A pre-release new version neither triggers PL3 nor is compared,
    /// and pre-release published versions are not baselines.
    #[test]
    fn pl3_ignores_pre_releases() {
        let previous = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx17)),
        )]);
        let new = table(&[(
            "lib",
            row(Requirement::Forbidden, Requirement::Min(CxxStandard::Cxx20)),
        )]);
        // New version is a pre-release: no lint.
        assert!(
            patch_release_findings(
                &ver("1.0.1-rc.1"),
                &new,
                &[(ver("1.0.0"), previous.clone())]
            )
            .is_empty()
        );
        // Only a pre-release sits below: not a baseline.
        assert!(
            patch_release_findings(&ver("1.0.1"), &new, &[(ver("1.0.1-rc.1"), previous)])
                .is_empty()
        );
    }

    /// PL1 also fires on the max side: an implementation standard
    /// above the declared interface maximum is an error, for both
    /// languages.
    #[test]
    fn pl1_errors_when_implementation_exceeds_declared_max() {
        let lib = target(
            "lib",
            TargetKind::Library,
            &["src/lib.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx23)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::Requirement(
                        StandardRequirement::bounded(CxxStandard::Cxx14, Some(CxxStandard::Cxx20))
                            .unwrap(),
                    ),
                )),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![lib]));
        let error = findings.iter().find(|f| f.code == "PL1").expect("PL1");
        assert!(error.is_error());
        assert!(
            error.message.contains("max `c++20` is older than"),
            "unexpected message: {}",
            error.message
        );

        let clib = target(
            "clib",
            TargetKind::Library,
            &["src/lib.c"],
            LanguageStandardSettings {
                c_standard: Some(StandardDeclaration::Declared(CStandard::C23)),
                interface_c_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::Requirement(
                        StandardRequirement::bounded(CStandard::C99, Some(CStandard::C17)).unwrap(),
                    ),
                )),
                ..Default::default()
            },
        );
        let findings = manifest_findings(&package(vec![clib]));
        assert!(
            findings
                .iter()
                .any(|f| f.code == "PL1" && f.message.contains("max `c17`")),
            "expected the C max PL1: {findings:?}"
        );

        // An implementation inside the declared range is clean.
        let ok = target(
            "ok",
            TargetKind::Library,
            &["src/ok.cc"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                interface_cxx_standard: Some(StandardDeclaration::Declared(
                    InterfaceRequirement::Requirement(
                        StandardRequirement::bounded(CxxStandard::Cxx14, Some(CxxStandard::Cxx20))
                            .unwrap(),
                    ),
                )),
                ..Default::default()
            },
        );
        assert!(
            manifest_findings(&package(vec![ok]))
                .iter()
                .all(|f| f.code != "PL1")
        );
    }

    /// PL3 checks the accepted set, not a total order: adding a cap,
    /// shifting a range sideways, and flipping to `"none"` all
    /// narrow it; a pure widening does not lint, and neither does
    /// the sat-equal unconstrained -> lowest-minimum change.
    #[test]
    fn pl3_narrowing_is_semantic_over_the_accepted_set() {
        fn table(requirement: Requirement<CxxStandard>) -> StandardsMetadata {
            let mut table = StandardsMetadata::default();
            table.targets.insert(
                "lib".to_owned(),
                TargetStandards {
                    interface_cxx: requirement,
                    ..Default::default()
                },
            );
            table
        }
        let version = semver::Version::parse("1.2.1").unwrap();
        let published =
            |requirement| vec![(semver::Version::parse("1.2.0").unwrap(), table(requirement))];
        let narrowed = |old, new| {
            patch_release_findings(&version, &table(new), &published(old))
                .iter()
                .any(|f| f.code == "PL3")
        };

        // Adding a cap narrows.
        assert!(narrowed(
            Requirement::Min(CxxStandard::Cxx14),
            Requirement::bounded(CxxStandard::Cxx14, CxxStandard::Cxx20).unwrap(),
        ));
        // A sideways shift narrows (it drops c++11 consumers) even
        // though the two ranges are order-incomparable.
        assert!(narrowed(
            Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx17).unwrap(),
            Requirement::bounded(CxxStandard::Cxx14, CxxStandard::Cxx20).unwrap(),
        ));
        // Flipping to forbidden narrows.
        assert!(narrowed(
            Requirement::Min(CxxStandard::Cxx14),
            Requirement::Forbidden,
        ));
        // Pure widening does not lint.
        assert!(!narrowed(
            Requirement::bounded(CxxStandard::Cxx14, CxxStandard::Cxx17).unwrap(),
            Requirement::Min(CxxStandard::Cxx11),
        ));
        // Sat-equal reshaping does not lint: unconstrained and the
        // lowest minimum accept the same set.
        assert!(!narrowed(
            Requirement::Unconstrained,
            Requirement::Min(CxxStandard::Cxx98),
        ));
    }
}
