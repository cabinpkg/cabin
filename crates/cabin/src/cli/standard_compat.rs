//! Rendering for the experimental `standard-compat` warnings.
//!
//! `cabin-build`'s post-resolution pass hands the CLI typed
//! [`StandardCompatViolation`] records (see
//! `cabin_build::standard_compat`); this module composes the
//! user-facing wording, re-locates the consumer's standard
//! declaration in its manifest for a labeled snippet
//! ([`cabin_manifest::standard_field_span`]), and renders each
//! record through `cabin-diagnostics` as a warning-severity
//! diagnostic.  Warnings never affect the exit status.

use std::collections::BTreeSet;
use std::fmt;

use anyhow::Result;
use cabin_build::{
    DeclScope, DeclSite, EdgeRequirement, RequirementOrigin, StandardCompatViolation,
};
use cabin_core::ExperimentalFeature;
use cabin_diagnostics::miette;

/// Whether the user opted into the pass for this invocation.
pub(crate) fn requested(unstable: &BTreeSet<ExperimentalFeature>) -> bool {
    unstable.contains(&ExperimentalFeature::StandardCompat)
}

/// Render every violation as a warning diagnostic on stderr.
/// Violations arrive pre-sorted from the planner; rendering keeps
/// that order.  `lockfile_pinned` holds the (name, version)
/// selections that came straight out of a pre-existing
/// `cabin.lock`; a violation whose dependency is among them also
/// explains the likely staleness cause and the re-resolve remedy.
pub(crate) fn report_warnings(
    warnings: &[StandardCompatViolation],
    color: cabin_core::ColorChoice,
    lockfile_pinned: &BTreeSet<(String, String)>,
) -> Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }
    let mut stderr = termcolor::StandardStream::stderr(cabin_diagnostics::termcolor_choice(color));
    for violation in warnings {
        let from_lockfile = lockfile_pinned.contains(&(
            violation.dependency_package.clone(),
            violation.dependency_version.clone(),
        ));
        let diagnostic = warning_diagnostic(violation, from_lockfile);
        cabin_diagnostics::render(&diagnostic, &mut stderr, color)?;
    }
    Ok(())
}

/// One rendered warning.  Implements [`miette::Diagnostic`] by hand
/// (matching `cabin_diagnostics::CodedMessage`) so the CLI does not
/// grow a direct dependency on the derive machinery.
#[derive(Debug)]
struct StandardCompatWarning {
    message: String,
    help: String,
    source: miette::NamedSource<String>,
    label: String,
    span: Option<miette::SourceSpan>,
}

impl fmt::Display for StandardCompatWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StandardCompatWarning {}

impl miette::Diagnostic for StandardCompatWarning {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(
            cabin_diagnostics::code::LANGUAGE_STANDARD_COMPAT_VIOLATION,
        ))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Warning)
    }

    fn help(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(&self.help))
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.span
            .is_some()
            .then_some(&self.source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        let span = self.span?;
        Some(Box::new(std::iter::once(
            miette::LabeledSpan::new_with_span(Some(self.label.clone()), span),
        )))
    }
}

fn warning_diagnostic(
    violation: &StandardCompatViolation,
    from_lockfile: bool,
) -> StandardCompatWarning {
    let lang = violation.language.human_label();

    // "imposed by `origin` via public dependency chain `a` -> `b`"
    // whenever the requirement was not declared by the direct
    // dependency itself.
    let imposed = if violation.chain.len() > 1 {
        format!(
            ", imposed by `{}` via public dependency chain `{}`",
            violation.origin_target,
            violation.chain.join("` -> `"),
        )
    } else {
        String::new()
    };

    let message = match (violation.requirement, &violation.origin) {
        (EdgeRequirement::Min(min), RequirementOrigin::Declared { site }) => format!(
            "target `{}` compiles {lang} as `{}`, but its dependency `{}` requires {lang} \
             consumers at `{min}` or newer{imposed} (`{}` in {})",
            violation.consumer,
            violation.consumer_standard,
            violation.dependency,
            site.field,
            site.manifest_path.display(),
        ),
        (EdgeRequirement::Min(min), RequirementOrigin::HeaderOnlyInference { site }) => format!(
            "target `{}` compiles {lang} as `{}`, but its dependency `{}` requires {lang} \
             consumers at `{min}` or newer{imposed} (inferred from implementation standard: \
             `{}` in {})",
            violation.consumer,
            violation.consumer_standard,
            violation.dependency,
            site.field,
            site.manifest_path.display(),
        ),
        (EdgeRequirement::Forbidden, RequirementOrigin::DeclaredNone { site }) => format!(
            "target `{}` compiles {lang}, but its dependency `{}` cannot be consumed from \
             {lang}: {lang} consumption was disabled by `{} = \"none\"`{imposed} (in {})",
            violation.consumer,
            violation.dependency,
            site.field,
            site.manifest_path.display(),
        ),
        (EdgeRequirement::Forbidden, RequirementOrigin::CrossLanguageDefault) => format!(
            "target `{}` compiles {lang}, but its dependency `{}` implements no {lang} and \
             declares no `{}`, so it cannot be consumed from {lang}{imposed}",
            violation.consumer,
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
    let help = match violation.requirement {
        EdgeRequirement::Min(min) => format!(
            "raise `{}`'s {lang} standard to at least `{min}`{pin}{lockfile_note}",
            violation.consumer,
        ),
        EdgeRequirement::Forbidden => format!(
            "`{}` cannot be consumed from {lang} at any standard level{pin}{lockfile_note}",
            violation.origin_target,
        ),
    };

    let (source, span) = consumer_snippet(&violation.consumer_site);
    StandardCompatWarning {
        message,
        help,
        source,
        label: format!(
            "`{}` compiles {lang} as `{}`",
            violation.consumer, violation.consumer_standard,
        ),
        span,
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

/// Load the consumer's manifest and re-locate its standard
/// declaration.  Best-effort: an unreadable file or a failed span
/// lookup renders the diagnostic without a snippet.
fn consumer_snippet(site: &DeclSite) -> (miette::NamedSource<String>, Option<miette::SourceSpan>) {
    let text = std::fs::read_to_string(&site.manifest_path).unwrap_or_default();
    let scope = match &site.scope {
        DeclScope::Package => cabin_manifest::StandardFieldScope::Package,
        DeclScope::Target(name) => cabin_manifest::StandardFieldScope::Target(name),
        DeclScope::Workspace => cabin_manifest::StandardFieldScope::Workspace,
    };
    let span = cabin_manifest::standard_field_span(&text, scope, site.field)
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
        }
    }

    /// Pins the rendered shape of the transitive minimum-violation
    /// warning: severity, stable code, consumer standard, origin
    /// citation, chain, and both remedies.
    #[test]
    fn renders_transitive_minimum_violation() {
        let rendered = render(&warning_diagnostic(&min_violation(), false));
        assert!(
            rendered.contains("cabin::language::standard_compat_violation"),
            "expected the stable code in: {rendered}"
        );
        assert!(
            rendered.contains(
                "target `app:app` compiles C++ as `c++17`, but its dependency `liba:liba` \
                 requires C++ consumers at `c++20` or newer"
            ),
            "expected the core sentence in: {rendered}"
        );
        assert!(
            rendered.contains(
                "imposed by `libb:libb` via public dependency chain `liba:liba` -> `libb:libb`"
            ),
            "expected the provenance chain in: {rendered}"
        );
        assert!(
            rendered.contains("`interface-cxx-standard` in /nonexistent/libb/cabin.toml"),
            "expected the origin declaration citation in: {rendered}"
        );
        assert!(
            rendered.contains("raise `app:app`'s C++ standard to at least `c++20`"),
            "expected the raise remedy in: {rendered}"
        );
        assert!(
            rendered.contains("or pin `liba` to an older version (currently 1.2.0)"),
            "expected the pin remedy in: {rendered}"
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
        let rendered = render(&warning_diagnostic(&violation, false));
        assert!(
            !rendered.contains("public dependency chain"),
            "a single-hop origin must not mention a chain: {rendered}"
        );
        assert!(
            !rendered.contains("pin `"),
            "a path dependency offers no pin remedy: {rendered}"
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
        let rendered = render(&warning_diagnostic(&violation, false));
        assert!(
            rendered.contains(
                "(inferred from implementation standard: `cxx-standard` in \
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
        let rendered = render(&warning_diagnostic(&violation, false));
        assert!(
            rendered
                .contains("C++ consumption was disabled by `interface-cxx-standard = \"none\"`"),
            "expected the disabled-consumption wording in: {rendered}"
        );
        assert!(
            rendered.contains("`libb:libb` cannot be consumed from C++ at any standard level"),
            "expected the forbidden help in: {rendered}"
        );
    }

    /// The strict C++-to-C default names the missing interface
    /// field.
    #[test]
    fn renders_cross_language_default() {
        let mut violation = min_violation();
        violation.language = cabin_core::SourceLanguage::C;
        violation.consumer_standard = "c11";
        violation.dependency = "cxxlib:cxxlib".to_owned();
        violation.origin_target = "cxxlib:cxxlib".to_owned();
        violation.chain = vec!["cxxlib:cxxlib".to_owned()];
        violation.dependency_is_registry = false;
        violation.requirement = EdgeRequirement::Forbidden;
        violation.origin = RequirementOrigin::CrossLanguageDefault;
        let rendered = render(&warning_diagnostic(&violation, false));
        assert!(
            rendered.contains(
                "implements no C and declares no `interface-c-standard`, so it cannot be \
                 consumed from C"
            ),
            "expected the cross-language default wording in: {rendered}"
        );
    }

    /// A warning renders with a labeled snippet when the manifest
    /// is readable, and the label carries the consumer's standard.
    #[test]
    fn renders_snippet_when_manifest_is_readable() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = dir.path().join("cabin.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[target.app]\nkind = \
             \"executable\"\nsources = [\"src/main.cc\"]\ncxx-standard = \"c++17\"\n",
        )
        .unwrap();
        let mut violation = min_violation();
        violation.consumer_site.manifest_path = manifest;
        let rendered = render(&warning_diagnostic(&violation, false));
        assert!(
            rendered.contains("cxx-standard = \"c++17\""),
            "expected the manifest snippet line in: {rendered}"
        );
        assert!(
            rendered.contains("`app:app` compiles C++ as `c++17`"),
            "expected the snippet label in: {rendered}"
        );
    }

    /// A dependency whose resolved version came out of the lockfile
    /// appends the staleness explanation and the `cabin update`
    /// remedy to its warning, after the usual remedies.
    #[test]
    fn renders_lockfile_staleness_note_when_seeded() {
        let rendered = render(&warning_diagnostic(&min_violation(), true));
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
        // The usual remedies stay in place, ahead of the note.
        assert!(
            rendered.contains("raise `app:app`'s C++ standard to at least `c++20`"),
            "expected the raise remedy in: {rendered}"
        );
        assert!(
            rendered.contains("or pin `liba` to an older version (currently 1.2.0)"),
            "expected the pin remedy in: {rendered}"
        );
    }

    /// Fresh resolution renders no lockfile note.
    #[test]
    fn fresh_resolution_renders_no_lockfile_note() {
        let rendered = render(&warning_diagnostic(&min_violation(), false));
        assert!(
            !rendered.contains("cabin.lock") && !rendered.contains("cabin update"),
            "a fresh-resolution warning must not mention the lockfile: {rendered}"
        );
    }

    /// Path dependencies are never locked, so the note is withheld
    /// even when a caller flags the violation as lockfile-loaded.
    #[test]
    fn path_dependency_renders_no_lockfile_note_even_when_seeded() {
        let mut violation = min_violation();
        violation.dependency_is_registry = false;
        let rendered = render(&warning_diagnostic(&violation, true));
        assert!(
            !rendered.contains("cabin.lock") && !rendered.contains("cabin update"),
            "a path dependency's warning must not mention the lockfile: {rendered}"
        );
    }

    /// Warning severity renders with miette's warning glyph, not
    /// the error cross.
    #[test]
    fn renders_as_warning_severity() {
        let rendered = render(&warning_diagnostic(&min_violation(), false));
        assert!(
            rendered.contains('⚠'),
            "expected the warning glyph in: {rendered}"
        );
        assert!(
            !rendered.contains('×'),
            "a warning must not render the error cross: {rendered}"
        );
    }
}
