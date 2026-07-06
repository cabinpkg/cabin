//! Rendering and gating for the standard-compatibility check.
//!
//! `cabin-build`'s post-resolution pass hands the CLI typed
//! [`StandardCompatViolation`] records (see
//! `cabin_build::standard_compat`); this module composes the
//! user-facing wording - the provenance chain with manifest
//! `path:line` references - re-locates the consumer's standard
//! declaration for a labeled snippet
//! ([`cabin_manifest::standard_field_span`]), and renders each
//! record through `cabin-diagnostics`.  Violations are
//! error-severity and fail the command; a violation whose edge
//! carries the per-edge `ignore-interface-standard = true`
//! override renders as an unchecked-edge note instead and never
//! gates.

use std::collections::BTreeSet;
use std::fmt;

use anyhow::Result;
use cabin_build::{
    DeclScope, DeclSite, EdgeRequirement, RequirementOrigin, StandardCompatViolation,
};
use cabin_diagnostics::miette;

/// Render every violation and gate the command.  Violations arrive
/// pre-sorted from the planner; rendering keeps that order.
/// `lockfile_pinned` holds the (name, version) selections that came
/// straight out of a pre-existing `cabin.lock`; a violation whose
/// dependency is among them also explains the likely staleness
/// cause and the re-resolve remedy.
///
/// Non-ignored violations render as errors and produce an `Err`
/// that fails the command.  Ignored violations (per-edge
/// `ignore-interface-standard = true`) render as one
/// unchecked-edge note per edge and never gate.
///
/// # Errors
/// Returns an error when at least one non-ignored violation was
/// rendered.
pub(crate) fn report(
    violations: &[StandardCompatViolation],
    color: cabin_core::ColorChoice,
    lockfile_pinned: &BTreeSet<(String, String)>,
) -> Result<()> {
    if violations.is_empty() {
        return Ok(());
    }
    let mut stderr = termcolor::StandardStream::stderr(cabin_diagnostics::termcolor_choice(color));
    let mut unchecked_edges: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut gating = 0usize;
    for violation in violations {
        if violation.ignored {
            // One note per edge: a mixed-language consumer failing
            // both languages on one overridden edge yields two
            // records but the edge goes unchecked exactly once.
            if unchecked_edges.insert((&violation.consumer, &violation.dependency)) {
                cabin_diagnostics::render(&unchecked_note(violation), &mut stderr, color)?;
            }
            continue;
        }
        gating += 1;
        let from_lockfile = lockfile_pinned.contains(&(
            violation.dependency_package.clone(),
            violation.dependency_version.clone(),
        ));
        let diagnostic = violation_diagnostic(violation, from_lockfile);
        cabin_diagnostics::render(&diagnostic, &mut stderr, color)?;
    }
    if gating > 0 {
        anyhow::bail!(
            "{gating} standard compatibility violation{s}",
            s = crate::plural(gating),
        );
    }
    Ok(())
}

/// One rendered diagnostic.  Implements [`miette::Diagnostic`] by
/// hand (matching `cabin_diagnostics::CodedMessage`) so the CLI
/// does not grow a direct dependency on the derive machinery.
#[derive(Debug)]
struct StandardCompatDiagnostic {
    message: String,
    help: String,
    severity: miette::Severity,
    code: &'static str,
    source: Option<miette::NamedSource<String>>,
    label: String,
    span: Option<miette::SourceSpan>,
}

impl fmt::Display for StandardCompatDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StandardCompatDiagnostic {}

impl miette::Diagnostic for StandardCompatDiagnostic {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(self.code))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(self.severity)
    }

    fn help(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(&self.help))
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.span?;
        self.source
            .as_ref()
            .map(|source| source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        let span = self.span?;
        Some(Box::new(std::iter::once(
            miette::LabeledSpan::new_with_span(Some(self.label.clone()), span),
        )))
    }
}

fn violation_diagnostic(
    violation: &StandardCompatViolation,
    from_lockfile: bool,
) -> StandardCompatDiagnostic {
    let lang = violation.language.human_label();
    let consumer = format!(
        "`{}` ({}, {})",
        violation.consumer,
        violation.consumer_standard,
        site_ref(&violation.consumer_site),
    );

    // "via public dependency `origin`" whenever the requirement was
    // not declared by the direct dependency itself; longer
    // provenance chains name every hop.
    let via = match violation.chain.len() {
        0 | 1 => String::new(),
        2 => format!(" via public dependency `{}`", violation.origin_target),
        _ => format!(
            " via public dependency chain `{}`",
            violation.chain.join("` -> `"),
        ),
    };

    let message = match (violation.requirement, &violation.origin) {
        (EdgeRequirement::Min(min), RequirementOrigin::Declared { site }) => format!(
            "{consumer} -> `{}` requires {lang} consumers at `{min}` or newer{via} (`{}`, {})",
            violation.dependency,
            site.field,
            site_ref(site),
        ),
        (EdgeRequirement::Min(min), RequirementOrigin::HeaderOnlyInference { site }) => format!(
            "{consumer} -> `{}` requires {lang} consumers at `{min}` or newer{via} (inferred \
             from implementation standard `{}`, {})",
            violation.dependency,
            site.field,
            site_ref(site),
        ),
        (EdgeRequirement::Forbidden, RequirementOrigin::DeclaredNone { site }) => format!(
            "{consumer} -> `{}` cannot be consumed from {lang}: {lang} consumption was \
             disabled by `{} = \"none\"`{via} ({})",
            violation.dependency,
            site.field,
            site_ref(site),
        ),
        (EdgeRequirement::Forbidden, RequirementOrigin::CrossLanguageDefault) => format!(
            "{consumer} -> `{}` implements no {lang} and declares no `{}`, so it cannot be \
             consumed from {lang}{via}",
            violation.dependency,
            interface_field(violation.language),
        ),
        // The pass only ever pairs `Min` with a declaration or
        // inference origin and `Forbidden` with `none` or the
        // cross-language default (spec D9 rows 1-3, 6).
        (EdgeRequirement::Min(_) | EdgeRequirement::Forbidden, _) => {
            unreachable!("requirement/origin combination violates spec D9: {violation:?}")
        }
    };

    let pin = if violation.dependency_is_registry {
        format!(
            ", or pin `{}` to an older version (currently {})",
            violation.dependency_package, violation.dependency_version,
        )
    } else {
        String::new()
    };
    // `cabin.lock` records version pins only - never standards - so
    // a violation whose dependency version came straight out of the
    // lockfile usually means a manifest's standard declaration
    // changed after the lockfile was generated.  Path dependencies
    // are never locked, so the note would only mislead there.
    let lockfile_note = if from_lockfile && violation.dependency_is_registry {
        "; this dependency's resolved version was loaded from cabin.lock, which records \
         version pins only - if a standard declaration changed in a manifest after the \
         lockfile was generated, run `cabin update` to re-resolve"
    } else {
        ""
    };
    // The per-edge override lives on the dependency entry that
    // resolved this edge, so the remedy names that entry's table
    // and is withheld for intra-package edges, which have no
    // entry to carry it.  It is also withheld for minimum
    // violations: those the always-on build-time interface
    // enforcement (`validate_planned_standards`) independently
    // rejects, so the override would silence this error only to
    // fail the command anyway.  The forbidden classes (an
    // interface `"none"`, the strict cross-language default) are
    // exactly the ones that layer deliberately accepts - there
    // the override genuinely unblocks the command.
    let last_resort = match (violation.requirement, &violation.override_section) {
        (EdgeRequirement::Forbidden, Some(section)) => format!(
            "; as a last resort, `{} = {{ ..., ignore-interface-standard = true }}` in the \
             `{section}` table of {} leaves exactly this edge unchecked",
            violation.dependency_package,
            violation.consumer_manifest_path.display(),
        ),
        _ => String::new(),
    };
    let help = match violation.requirement {
        EdgeRequirement::Min(min) => format!(
            "raise `{}`'s {lang} standard to at least `{min}`{pin}{lockfile_note}{last_resort}",
            violation.consumer,
        ),
        EdgeRequirement::Forbidden => format!(
            "`{}` cannot be consumed from {lang} at any standard level{pin}{lockfile_note}\
             {last_resort}",
            violation.origin_target,
        ),
    };

    let (source, span) = consumer_snippet(&violation.consumer_site);
    StandardCompatDiagnostic {
        message,
        help,
        severity: miette::Severity::Error,
        code: cabin_diagnostics::code::LANGUAGE_STANDARD_COMPAT_VIOLATION,
        source: Some(source),
        label: format!(
            "`{}` compiles {lang} as `{}`",
            violation.consumer, violation.consumer_standard,
        ),
        span,
    }
}

/// The downgraded note for an edge the consuming package opted out
/// of the check: the violation is suppressed, but the edge is
/// called out as unchecked so the override cannot silently rot.
fn unchecked_note(violation: &StandardCompatViolation) -> StandardCompatDiagnostic {
    // Only override-suppressed violations reach this note, and a
    // suppressing entry always has a section.
    let section = violation
        .override_section
        .as_deref()
        .unwrap_or("[dependencies]");
    StandardCompatDiagnostic {
        message: format!(
            "dependency edge `{}` -> `{}` is unchecked: `ignore-interface-standard = true` \
             is set for `{}` in the `{section}` table of {}",
            violation.consumer,
            violation.dependency,
            violation.dependency_package,
            violation.consumer_manifest_path.display(),
        ),
        help: format!(
            "the edge violates `{}`'s standard compatibility requirements; remove \
             `ignore-interface-standard` from the `{section}` entry to re-enable the check",
            violation.origin_target,
        ),
        severity: miette::Severity::Advice,
        code: cabin_diagnostics::code::LANGUAGE_STANDARD_COMPAT_UNCHECKED_EDGE,
        source: None,
        label: String::new(),
        span: None,
    }
}

/// The interface field the strict cross-language default points
/// the user at.
fn interface_field(language: cabin_core::SourceLanguage) -> &'static str {
    match language {
        cabin_core::SourceLanguage::C => "interface-c-standard",
        cabin_core::SourceLanguage::Cxx => "interface-cxx-standard",
    }
}

/// `path:line` of the declaration a [`DeclSite`] cites, for the
/// provenance chain.  Best-effort: an unreadable manifest or a
/// failed span lookup renders the bare path.
fn site_ref(site: &DeclSite) -> String {
    match site_line(site) {
        Some(line) => format!("{}:{line}", site.manifest_path.display()),
        None => site.manifest_path.display().to_string(),
    }
}

/// 1-based line number of the cited declaration.
fn site_line(site: &DeclSite) -> Option<usize> {
    let text = std::fs::read_to_string(&site.manifest_path).ok()?;
    let range = cabin_manifest::standard_field_span(&text, field_scope(&site.scope), site.field)?;
    Some(text[..range.start].bytes().filter(|&b| b == b'\n').count() + 1)
}

fn field_scope(scope: &DeclScope) -> cabin_manifest::StandardFieldScope<'_> {
    match scope {
        DeclScope::Package => cabin_manifest::StandardFieldScope::Package,
        DeclScope::Target(name) => cabin_manifest::StandardFieldScope::Target(name),
        DeclScope::Workspace => cabin_manifest::StandardFieldScope::Workspace,
    }
}

/// Load the consumer's manifest and re-locate its standard
/// declaration.  Best-effort: an unreadable file or a failed span
/// lookup renders the diagnostic without a snippet.
fn consumer_snippet(site: &DeclSite) -> (miette::NamedSource<String>, Option<miette::SourceSpan>) {
    let text = std::fs::read_to_string(&site.manifest_path).unwrap_or_default();
    let span = cabin_manifest::standard_field_span(&text, field_scope(&site.scope), site.field)
        .map(|range| miette::SourceSpan::new(range.start.into(), range.len()));
    (
        miette::NamedSource::new(site.manifest_path.display().to_string(), text),
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn render(diagnostic: &dyn miette::Diagnostic) -> String {
        let mut out = String::new();
        // A generous width keeps every asserted sentence on one
        // line; the default 80-column wrap would split the longer
        // messages mid-phrase.
        miette::GraphicalReportHandler::new_themed(miette::GraphicalTheme::unicode_nocolor())
            .without_cause_chain()
            .with_width(500)
            .render_report(&mut out, diagnostic)
            .unwrap();
        out
    }

    fn render_error(violation: &StandardCompatViolation, from_lockfile: bool) -> String {
        render(&violation_diagnostic(violation, from_lockfile))
    }

    fn min_violation() -> StandardCompatViolation {
        StandardCompatViolation {
            consumer: "app:app".to_owned(),
            language: cabin_core::SourceLanguage::Cxx,
            consumer_standard: "c++17",
            consumer_site: DeclSite {
                manifest_path: PathBuf::from("/nonexistent/app/cabin.toml"),
                scope: DeclScope::Target("app".to_owned()),
                field: "cxx-standard",
            },
            dependency: "liba:liba".to_owned(),
            dependency_package: "liba".to_owned(),
            dependency_version: "1.2.0".to_owned(),
            dependency_is_registry: true,
            requirement: EdgeRequirement::Min("c++20"),
            origin_target: "libb:libb".to_owned(),
            origin: RequirementOrigin::Declared {
                site: DeclSite {
                    manifest_path: PathBuf::from("/nonexistent/libb/cabin.toml"),
                    scope: DeclScope::Target("libb".to_owned()),
                    field: "interface-cxx-standard",
                },
            },
            chain: vec!["liba:liba".to_owned(), "libb:libb".to_owned()],
            consumer_manifest_path: PathBuf::from("/nonexistent/app/cabin.toml"),
            ignored: false,
            override_section: Some("[dependencies]".to_owned()),
        }
    }

    /// Pins the rendered shape of the transitive minimum-violation
    /// error: severity, stable code, the consumer -> dependency
    /// provenance arrow, origin citation, and all three remedies in
    /// order (raise, pin, override).
    #[test]
    fn renders_transitive_minimum_violation() {
        let rendered = render_error(&min_violation(), false);
        assert!(
            rendered.contains("cabin::language::standard_compat_violation"),
            "expected the stable code in: {rendered}"
        );
        assert!(
            rendered.contains(
                "`app:app` (c++17, /nonexistent/app/cabin.toml) -> `liba:liba` requires C++ \
                 consumers at `c++20` or newer via public dependency `libb:libb` \
                 (`interface-cxx-standard`, /nonexistent/libb/cabin.toml)"
            ),
            "expected the provenance-chain sentence in: {rendered}"
        );
        assert!(
            rendered.contains("raise `app:app`'s C++ standard to at least `c++20`"),
            "expected the raise remedy in: {rendered}"
        );
        assert!(
            rendered.contains("or pin `liba` to an older version (currently 1.2.0)"),
            "expected the pin remedy in: {rendered}"
        );
        // The always-on build-time enforcement independently
        // rejects minimum violations, so the override remedy - a
        // dead end here - is withheld.
        assert!(
            !rendered.contains("ignore-interface-standard"),
            "a minimum violation must not offer the override: {rendered}"
        );
        let raise = rendered.find("raise `app:app`").unwrap();
        let pin = rendered.find("or pin `liba`").unwrap();
        assert!(raise < pin, "remedies must read raise -> pin: {rendered}");
    }

    /// Snapshot of the provenance rendering with resolvable
    /// manifests: both sites carry `path:line` references and the
    /// consumer snippet is labeled.
    #[test]
    fn renders_provenance_chain_with_manifest_lines() {
        let dir = assert_fs::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::create_dir_all(dir.path().join("libb")).unwrap();
        let app_manifest = dir.path().join("app/cabin.toml");
        let libb_manifest = dir.path().join("libb/cabin.toml");
        std::fs::write(
            &app_manifest,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[target.app]\ntype = \
             \"executable\"\nsources = [\"src/main.cc\"]\ncxx-standard = \"c++17\"\n",
        )
        .unwrap();
        std::fs::write(
            &libb_manifest,
            "[package]\nname = \"libb\"\nversion = \"0.1.0\"\n\n[target.libb]\ntype = \
             \"library\"\nsources = [\"src/b.cc\"]\ninterface-cxx-standard = \"c++20\"\n",
        )
        .unwrap();
        let mut violation = min_violation();
        violation.consumer_site.manifest_path = app_manifest.clone();
        violation.consumer_manifest_path = app_manifest;
        let RequirementOrigin::Declared { site } = &mut violation.origin else {
            unreachable!("min_violation carries a declared origin");
        };
        site.manifest_path = libb_manifest;
        let rendered = render_error(&violation, false);
        // `cxx-standard` is on line 8 of the app manifest and
        // `interface-cxx-standard` on line 8 of libb's.  Normalize
        // Windows separators so the pinned form is one string.
        let normalized = rendered
            .replace(&dir.path().display().to_string(), "<dir>")
            .replace('\\', "/");
        assert!(
            normalized.contains(
                "`app:app` (c++17, <dir>/app/cabin.toml:8) -> `liba:liba` requires C++ \
                 consumers at `c++20` or newer via public dependency `libb:libb` \
                 (`interface-cxx-standard`, <dir>/libb/cabin.toml:8)"
            ),
            "expected line-numbered provenance in: {normalized}"
        );
        assert!(
            normalized.contains("cxx-standard = \"c++17\""),
            "expected the manifest snippet line in: {normalized}"
        );
        assert!(
            normalized.contains("`app:app` compiles C++ as `c++17`"),
            "expected the snippet label in: {normalized}"
        );
    }

    /// A direct (single-hop) declaration cites the dependency's own
    /// manifest and mentions no chain.
    #[test]
    fn renders_direct_violation_without_chain() {
        let mut violation = min_violation();
        violation.dependency = "libb:libb".to_owned();
        violation.chain = vec!["libb:libb".to_owned()];
        violation.dependency_is_registry = false;
        let rendered = render_error(&violation, false);
        assert!(
            !rendered.contains("via public dependency"),
            "a single-hop origin must not mention a chain: {rendered}"
        );
        assert!(
            !rendered.contains("pin `"),
            "a path dependency offers no pin remedy: {rendered}"
        );
    }

    /// A three-hop provenance renders the full chain, hop by hop.
    #[test]
    fn renders_longer_chains_hop_by_hop() {
        let mut violation = min_violation();
        violation.chain = vec![
            "liba:liba".to_owned(),
            "mid:mid".to_owned(),
            "libb:libb".to_owned(),
        ];
        let rendered = render_error(&violation, false);
        assert!(
            rendered
                .contains("via public dependency chain `liba:liba` -> `mid:mid` -> `libb:libb`"),
            "expected the full chain in: {rendered}"
        );
    }

    /// Header-only inference is marked as inferred, per the
    /// documented wording.
    #[test]
    fn renders_header_only_inference_marker() {
        let mut violation = min_violation();
        violation.dependency = "hdr:hdr".to_owned();
        violation.origin_target = "hdr:hdr".to_owned();
        violation.chain = vec!["hdr:hdr".to_owned()];
        violation.origin = RequirementOrigin::HeaderOnlyInference {
            site: DeclSite {
                manifest_path: PathBuf::from("/nonexistent/hdr/cabin.toml"),
                scope: DeclScope::Target("hdr".to_owned()),
                field: "cxx-standard",
            },
        };
        let rendered = render_error(&violation, false);
        assert!(
            rendered.contains(
                "(inferred from implementation standard `cxx-standard`, \
                 /nonexistent/hdr/cabin.toml)"
            ),
            "expected the inference marker in: {rendered}"
        );
    }

    /// A declared `"none"` renders the disabled-consumption wording
    /// and the forbidden help.
    #[test]
    fn renders_declared_none_as_disabled_consumption() {
        let mut violation = min_violation();
        violation.dependency = "libb:libb".to_owned();
        violation.chain = vec!["libb:libb".to_owned()];
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::DeclaredNone {
            site: DeclSite {
                manifest_path: PathBuf::from("/nonexistent/libb/cabin.toml"),
                scope: DeclScope::Target("libb".to_owned()),
                field: "interface-cxx-standard",
            },
        };
        let rendered = render_error(&violation, false);
        assert!(
            rendered
                .contains("C++ consumption was disabled by `interface-cxx-standard = \"none\"`"),
            "expected the disabled-consumption wording in: {rendered}"
        );
        assert!(
            rendered.contains("`libb:libb` cannot be consumed from C++ at any standard level"),
            "expected the forbidden help in: {rendered}"
        );
        // The build-time enforcement deliberately accepts `"none"`,
        // so here the override genuinely unblocks the command and
        // is offered - last, after the pin.
        assert!(
            rendered.contains(
                "as a last resort, `liba = { ..., ignore-interface-standard = true }` in the \
                 `[dependencies]` table of /nonexistent/app/cabin.toml leaves exactly this \
                 edge unchecked"
            ),
            "expected the override remedy in: {rendered}"
        );
        let pin = rendered.find("or pin `liba`").unwrap();
        let last_resort = rendered.find("as a last resort").unwrap();
        assert!(pin < last_resort, "the override stays last: {rendered}");
    }

    /// The strict C++-to-C default names the missing interface
    /// field.
    #[test]
    fn renders_cross_language_default() {
        let mut violation = min_violation();
        violation.language = cabin_core::SourceLanguage::C;
        violation.consumer_standard = "c11";
        violation.dependency = "cxxlib:cxxlib".to_owned();
        violation.dependency_package = "cxxlib".to_owned();
        violation.origin_target = "cxxlib:cxxlib".to_owned();
        violation.chain = vec!["cxxlib:cxxlib".to_owned()];
        violation.dependency_is_registry = false;
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::CrossLanguageDefault;
        let rendered = render_error(&violation, false);
        assert!(
            rendered.contains(
                "implements no C and declares no `interface-c-standard`, so it cannot be \
                 consumed from C"
            ),
            "expected the cross-language default wording in: {rendered}"
        );
    }

    /// An intra-package edge has no dependency entry to carry the
    /// override (the planner records no section), so the
    /// last-resort remedy is withheld.
    #[test]
    fn intra_package_edge_offers_no_override_remedy() {
        let mut violation = min_violation();
        violation.dependency = "app:lib".to_owned();
        violation.dependency_package = "app".to_owned();
        violation.dependency_is_registry = false;
        violation.chain = vec!["app:lib".to_owned()];
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::CrossLanguageDefault;
        violation.language = cabin_core::SourceLanguage::C;
        violation.consumer_standard = "c11";
        violation.override_section = None;
        let rendered = render_error(&violation, false);
        assert!(
            !rendered.contains("ignore-interface-standard"),
            "an intra-package edge must not suggest the override: {rendered}"
        );
    }

    /// A dependency whose resolved version came out of the lockfile
    /// appends the staleness explanation and the `cabin update`
    /// remedy, after the usual remedies and before the override.
    #[test]
    fn renders_lockfile_staleness_note_when_seeded() {
        let mut violation = min_violation();
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::DeclaredNone {
            site: DeclSite {
                manifest_path: PathBuf::from("/nonexistent/libb/cabin.toml"),
                scope: DeclScope::Target("libb".to_owned()),
                field: "interface-cxx-standard",
            },
        };
        let rendered = render_error(&violation, true);
        assert!(
            rendered.contains("this dependency's resolved version was loaded from cabin.lock"),
            "expected the lockfile provenance in: {rendered}"
        );
        assert!(
            rendered.contains("records version pins only"),
            "expected the pins-only explanation in: {rendered}"
        );
        assert!(
            rendered.contains(
                "if a standard declaration changed in a manifest after the lockfile was \
                 generated, run `cabin update` to re-resolve"
            ),
            "expected the likely cause and the update remedy in: {rendered}"
        );
        let update = rendered.find("cabin update").unwrap();
        let last_resort = rendered.find("as a last resort").unwrap();
        assert!(
            update < last_resort,
            "the override stays the last remedy: {rendered}"
        );
    }

    /// Fresh resolution renders no lockfile note.
    #[test]
    fn fresh_resolution_renders_no_lockfile_note() {
        let rendered = render_error(&min_violation(), false);
        assert!(
            !rendered.contains("cabin.lock") && !rendered.contains("cabin update"),
            "a fresh-resolution violation must not mention the lockfile: {rendered}"
        );
    }

    /// Path dependencies are never locked, so the note is withheld
    /// even when a caller flags the violation as lockfile-loaded.
    #[test]
    fn path_dependency_renders_no_lockfile_note_even_when_seeded() {
        let mut violation = min_violation();
        violation.dependency_is_registry = false;
        let rendered = render_error(&violation, true);
        assert!(
            !rendered.contains("cabin.lock") && !rendered.contains("cabin update"),
            "a path dependency's violation must not mention the lockfile: {rendered}"
        );
    }

    /// Violations render with miette's error cross.
    #[test]
    fn renders_error_severity() {
        let rendered = render_error(&min_violation(), false);
        assert!(
            rendered.contains('×'),
            "expected the error cross in: {rendered}"
        );
    }

    /// The unchecked-edge note names the edge, the override, and
    /// the declaring manifest, renders at advice severity under its
    /// own stable code, and points back at the violated origin.
    #[test]
    fn renders_unchecked_edge_note() {
        let mut violation = min_violation();
        violation.ignored = true;
        let rendered = render(&unchecked_note(&violation));
        assert!(
            rendered.contains("cabin::language::standard_compat_unchecked_edge"),
            "expected the stable note code in: {rendered}"
        );
        assert!(
            rendered.contains(
                "dependency edge `app:app` -> `liba:liba` is unchecked: \
                 `ignore-interface-standard = true` is set for `liba` in the \
                 `[dependencies]` table of /nonexistent/app/cabin.toml"
            ),
            "expected the unchecked-edge wording in: {rendered}"
        );
        assert!(
            rendered.contains(
                "remove `ignore-interface-standard` from the `[dependencies]` entry to \
                 re-enable the check"
            ),
            "expected the re-enable help in: {rendered}"
        );
        assert!(
            !rendered.contains('×') && !rendered.contains('⚠'),
            "a note must render below warning severity: {rendered}"
        );
    }

    /// An edge resolved through `[dev-dependencies]` names that
    /// table in both the last-resort remedy and the unchecked
    /// note, so following the advice never promotes a test-only
    /// dependency to a normal one.
    #[test]
    fn dev_resolved_edge_names_the_dev_dependencies_table() {
        let mut violation = min_violation();
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::DeclaredNone {
            site: DeclSite {
                manifest_path: PathBuf::from("/nonexistent/libb/cabin.toml"),
                scope: DeclScope::Target("libb".to_owned()),
                field: "interface-cxx-standard",
            },
        };
        violation.override_section = Some("[dev-dependencies]".to_owned());
        let rendered = render_error(&violation, false);
        assert!(
            rendered.contains(
                "in the `[dev-dependencies]` table of /nonexistent/app/cabin.toml leaves \
                 exactly this edge unchecked"
            ),
            "expected the dev table in the remedy: {rendered}"
        );
        violation.ignored = true;
        let note = render(&unchecked_note(&violation));
        assert!(
            note.contains("is set for `liba` in the `[dev-dependencies]` table of"),
            "expected the dev table in the note: {note}"
        );
        assert!(
            note.contains("remove `ignore-interface-standard` from the `[dev-dependencies]` entry"),
            "expected the dev table in the note help: {note}"
        );
    }

    /// `report` fails with the violation count once every diagnostic
    /// has rendered; the ignored-only path succeeds.
    #[test]
    fn report_gates_unless_ignored() {
        let violations = [min_violation()];
        let err = report(
            &violations,
            cabin_core::ColorChoice::Never,
            &BTreeSet::new(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert_eq!(
            message, "1 standard compatibility violation",
            "expected the bare violation count in: {message}"
        );

        let mut ignored = min_violation();
        ignored.ignored = true;
        report(&[ignored], cabin_core::ColorChoice::Never, &BTreeSet::new())
            .expect("ignored violations never gate");
    }
}
