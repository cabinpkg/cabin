use camino::Utf8PathBuf;

use super::target::parse_target_kind;
use super::*;
use cabin_core::{DependencyKind, DependencySource, PortDepSource, TargetKind, ValidationError};

fn parse_project(input: &str) -> Package {
    parse_manifest_str(input)
        .expect("manifest should parse")
        .package
        .expect("manifest should contain [package]")
}

fn parse_project_err(input: &str) -> ManifestError {
    parse_manifest_str(input).expect_err("manifest should fail to parse")
}

fn requirement<S>(min: S) -> cabin_core::InterfaceRequirement<S> {
    cabin_core::InterfaceRequirement::Requirement(cabin_core::StandardRequirement {
        min,
        max: None,
    })
}

const MINIMAL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"
    "#;

const FULL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"
        cxx-standard = "c++17"

        [target.hello]
        type = "executable"
        sources = ["src/main.cc"]
        include-dirs = ["include"]
        defines = ["HELLO=1"]
        deps = []
    "#;

#[test]
fn parses_minimal_manifest() {
    let package = parse_project(MINIMAL);
    assert_eq!(package.name.as_str(), "hello");
    assert_eq!(package.version.to_string(), "0.1.0");
    assert!(package.targets.is_empty());
    assert!(package.dependencies.is_empty());
}

#[test]
fn parses_executable_target() {
    let package = parse_project(FULL);
    assert_eq!(package.targets.len(), 1);
    let target = &package.targets[0];
    assert_eq!(target.name.as_str(), "hello");
    assert_eq!(target.kind, TargetKind::Executable);
    assert_eq!(target.sources, vec![Utf8PathBuf::from("src/main.cc")]);
    assert_eq!(target.include_dirs, vec![Utf8PathBuf::from("include")]);
    assert_eq!(target.defines, vec!["HELLO=1".to_string()]);
    assert!(target.deps.is_empty());
}

#[test]
fn target_source_and_include_paths_with_spaces_round_trip_as_utf8() {
    // A path containing a space is valid UTF-8 and must survive
    // manifest parsing into a `Utf8PathBuf` byte-for-byte: camino
    // does not normalize or split on whitespace.
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"
            cxx-standard = "c++17"

            [target.hello]
            type = "executable"
            sources = ["src/my source.cc"]
            include-dirs = ["my include dir"]
        "#;
    let package = parse_project(manifest);
    let target = &package.targets[0];
    assert_eq!(target.sources, vec![Utf8PathBuf::from("src/my source.cc")]);
    assert_eq!(
        target.include_dirs,
        vec![Utf8PathBuf::from("my include dir")]
    );
}

#[test]
fn target_unknown_fields_are_rejected() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "library"
            sources = ["src/lib.cc"]
            not-a-real-key = ["include"]
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::Toml(source) => {
            let message = source.to_string();
            assert!(
                message.contains("unknown field `not-a-real-key`"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected TOML parse error, got {other:?}"),
    }
}

#[test]
fn target_include_dirs_is_kebab_case() {
    let manifest = r#"
[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello]
type = "executable"
sources = ["src/main.cc"]
include-dirs = ["include"]
"#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].include_dirs,
        vec![Utf8PathBuf::from("include")]
    );
}

#[test]
fn target_include_dirs_snake_case_is_rejected_as_unknown_field() {
    let manifest = r#"
[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]
include_dirs = ["include"]
"#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::Toml(source) => {
            let message = source.to_string();
            assert!(
                message.contains("unknown field"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected TOML parse error, got {other:?}"),
    }
}

#[test]
fn header_only_kind_is_accepted() {
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header-only"
            include-dirs = ["include"]
            interface-cxx-standard = "c++17"
        "#;
    let package = parse_project(manifest);
    let target = &package.targets[0];
    assert_eq!(target.kind, TargetKind::HeaderOnly);
    assert!(target.sources.is_empty());
}

#[test]
fn header_only_snake_case_kind_is_rejected() {
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header_only"
            include-dirs = ["include"]
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::UnknownTargetType { target, value } => {
            assert_eq!(target, "hdr");
            assert_eq!(value, "header_only");
        }
        other => panic!("expected UnknownTargetType, got {other:?}"),
    }
}

#[test]
fn header_only_rejects_sources() {
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header-only"
            sources = ["src/empty.cc"]
            include-dirs = ["include"]
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::HeaderOnlyDeclaresSources { target } => {
            assert_eq!(target, "hdr");
        }
        other => panic!("expected HeaderOnlyDeclaresSources, got {other:?}"),
    }
}

#[test]
fn package_and_target_language_standards_parse() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            c-standard = "c11"
            cxx-standard = "c++17"
            interface-cxx-standard = "c++14"

            [target.core]
            type = "library"
            sources = ["src/core.cc"]
            cxx-standard = "c++20"
            interface-cxx-standard = "c++17"
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.language.c_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CStandard::C11
        ))
    );
    assert_eq!(
        package.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CxxStandard::Cxx17
        ))
    );
    assert_eq!(
        package.language.interface_cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(requirement(
            cabin_core::CxxStandard::Cxx14
        )))
    );
    assert_eq!(package.language.interface_c_standard, None);
    let core = &package.targets[0];
    assert_eq!(
        core.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CxxStandard::Cxx20
        ))
    );
    assert_eq!(
        core.language.interface_cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(requirement(
            cabin_core::CxxStandard::Cxx17
        )))
    );
    assert_eq!(core.language.c_standard, None);
}

#[test]
fn gnu_dialect_spellings_are_rejected_as_unknown_values() {
    for (field, value) in [
        ("c-standard", "gnu11"),
        ("cxx-standard", "gnu++20"),
        ("interface-cxx-standard", "gnu++17"),
    ] {
        let manifest = format!(
            r#"
                [package]
                name = "foo"
                version = "0.1.0"
                {field} = "{value}"
            "#
        );
        let err = parse_project_err(&manifest);
        let message = err.to_string();
        assert!(
            message.contains(&format!("standard `{value}`")),
            "unexpected error for {field}: {message}"
        );
        // Plain unknown-value diagnostics: no gnu-extensions hint.
        assert!(
            !message.contains("gnu-extensions"),
            "unexpected error for {field}: {message}"
        );
    }
}

#[test]
fn standard_aliases_normalize_immediately() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            c-standard = "c90"
            cxx-standard = "c++03"
            interface-cxx-standard = "c++03"
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.language.c_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CStandard::C89
        ))
    );
    assert_eq!(
        package.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CxxStandard::Cxx98
        ))
    );
    assert_eq!(
        package.language.interface_cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(requirement(
            cabin_core::CxxStandard::Cxx98
        )))
    );
}

#[test]
fn gnu_extensions_parses_at_package_and_target_level() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = "c++17"
            gnu-extensions = true

            [target.core]
            type = "library"
            sources = ["src/core.cc"]
            gnu-extensions = false
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.language.gnu_extensions, Some(true));
    assert_eq!(package.targets[0].language.gnu_extensions, Some(false));

    // Non-boolean values are rejected by the manifest schema.
    let err = parse_project_err(
        r#"
            [package]
            name = "foo"
            version = "0.1.0"
            gnu-extensions = "true"
        "#,
    );
    assert!(matches!(err, ManifestError::Toml(_)), "got {err:?}");
}

#[test]
fn none_is_accepted_only_on_interface_fields() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = "c++17"
            interface-c-standard = "none"

            [target.core]
            type = "library"
            sources = ["src/core.cc"]
            interface-cxx-standard = "none"
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.language.interface_c_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::InterfaceRequirement::None
        ))
    );
    assert_eq!(
        package.targets[0].language.interface_cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::InterfaceRequirement::None
        ))
    );

    for field in ["c-standard", "cxx-standard"] {
        let manifest = format!(
            r#"
                [package]
                name = "foo"
                version = "0.1.0"
                {field} = "none"
            "#
        );
        let err = parse_project_err(&manifest);
        assert!(
            err.to_string().contains("`none` is only valid on"),
            "unexpected error for {field}: {err}"
        );
    }
}

#[test]
fn range_like_standard_values_get_the_reserved_diagnostic() {
    for (field, value) in [
        ("c-standard", ">=c11"),
        ("cxx-standard", "c++17,c++20"),
        ("interface-c-standard", "<=c17"),
        ("interface-cxx-standard", ">=c++17"),
    ] {
        let manifest = format!(
            r#"
                [package]
                name = "foo"
                version = "0.1.0"
                {field} = "{value}"
            "#
        );
        let err = parse_project_err(&manifest);
        assert!(
            err.to_string().contains("reserved for a future version"),
            "unexpected error for {field}: {err}"
        );
    }
}

#[test]
fn invalid_standard_value_lists_valid_spellings() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = "c++2x"
        "#;
    let err = parse_project_err(manifest);
    let message = err.to_string();
    assert!(message.contains("c++2x"), "unexpected error: {message}");
    assert!(
        message.contains("c++98, c++11, c++14, c++17, c++20, c++23, c++26"),
        "unexpected error: {message}"
    );
}

#[test]
fn c_standard_value_in_cxx_slot_is_rejected() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = "c11"
        "#;
    let err = parse_project_err(manifest);
    assert!(
        err.to_string().contains("unknown C++ standard `c11`"),
        "unexpected error: {err}"
    );
}

#[test]
fn interface_standard_on_executable_like_targets_is_rejected() {
    for (kind, field, value) in [
        ("executable", "interface-cxx-standard", "c++17"),
        ("test", "interface-c-standard", "c11"),
        ("example", "interface-cxx-standard", "c++20"),
    ] {
        let manifest = format!(
            r#"
                [package]
                name = "foo"
                version = "0.1.0"

                [target.app]
                type = "{kind}"
                sources = ["src/main.cc"]
                {field} = "{value}"
            "#
        );
        let err = parse_project_err(&manifest);
        let message = err.to_string();
        assert!(
            message.contains(field) && message.contains(kind),
            "expected rejection naming `{field}` on `{kind}`, got: {message}"
        );
    }
}

#[test]
fn implementation_standard_on_executable_is_accepted() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"

            [target.app]
            type = "executable"
            sources = ["src/main.cc"]
            cxx-standard = "c++20"
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CxxStandard::Cxx20
        ))
    );
}

#[test]
fn compiled_language_without_standard_is_rejected() {
    // No built-in defaults: a compiled language must have an
    // effective standard from the target or `[package]` tier.
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"

            [target.app]
            type = "executable"
            sources = ["src/main.cc"]
        "#;
    let err = parse_project_err(manifest);
    match &err {
        ManifestError::MissingLanguageStandard {
            target,
            language,
            field,
        } => {
            assert_eq!(target, "app");
            assert_eq!(*language, "C++");
            assert_eq!(*field, "cxx-standard");
        }
        other => panic!("expected MissingLanguageStandard, got {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains("cxx-standard") && message.contains("workspace = true"),
        "message must name the field and the workspace opt-in: {message}"
    );

    // A declared C++ standard does not cover C sources.
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = "c++17"

            [target.app]
            type = "executable"
            sources = ["src/main.cc", "src/util.c"]
        "#;
    let err = parse_project_err(manifest);
    match &err {
        ManifestError::MissingLanguageStandard {
            language, field, ..
        } => {
            assert_eq!(*language, "C");
            assert_eq!(*field, "c-standard");
        }
        other => panic!("expected MissingLanguageStandard, got {other:?}"),
    }
}

#[test]
fn target_level_standard_covers_its_compiled_language() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"

            [target.app]
            type = "executable"
            sources = ["src/main.c"]
            c-standard = "c17"
        "#;
    parse_project(manifest);
}

#[test]
fn workspace_marker_counts_as_declared_standard() {
    // The marker resolves (or errors) at workspace load; for
    // coverage purposes opting in is declaring.
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"
            cxx-standard = { workspace = true }

            [target.app]
            type = "executable"
            sources = ["src/main.cc"]
        "#;
    parse_project(manifest);
}

#[test]
fn header_only_without_interface_standard_is_rejected() {
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header-only"
            include-dirs = ["include"]
        "#;
    let err = parse_project_err(manifest);
    match &err {
        ManifestError::HeaderOnlyMissingInterfaceStandard { target } => {
            assert_eq!(target, "hdr");
        }
        other => panic!("expected HeaderOnlyMissingInterfaceStandard, got {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains("interface-c-standard") && message.contains("interface-cxx-standard"),
        "message must name both interface fields: {message}"
    );

    // An implementation standard alone does not describe the
    // headers' requirement.
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"
            cxx-standard = "c++17"

            [target.hdr]
            type = "header-only"
            include-dirs = ["include"]
        "#;
    let err = parse_project_err(manifest);
    assert!(matches!(
        err,
        ManifestError::HeaderOnlyMissingInterfaceStandard { .. }
    ));
}

#[test]
fn header_only_interface_standard_at_either_level_is_accepted() {
    for fields in [
        ("", "interface-c-standard = \"c11\""),
        ("interface-cxx-standard = \"c++17\"", ""),
        ("interface-c-standard = { workspace = true }", ""),
    ] {
        let (package_field, target_field) = fields;
        let manifest = format!(
            r#"
                [package]
                name = "hdr"
                version = "0.1.0"
                {package_field}

                [target.hdr]
                type = "header-only"
                include-dirs = ["include"]
                {target_field}
            "#
        );
        parse_project(&manifest);
    }
}

#[test]
fn misspelled_standard_field_hits_generic_unknown_field_path() {
    let manifest = r#"
            [package]
            name = "foo"
            version = "0.1.0"

            [target.app]
            type = "executable"
            sources = ["src/main.cc"]
            cxx-std = "c++20"
        "#;
    let err = parse_project_err(manifest);
    assert!(
        err.to_string().contains("unknown field"),
        "unexpected error: {err}"
    );
}

#[test]
fn executable_accepts_mixed_c_and_cpp_sources() {
    // Target kinds describe artifact role only; the parser
    // accepts both C/C++ source extensions under any
    // executable / library / test / example target.  Source-
    // language classification is per-file in the planner.
    let manifest = r#"
            [package]
            name = "exe"
            version = "0.1.0"
            c-standard = "c11"
            cxx-standard = "c++17"

            [target.exe]
            type = "executable"
            sources = ["src/main.c", "src/helper.cc"]
        "#;
    let package = parse_project(manifest);
    let target = &package.targets[0];
    assert_eq!(target.kind, TargetKind::Executable);
    assert_eq!(
        target.sources,
        vec![
            Utf8PathBuf::from("src/main.c"),
            Utf8PathBuf::from("src/helper.cc"),
        ]
    );
}

#[test]
fn omitted_optional_arrays_default_to_empty() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "executable"
        "#;
    let package = parse_project(manifest);
    let target = &package.targets[0];
    assert!(target.sources.is_empty());
    assert!(target.include_dirs.is_empty());
    assert!(target.defines.is_empty());
    assert!(target.deps.is_empty());
}

#[test]
fn parses_all_supported_target_kinds() {
    let manifest = r#"
            [package]
            name = "many"
            version = "0.1.0"
            cxx-standard = "c++17"
            interface-cxx-standard = "c++17"

            [target.a]
            type = "library"

            [target.b]
            type = "executable"

            [target.c]
            type = "test"
            sources = ["tests/e.cc"]

            [target.d]
            type = "example"
            sources = ["examples/f.cc"]

            [target.e]
            type = "header-only"
            include-dirs = ["include"]
        "#;
    let package = parse_project(manifest);
    let kinds: Vec<TargetKind> = package.targets.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TargetKind::Library,
            TargetKind::Executable,
            TargetKind::Test,
            TargetKind::Example,
            TargetKind::HeaderOnly,
        ]
    );
}

/// The old `c_*` / `cpp_*` target kind strings are no longer
/// recognized.  A manifest using either must fail with
/// [`ManifestError::UnknownTargetType`] so existing users see
/// an explicit migration prompt rather than silent acceptance.
#[test]
fn legacy_c_and_cpp_target_kinds_are_rejected() {
    for legacy in [
        "cpp_library",
        "cpp_header_only",
        "cpp_executable",
        "cpp_test",
        "cpp_example",
        "c_library",
        "c_header_only",
        "c_executable",
        "c_test",
        "c_example",
    ] {
        let manifest = format!(
            "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"{legacy}\"\n"
        );
        let err = parse_manifest_str(&manifest).expect_err("legacy kind should be rejected");
        match err {
            ManifestError::UnknownTargetType { target, value } => {
                assert_eq!(target, "hello", "wrong target name in error for {legacy}");
                assert_eq!(value, legacy, "wrong value in error for {legacy}");
            }
            other => panic!("expected UnknownTargetType for {legacy}, got {other:?}"),
        }
    }
}

#[test]
fn empty_manifest_errors() {
    let err = parse_manifest_str("").unwrap_err();
    assert!(matches!(err, ManifestError::EmptyManifest));
}

#[test]
fn missing_project_name_errors() {
    let manifest = r#"
            [package]
            version = "0.1.0"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(err, ManifestError::Toml(_)));
}

#[test]
fn invalid_semver_errors() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "not-a-version"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::Version { value, .. } => assert_eq!(value, "not-a-version"),
        other => panic!("expected ManifestError::Version, got {other:?}"),
    }
}

#[test]
fn unknown_target_type_errors() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "wasm_executable"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::UnknownTargetType { target, value } => {
            assert_eq!(target, "hello");
            assert_eq!(value, "wasm_executable");
        }
        other => panic!("expected UnknownTargetType, got {other:?}"),
    }
}

#[test]
fn unknown_target_dep_now_passes_through_to_planner() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "executable"
            deps = ["external"]
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].deps[0],
        cabin_core::TargetDep::private("external")
    );
}

#[test]
fn empty_package_name_errors() {
    let manifest = r#"
            [package]
            name = ""
            version = "0.1.0"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Validation(ValidationError::EmptyPackageName)
    ));
}

#[test]
fn whitespace_package_name_errors() {
    let manifest = r#"
            [package]
            name = "hello world"
            version = "0.1.0"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Validation(ValidationError::PackageNameContainsWhitespace(_))
    ));
}

#[test]
fn empty_target_name_errors() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.""]
            type = "executable"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Validation(ValidationError::EmptyTargetName)
    ));
}

#[test]
fn whitespace_target_name_errors() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target."has space"]
            type = "executable"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Validation(ValidationError::TargetNameContainsWhitespace(_))
    ));
}

/// A quoted target name with path metacharacters must be rejected
/// at manifest-parse time.  The build planner joins
/// `target.name.as_str()` into object, executable, and Cargo target
/// directories, so accepting `[target."/tmp/out"]` would let a
/// malicious package write artifacts outside `--build-dir`.
#[test]
fn path_unsafe_target_name_errors() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target."/tmp/out"]
            type = "executable"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(
        matches!(
            err,
            ManifestError::Validation(ValidationError::UnsafeTargetName(_))
        ),
        "expected UnsafeTargetName, got {err:?}"
    );
}

/// Cross-package dep references use the `package:target` form.  The
/// `:` is outside the path-safe target-name grammar, so deps are
/// stored as raw strings (not `TargetName`) and validated only at
/// resolution time.  Pin the round-trip so the type relaxation does
/// not silently regress.
#[test]
fn qualified_cross_package_dep_round_trips_as_raw_string() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = ["other-pkg:lib"]
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].deps,
        vec![cabin_core::TargetDep::private("other-pkg:lib")]
    );
}

/// The `{ name = ... }` table form without `public` means the same
/// private edge as the string shorthand.
#[test]
fn target_dep_table_form_defaults_private() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = [{ name = "fmt" }]
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].deps,
        vec![cabin_core::TargetDep::private("fmt")]
    );
}

#[test]
fn target_dep_table_form_accepts_explicit_public() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = ["local", { name = "fmt", public = true }, { name = "ssl", public = false }]
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].deps,
        vec![
            cabin_core::TargetDep::private("local"),
            cabin_core::TargetDep {
                reference: "fmt".to_owned(),
                public: true,
            },
            cabin_core::TargetDep::private("ssl"),
        ]
    );
}

/// The table form keeps the reference exactly as written: a bare
/// name that will later resolve through the same-name shorthand
/// (`fmt` -> `fmt:fmt`) stays pre-alias here, because alias
/// resolution happens in the planner against a concrete package
/// graph - visibility is attached to the *resolved* edge there.
#[test]
fn target_dep_public_works_on_alias_and_qualified_references() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = [{ name = "fmt", public = true }, { name = "foo:opt", public = true }]
        "#;
    let package = parse_project(manifest);
    assert_eq!(
        package.targets[0].deps,
        vec![
            cabin_core::TargetDep {
                reference: "fmt".to_owned(),
                public: true,
            },
            cabin_core::TargetDep {
                reference: "foo:opt".to_owned(),
                public: true,
            },
        ]
    );
}

#[test]
fn target_dep_table_form_rejects_unknown_fields() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = [{ name = "fmt", export = true }]
        "#;
    let err = parse_project_err(manifest);
    let rendered = err.to_string();
    assert!(
        rendered.contains("export"),
        "error should name the unknown field, got: {rendered}"
    );
}

#[test]
fn target_dep_table_form_requires_name() {
    let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = [{ public = true }]
        "#;
    let err = parse_project_err(manifest);
    let rendered = err.to_string();
    assert!(
        rendered.contains("name"),
        "error should name the missing field, got: {rendered}"
    );
}

#[test]
fn parse_target_kind_rejects_unknown() {
    let err = parse_target_kind("t", "exe").unwrap_err();
    match err {
        ManifestError::UnknownTargetType { target, value } => {
            assert_eq!(target, "t");
            assert_eq!(value, "exe");
        }
        other => panic!("expected UnknownTargetType, got {other:?}"),
    }
}

// -------------------------------------------------------------------
// [dependencies] / [workspace] (path deps, workspace tables)
// -------------------------------------------------------------------

#[test]
fn parses_path_dependency() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet" }
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.dependencies.len(), 1);
    let dep = &package.dependencies[0];
    assert_eq!(dep.name.as_str(), "greet");
    assert_eq!(
        dep.source,
        DependencySource::Path(Utf8PathBuf::from("../greet"))
    );
}

#[test]
fn parses_workspace_with_members() {
    let manifest = r#"
            [workspace]
            members = ["packages/*", "tools/hello"]
        "#;
    let parsed = parse_manifest_str(manifest).unwrap();
    assert!(parsed.package.is_none());
    let ws = parsed.workspace.expect("[workspace] should be present");
    assert_eq!(ws.members, vec!["packages/*", "tools/hello"]);
}

#[test]
fn pure_workspace_preserves_root_policy_settings() {
    let manifest = r#"
            [workspace]
            members = ["packages/*"]

            [profile.release]
            opt-level = 0

            [toolchain]
            cxx = "clang++"

            [build]
            compiler-wrapper = "ccache"

            [patch]
            fmt = { path = "../fmt" }
        "#;
    let parsed = parse_manifest_str(manifest).unwrap();
    assert!(parsed.package.is_none());

    let release = cabin_core::ProfileName::new("release").unwrap();
    assert_eq!(
        parsed
            .root_settings
            .profiles
            .get(&release)
            .and_then(|p| p.opt_level),
        Some(cabin_core::OptLevel::O0)
    );
    assert_eq!(
        parsed
            .root_settings
            .toolchain
            .general
            .get(cabin_core::ToolKind::CxxCompiler)
            .map(cabin_core::ToolSpec::display)
            .as_deref(),
        Some("clang++")
    );
    assert_eq!(
        parsed.root_settings.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Name("ccache".into()),
        })
    );
    assert_eq!(parsed.root_settings.patches.entries.len(), 1);
}

#[test]
fn parses_workspace_with_root_project() {
    let manifest = r#"
            [package]
            name = "root"
            version = "0.1.0"

            [workspace]
            members = ["packages/*"]
        "#;
    let parsed = parse_manifest_str(manifest).unwrap();
    assert!(parsed.package.is_some());
    assert!(parsed.workspace.is_some());
}

#[test]
fn invalid_workspace_members_errors() {
    let manifest = r#"
            [workspace]
            members = "not-an-array"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(err, ManifestError::Toml(_)));
}

// -------------------------------------------------------------------
// versioned dependencies
// -------------------------------------------------------------------

#[test]
fn parses_string_version_dependency() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10.0.0 <11.0.0"
        "#;
    let package = parse_project(manifest);
    let dep = &package.dependencies[0];
    assert_eq!(dep.name.as_str(), "fmt");
    match &dep.source {
        DependencySource::Version(req) => {
            assert!(req.matches(&semver::Version::parse("10.2.1").unwrap()));
            assert!(!req.matches(&semver::Version::parse("11.0.0").unwrap()));
        }
        other => panic!("expected Version source, got {other:?}"),
    }
}

/// A scoped `[package]` name and scoped (quoted) dependency keys
/// parse to their full verbatim identity; TOML requires the quotes
/// because `/` is not a bare-key character.  Both requirement
/// spellings (string and rich table) work unchanged.
#[test]
fn parses_scoped_package_and_dependency_names() {
    let manifest = r#"
            [package]
            name = "fmtlib/fmt"
            version = "0.1.0"

            [dependencies]
            "gabime/spdlog" = ">=1.14 <2"
            "google/gtest" = { version = "^1.14" }
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.name.as_str(), "fmtlib/fmt");
    let names: Vec<&str> = package
        .dependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(names.contains(&"gabime/spdlog"));
    assert!(names.contains(&"google/gtest"));
}

#[test]
fn parses_table_version_dependency() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = "^1.13.0" }
        "#;
    let package = parse_project(manifest);
    match &package.dependencies[0].source {
        DependencySource::Version(req) => {
            assert!(req.matches(&semver::Version::parse("1.13.5").unwrap()));
            assert!(!req.matches(&semver::Version::parse("2.0.0").unwrap()));
        }
        other => panic!("expected Version source, got {other:?}"),
    }
}

#[test]
fn parses_mixed_path_and_version_dependencies() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet" }
            fmt = ">=10"
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.dependencies.len(), 2);
    // BTreeMap iteration is sorted; 'fmt' < 'greet'.
    let fmt = &package.dependencies[0];
    let greet = &package.dependencies[1];
    assert_eq!(fmt.name.as_str(), "fmt");
    assert!(matches!(fmt.source, DependencySource::Version(_)));
    assert_eq!(greet.name.as_str(), "greet");
    assert!(matches!(greet.source, DependencySource::Path(_)));
}

#[test]
fn dependency_with_both_path_and_version_errors() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet", version = "1.0" }
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::DependencyHasPathAndVersion { name } => assert_eq!(name, "greet"),
        other => panic!("expected DependencyHasPathAndVersion, got {other:?}"),
    }
}

#[test]
fn dependency_with_neither_path_nor_version_errors() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { }
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::DependencyMissingSource { name } => assert_eq!(name, "greet"),
        other => panic!("expected DependencyMissingSource, got {other:?}"),
    }
}

#[test]
fn invalid_string_version_requirement_errors() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">>>"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::InvalidDependencyRequirement {
            name, requirement, ..
        } => {
            assert_eq!(name, "fmt");
            assert_eq!(requirement, ">>>");
        }
        other => panic!("expected InvalidDependencyRequirement, got {other:?}"),
    }
}

#[test]
fn dependency_table_with_truly_unknown_field_errors_at_toml_layer() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = "1.0", "made-up-key" = true }
        "#;
    // Fields that aren't in the typed `RawDependencyTable` shape
    // are still rejected at the TOML layer via `deny_unknown_fields`.
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(err, ManifestError::Toml(_)));
}

#[test]
fn parses_supported_version_requirement_shapes() {
    // Smoke-test the requirement subset called out in the spec.
    let cases: &[(&str, &str)] = &[
        ("=1.2.3", "1.2.3"),
        (">=1.2.3", "1.2.3"),
        (">1.2.3", "1.2.4"),
        ("<=1.2.3", "1.2.3"),
        ("<2.0.0", "1.9.9"),
        (">=1.2.3 <2.0.0", "1.5.0"),
        ("^1.2.3", "1.5.0"),
        ("^0.2.3", "0.2.4"),
        ("^0.0.3", "0.0.3"),
        ("*", "9.9.9"),
    ];
    for (req_str, version_str) in cases {
        let manifest = format!(
            r#"
                [package]
                name = "app"
                version = "0.1.0"

                [dependencies]
                pkg = "{req_str}"
                "#,
        );
        let package = parse_project(&manifest);
        match &package.dependencies[0].source {
            DependencySource::Version(req) => {
                let v = semver::Version::parse(version_str).unwrap();
                assert!(
                    req.matches(&v),
                    "requirement {req_str:?} should match {version_str}"
                );
            }
            other => panic!("expected Version source for {req_str:?}, got {other:?}"),
        }
    }
}

// -----------------------------------------------------------------
// features
// -----------------------------------------------------------------

#[test]
fn features_parse_with_default_and_implications() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            default = ["simd"]
            simd = []
            ssl = []
            full = ["simd", "ssl"]
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.features.default, vec!["simd".to_string()]);
    assert_eq!(package.features.features.len(), 3);
    assert_eq!(
        package.features.features["full"],
        vec!["simd".to_string(), "ssl".into()]
    );
}

#[test]
fn features_reject_reserved_default_as_normal_feature() {
    // The `default` key is reserved.  TOML may reject this
    // as a duplicate key before Cabin's semantic validation
    // runs; either failure mode protects the invariant.
    let conflict = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            default = []
            "default" = []
        "#;
    let err = parse_manifest_str(conflict);
    assert!(err.is_err());
}

#[test]
fn features_unknown_reference_errors() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            full = ["ssl"]
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::Validation(ValidationError::UnknownFeatureReference {
            referrer,
            referenced,
        }) => {
            assert_eq!(referrer, "full");
            assert_eq!(referenced, "ssl");
        }
        other => panic!("expected UnknownFeatureReference, got {other:?}"),
    }
}

#[test]
fn features_invalid_name_errors() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            "foo/bar" = []
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::Validation(ValidationError::InvalidConfigName { kind, value }) => {
            assert_eq!(kind, "feature");
            assert_eq!(value, "foo/bar");
        }
        other => panic!("expected InvalidConfigName, got {other:?}"),
    }
}

#[test]
fn target_required_features_parse_into_model() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"
            cxx-standard = "c++17"

            [features]
            ssl = []

            [target.tls]
            type = "library"
            sources = ["src/tls.cc"]
            required-features = ["ssl"]
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.targets[0].required_features, vec!["ssl"]);
}

#[test]
fn target_required_features_reject_undeclared_feature() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"
            cxx-standard = "c++17"

            [target.tls]
            type = "library"
            sources = ["src/tls.cc"]
            required-features = ["ssl"]
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::Validation(ValidationError::UnknownRequiredFeature { target, feature }) => {
            assert_eq!(target, "tls");
            assert_eq!(feature, "ssl");
        }
        other => panic!("expected UnknownRequiredFeature, got {other:?}"),
    }
}

#[test]
fn features_cycle_errors() {
    let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            a = ["b"]
            b = ["a"]
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::Validation(ValidationError::FeatureCycle(_)) => {}
        other => panic!("expected FeatureCycle, got {other:?}"),
    }
}

// -----------------------------------------------------------------
// Dependency kinds: parsing the package-dep tables, rejecting
// unsupported syntax, and round-tripping kind information into
// `Package::dependencies`.
// -----------------------------------------------------------------

fn deps_of_kind(package: &Package, kind: DependencyKind) -> Vec<&cabin_core::Dependency> {
    package.dependencies_of_kind(kind).collect()
}

#[test]
fn parses_normal_dependencies_with_explicit_kind() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name.as_str(), "fmt");
    assert_eq!(deps[0].kind, DependencyKind::Normal);
}

#[test]
fn parses_each_package_dep_kind_section() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"

            [dev-dependencies]
            gtest = "^1.14"
        "#,
    );
    for (kind, expected_name) in [
        (DependencyKind::Normal, "fmt"),
        (DependencyKind::Dev, "gtest"),
    ] {
        let deps = deps_of_kind(&package, kind);
        assert_eq!(deps.len(), 1, "{kind:?} should have one dep");
        assert_eq!(deps[0].name.as_str(), expected_name);
        assert_eq!(deps[0].kind, kind);
    }
}

#[test]
fn unknown_top_level_table_is_rejected_by_deny_unknown_fields() {
    // Generic coverage that any unrecognized top-level table is
    // rejected by serde's `deny_unknown_fields`.  Use a token
    // that does not correspond to any past or planned feature
    // so future grammar changes are not pinned to a specific
    // name.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [not-a-real-table]
            anything = 1
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(
        matches!(err, ManifestError::Toml(_) | ManifestError::TomlAt(_)),
        "expected unknown-table TOML error, got {err:?}"
    );
}

#[test]
fn parses_system_dependencies() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true }
            openssl = { version = ">=3", system = true }
        "#,
    );
    assert_eq!(package.system_dependencies.len(), 2);
    // System deps round-trip through metadata sorted by name (BTreeMap iteration).
    let by_name: BTreeMap<&str, &cabin_core::SystemDependency> = package
        .system_dependencies
        .iter()
        .map(|sd| (sd.name.as_str(), sd))
        .collect();
    assert_eq!(by_name["openssl"].version, ">=3");
    assert_eq!(by_name["openssl"].kind, DependencyKind::Normal);
    assert_eq!(by_name["zlib"].version, ">=1.2");
    assert_eq!(by_name["zlib"].kind, DependencyKind::Normal);
}

#[test]
fn system_dependency_rejects_required_field() {
    // `required` was removed from the manifest surface: every
    // `system = true` dep is required.  The unknown field is
    // rejected by `deny_unknown_fields` on the raw table; the
    // resulting diagnostic must name the field by name so users
    // know what to remove.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            openssl = { version = ">=3", system = true, required = false }
        "#;
    let err = parse_project_err(manifest);
    let rendered = err.to_string();
    assert!(
        rendered.contains("unknown field `required`"),
        "diagnostic should name the unknown field; got {rendered}",
    );
}

#[test]
fn system_flag_routes_per_kind() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true }

            [dev-dependencies]
            gtest = { version = "^1.14", system = true }
        "#,
    );
    let by_name: BTreeMap<&str, &cabin_core::SystemDependency> = package
        .system_dependencies
        .iter()
        .map(|sd| (sd.name.as_str(), sd))
        .collect();
    assert_eq!(by_name["zlib"].kind, DependencyKind::Normal);
    assert_eq!(by_name["gtest"].kind, DependencyKind::Dev);
}

#[test]
fn same_package_name_across_different_kinds_is_allowed() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"

            [dev-dependencies]
            fmt = ">=10"
        "#,
    );
    // The duplicate-policy spec: same name across kinds is allowed.
    assert_eq!(deps_of_kind(&package, DependencyKind::Normal).len(), 1);
    assert_eq!(deps_of_kind(&package, DependencyKind::Dev).len(), 1);
}

#[test]
fn dev_dependency_path_dependency_is_accepted_in_metadata() {
    // Dev deps with `path = "..."` are declaration-only for
    // ordinary commands; we accept them at parse time and surface
    // them through `package.dependencies` so `cabin metadata`
    // lists them (`cabin test` activates them as graph edges).
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dev-dependencies]
            harness = { path = "../harness" }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Dev);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name.as_str(), "harness");
    assert_eq!(deps[0].kind, DependencyKind::Dev);
    assert!(matches!(deps[0].source, DependencySource::Path(_)));
}

#[test]
fn optional_dependency_is_parsed_with_optional_flag() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = ">=10", optional = true }
        "#;
    let package = parse_project(manifest);
    let dep = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "fmt")
        .unwrap();
    assert!(dep.optional);
    assert_eq!(dep.kind, DependencyKind::Normal);
    assert!(dep.features.is_empty());
    assert!(dep.default_features);
}

#[test]
fn optional_on_dev_dependency_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dev-dependencies]
            gtest = { version = "^1", optional = true }
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::OptionalNotSupportedForKind { name, kind } => {
            assert_eq!(name, "gtest");
            assert_eq!(kind, DependencyKind::Dev);
        }
        other => panic!("expected OptionalNotSupportedForKind, got {other:?}"),
    }
}

#[test]
fn optional_on_system_dependency_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true, optional = true }
        "#;
    // The parser enforces that `system = true` is incompatible
    // with `optional` (and `path`, `workspace`, `features`,
    // `default-features`, `git`, `registry`, `source`).  The
    // first conflicting field in declaration order surfaces
    // via `SystemConflictsWith`.
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::SystemConflictsWith { name, field, .. } => {
            assert_eq!(name, "zlib");
            assert_eq!(field, "optional");
        }
        other => panic!("expected SystemConflictsWith, got {other:?}"),
    }
}

#[test]
fn ignore_interface_standard_is_parsed_per_edge() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = ">=10", ignore-interface-standard = true }
            zlib = { version = ">=1.2" }

            [dev-dependencies]
            gtest = { version = "^1", ignore-interface-standard = true }
        "#;
    let package = parse_project(manifest);
    let flag_of = |name: &str| {
        package
            .dependencies
            .iter()
            .find(|d| d.name.as_str() == name)
            .unwrap()
            .ignore_interface_standard
    };
    assert!(flag_of("fmt"));
    assert!(!flag_of("zlib"), "the opt-out is per-edge, never implied");
    assert!(flag_of("gtest"), "dev edges accept the opt-out too");
}

#[test]
fn ignore_interface_standard_string_form_stays_unset() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"
        "#;
    let package = parse_project(manifest);
    assert!(!package.dependencies[0].ignore_interface_standard);
}

#[test]
fn ignore_interface_standard_is_parsed_on_conditional_dependencies() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.dependencies]
            fmt = { version = ">=10", ignore-interface-standard = true }
        "#;
    let package = parse_project(manifest);
    let dep = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "fmt")
        .unwrap();
    assert!(dep.ignore_interface_standard);
    assert!(dep.condition.is_some(), "the condition must be preserved");
}

#[test]
fn ignore_interface_standard_on_system_dependency_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true, ignore-interface-standard = true }
        "#;
    match parse_manifest_str(manifest).unwrap_err() {
        ManifestError::SystemConflictsWith { name, field, .. } => {
            assert_eq!(name, "zlib");
            assert_eq!(field, "ignore-interface-standard");
        }
        other => panic!("expected SystemConflictsWith, got {other:?}"),
    }
}

#[test]
fn unsupported_dependency_section_yields_toml_error() {
    // `[test-dependencies]` is not a recognized top-level
    // section.  `RawManifest` declares `deny_unknown_fields`
    // so a typo cannot silently drop dependencies - the
    // TOML layer surfaces an `unknown field` error pointing
    // at the offending section name.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [test-dependencies]
            gtest = "^1"
        "#;
    let err = parse_project_err(manifest);
    let rendered = format!("{err}");
    assert!(
        rendered.contains("test-dependencies"),
        "error should name the offending section: {rendered}"
    );
}

#[test]
fn target_cfg_dependency_round_trips_condition() {
    // `[target.'cfg(...)'.<kind>]` lands as ordinary
    // `Dependency` entries with `condition` populated.
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.dependencies]
            fmt = ">=10"

            [target.'cfg(arch = "x86_64")'.dev-dependencies]
            gtest = "^1.14"

            [target.'cfg(arch = "aarch64")'.dependencies]
            zlib = { version = ">=1.2", system = true }
        "#,
    );
    let normal = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(normal.len(), 1);
    assert_eq!(normal[0].name.as_str(), "fmt");
    assert_eq!(
        normal[0].condition.as_ref().map(ToString::to_string),
        Some("os = \"linux\"".to_owned()),
    );
    let dev = deps_of_kind(&package, DependencyKind::Dev);
    assert_eq!(dev.len(), 1);
    assert_eq!(
        dev[0].condition.as_ref().map(ToString::to_string),
        Some("arch = \"x86_64\"".to_owned()),
    );
    assert_eq!(package.system_dependencies.len(), 1);
    let sys = &package.system_dependencies[0];
    assert_eq!(sys.name.as_str(), "zlib");
    assert_eq!(
        sys.condition.as_ref().map(ToString::to_string),
        Some("arch = \"aarch64\"".to_owned()),
    );
}

#[test]
fn target_cfg_workspace_inheritance_is_rejected() {
    // `workspace = true` inside `[target.'cfg(...)'.<kind>]`
    // is rejected so a single workspace key never silently
    // means different things on different hosts.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.dependencies]
            fmt = { workspace = true }
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(
        matches!(err, ManifestError::WorkspaceInsideConditionalTarget { .. }),
        "expected WorkspaceInsideConditionalTarget, got {err:?}",
    );
}

#[test]
fn target_cfg_invalid_predicate_yields_clear_error() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(host_endian = "little")'.dependencies]
            fmt = ">=10"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::InvalidTargetCfg { raw, .. } => {
            assert!(raw.contains("host_endian"), "{raw}");
        }
        other => panic!("expected InvalidTargetCfg, got {other:?}"),
    }
}

#[test]
fn manifest_without_profile_tables_round_trips() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"
        "#,
    );
    assert!(package.profiles.is_empty());
}

#[test]
fn dev_override_is_parsed() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.dev]
            opt-level = 1
            assertions = false
        "#,
    );
    let dev_name = cabin_core::ProfileName::new("dev").unwrap();
    let dev = package.profiles.get(&dev_name).expect("dev profile parsed");
    assert!(dev.inherits.is_none());
    assert_eq!(dev.opt_level, Some(cabin_core::OptLevel::O1));
    assert_eq!(dev.assertions, Some(false));
    assert!(dev.debug.is_none());
}

#[test]
fn release_override_is_parsed_with_string_opt_level() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = "s"
            debug = true
        "#,
    );
    let release_name = cabin_core::ProfileName::new("release").unwrap();
    let r = package.profiles.get(&release_name).unwrap();
    assert_eq!(r.opt_level, Some(cabin_core::OptLevel::S));
    assert_eq!(r.debug, Some(true));
}

#[test]
fn custom_profile_with_inherits_is_parsed() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.relwithdebinfo]
            inherits = "release"
            debug = true
        "#,
    );
    let custom = cabin_core::ProfileName::new("relwithdebinfo").unwrap();
    let p = package.profiles.get(&custom).unwrap();
    assert_eq!(
        p.inherits.as_ref().map(cabin_core::ProfileName::as_str),
        Some("release")
    );
    assert_eq!(p.debug, Some(true));
    assert!(p.opt_level.is_none());
}

#[test]
fn profile_release_accepts_cxxflags_override() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = 3
            cxxflags = ["-march=native"]
        "#;
    let package = parse_project(manifest);
    let release = cabin_core::ProfileName::new("release".to_owned()).expect("valid profile name");
    let prof = package.profiles.get(&release).expect("release profile");
    let build = prof.build.as_ref().expect("override build flags");
    assert_eq!(build.cxxflags, vec!["-march=native".to_owned()]);
}

#[test]
fn top_level_profile_accepts_cflags_cxxflags_and_ldflags() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            cflags = ["-std=c99"]
            cxxflags = ["-fno-rtti"]
            ldflags = ["-Wl,--as-needed"]
        "#;
    let package = parse_project(manifest);
    let build = &package.build.general;
    assert_eq!(build.cflags, vec!["-std=c99".to_owned()]);
    assert_eq!(build.cxxflags, vec!["-fno-rtti".to_owned()]);
    assert_eq!(build.ldflags, vec!["-Wl,--as-needed".to_owned()]);
}

#[test]
fn profile_accepts_link_libs_in_base_and_cfg_tables() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            link-libs = ["m"]

            [target.'cfg(family = "unix")'.profile]
            link-libs = ["pthread", "dl"]
        "#;
    let package = parse_project(manifest);
    assert_eq!(package.build.general.link_libs, vec!["m".to_owned()]);
    let conditional = &package.build.conditional;
    assert_eq!(conditional.len(), 1);
    assert_eq!(
        conditional[0].flags.link_libs,
        vec!["pthread".to_owned(), "dl".to_owned()]
    );
}

#[test]
fn target_named_profile_overlays_accept_valid_undeclared_names() {
    for (key, expected) in [
        ("dev", "dev"),
        ("release", "release"),
        ("static", "static"),
        ("release-lto", "release-lto"),
        ("'release.lto'", "release.lto"),
        ("cflags", "cflags"),
    ] {
        let manifest = format!(
            r#"
                [package]
                name = "app"
                version = "0.1.0"

                [target.'cfg(os = "linux")'.profile.{key}]
                ldflags = ["-{expected}"]
            "#
        );
        let package = parse_project(&manifest);
        let conditional = &package.build.conditional;
        assert_eq!(conditional.len(), 1, "overlay key {key}");
        assert_eq!(
            conditional[0]
                .profile
                .as_ref()
                .map(cabin_core::ProfileName::as_str),
            Some(expected),
            "overlay key {key}",
        );
        assert_eq!(
            conditional[0].flags.ldflags,
            vec![format!("-{expected}")],
            "overlay key {key}",
        );
    }
}

#[test]
fn target_named_profile_overlay_accepts_all_array_fields() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile.release]
            defines = ["LINUX_RELEASE"]
            include-dirs = ["include/linux-release"]
            cflags = ["-fno-plt"]
            cxxflags = ["-fno-semantic-interposition"]
            ldflags = ["-Wl,--as-needed"]
            link-libs = ["pthread", "dl"]
        "#,
    );
    let overlay = &package.build.conditional[0];
    assert_eq!(
        overlay
            .profile
            .as_ref()
            .map(cabin_core::ProfileName::as_str),
        Some("release"),
    );
    assert_eq!(overlay.flags.defines, vec!["LINUX_RELEASE"]);
    assert_eq!(
        overlay.flags.include_dirs,
        vec![camino::Utf8PathBuf::from("include/linux-release")],
    );
    assert_eq!(overlay.flags.cflags, vec!["-fno-plt"]);
    assert_eq!(overlay.flags.cxxflags, vec!["-fno-semantic-interposition"],);
    assert_eq!(overlay.flags.ldflags, vec!["-Wl,--as-needed"]);
    assert_eq!(overlay.flags.link_libs, vec!["pthread", "dl"]);
}

#[test]
fn target_general_and_named_profile_layers_compose() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile]
            ldflags = ["general"]

            [target.'cfg(os = "linux")'.profile.release]
            ldflags = ["named"]
        "#,
    );
    assert_eq!(package.build.conditional.len(), 2);
    assert!(package.build.conditional[0].profile.is_none());
    assert_eq!(package.build.conditional[0].flags.ldflags, vec!["general"],);
    assert_eq!(
        package.build.conditional[1]
            .profile
            .as_ref()
            .map(cabin_core::ProfileName::as_str),
        Some("release"),
    );
    assert_eq!(package.build.conditional[1].flags.ldflags, vec!["named"]);
}

#[test]
fn target_profile_tables_preserve_manifest_order() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile.release]
            ldflags = ["linux"]

            [target.'cfg(arch = "x86_64")'.profile.release]
            ldflags = ["x86_64"]
        "#,
    );
    let ldflags: Vec<&str> = package
        .build
        .conditional
        .iter()
        .map(|layer| layer.flags.ldflags[0].as_str())
        .collect();
    assert_eq!(ldflags, vec!["linux", "x86_64"]);
}

#[test]
fn target_named_profile_overlay_rejects_invalid_profile_name() {
    let err = parse_project_err(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile.'bad/name']
            ldflags = ["bad"]
        "#,
    );
    assert!(matches!(err, ManifestError::InvalidProfileName { .. }));
}

#[test]
fn target_named_profile_overlay_rejects_definition_and_unknown_fields() {
    let cases = [
        (
            r#"inherits = "release""#,
            "`inherits` is not allowed",
            "profile inheritance must be defined by `[profile.release-lto]`",
        ),
        (
            "debug = false",
            "`debug` is not allowed",
            "may only contain array flag fields",
        ),
        (
            r#"opt-level = "z""#,
            "`opt-level` is not allowed",
            "may only contain array flag fields",
        ),
        (
            "assertions = false",
            "`assertions` is not allowed",
            "may only contain array flag fields",
        ),
        (
            r#"toolchain = { cxx = "clang++" }"#,
            "`toolchain` is not allowed",
            "may only contain array flag fields",
        ),
        (
            r#"foo = ["bar"]"#,
            "unknown field `foo`",
            "supported fields are defines, include-dirs, cflags, cxxflags, ldflags, and link-libs",
        ),
    ];

    for (field, expected, help) in cases {
        let manifest = format!(
            r#"
                [package]
                name = "app"
                version = "0.1.0"

                [target.'cfg(os = "linux")'.profile.release-lto]
                {field}
            "#
        );
        let message = parse_project_err(&manifest).to_string();
        assert!(message.contains(expected), "{message}");
        assert!(message.contains(help), "{message}");
        assert!(
            message.contains(r#"[target.'cfg(os = "linux")'.profile.release-lto]"#),
            "{message}",
        );
    }
}

#[test]
fn profile_is_not_a_cfg_key() {
    let err = parse_project_err(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(profile = "release")'.profile]
            ldflags = ["bad"]
        "#,
    );
    assert!(matches!(err, ManifestError::InvalidTargetCfg { .. }));
    assert!(err.to_string().contains("profile"));
}

#[test]
fn feature_cfg_on_profile_table_is_accepted() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(feature = "single-threaded")'.profile]
            defines = ["SQLITE_THREADSAFE=0"]
        "#;
    let package = parse_project(manifest);
    let conditional = &package.build.conditional;
    assert_eq!(conditional.len(), 1);
    assert!(conditional[0].condition.references_feature());
    assert_eq!(
        conditional[0].flags.defines,
        vec!["SQLITE_THREADSAFE=0".to_owned()]
    );
}

#[test]
fn feature_cfg_on_dependency_table_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(feature = "simd")'.dependencies]
            simdlib = { path = "../simdlib" }
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::FeatureConditionNotAllowedHere { table, .. } => {
            assert_eq!(table, "dependencies");
        }
        other => panic!("expected FeatureConditionNotAllowedHere, got {other:?}"),
    }
}

#[test]
fn compiler_cfg_on_profile_table_is_accepted() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(all(cxx = "clang", cxx_version = ">=18"))'.profile]
            cxxflags = ["-stdlib=libc++"]
        "#;
    let package = parse_project(manifest);
    let conditional = &package.build.conditional;
    assert_eq!(conditional.len(), 1);
    assert!(conditional[0].condition.references_compiler());
    assert_eq!(
        conditional[0].condition.to_string(),
        r#"all(cxx = "clang", cxx_version = ">=18")"#
    );
    assert_eq!(
        conditional[0].flags.cxxflags,
        vec!["-stdlib=libc++".to_owned()]
    );
}

#[test]
fn compiler_cfg_on_dependency_table_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(cxx = "clang")'.dependencies]
            fmt = { path = "../fmt" }
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::CompilerConditionNotAllowedHere { table, .. } => {
            assert_eq!(table, "dependencies");
        }
        other => panic!("expected CompilerConditionNotAllowedHere, got {other:?}"),
    }
}

#[test]
fn compiler_cfg_on_dev_dependency_table_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(cc = "gcc")'.dev-dependencies]
            catch2 = { path = "../catch2" }
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::CompilerConditionNotAllowedHere { table, .. } => {
            assert_eq!(table, "dev-dependencies");
        }
        other => panic!("expected CompilerConditionNotAllowedHere, got {other:?}"),
    }
}

#[test]
fn compiler_cfg_on_toolchain_table_is_rejected() {
    // Circular by construction: the toolchain table picks the
    // compiler, so it cannot itself be gated on the detected one.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(cxx_version = ">=18")'.toolchain]
            cxx = "clang++"
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::CompilerConditionNotAllowedHere { table, .. } => {
            assert_eq!(table, "toolchain");
        }
        other => panic!("expected CompilerConditionNotAllowedHere, got {other:?}"),
    }
}

#[test]
fn feature_cfg_on_workspace_root_toolchain_is_rejected() {
    // A pure workspace root (no [package]) never reaches
    // project_from_raw, yet still captures conditional toolchain
    // settings that are evaluated platform-only.  The feature-cfg check
    // runs before the package/workspace-root split, so it must reject
    // this too rather than silently ignoring it.
    let manifest = r#"
            [workspace]
            members = ["a"]

            [target.'cfg(feature = "fast")'.toolchain]
            cxx = "clang++"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::FeatureConditionNotAllowedHere { table, .. } => {
            assert_eq!(table, "toolchain");
        }
        other => panic!("expected FeatureConditionNotAllowedHere, got {other:?}"),
    }
}

#[test]
fn invalid_link_lib_name_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            link-libs = ["-levil"]
        "#;
    let err = parse_project_err(manifest);
    assert!(
        matches!(err, ManifestError::InvalidBuildFlags(_)),
        "expected InvalidBuildFlags, got {err:?}"
    );
}

#[test]
fn unknown_profile_field_is_rejected() {
    // Generic coverage for `deny_unknown_fields` on
    // `[profile.<name>]`.  The field name is intentionally a
    // sentinel so this test is not pinned to a specific
    // hypothetical knob.
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            not-a-real-key = "x"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    assert!(matches!(err, ManifestError::Toml(_)));
    let msg = err.to_string();
    assert!(msg.contains("not-a-real-key"), "{msg}");
}

#[test]
fn invalid_opt_level_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = "fast"
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid opt-level"), "{msg}");
}

#[test]
fn invalid_profile_name_is_rejected() {
    let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.".release"]
            opt-level = 0
        "#;
    let err = parse_manifest_str(manifest).unwrap_err();
    match err {
        ManifestError::InvalidProfileName { value } => {
            assert_eq!(value, ".release");
        }
        other => panic!("expected InvalidProfileName, got {other:?}"),
    }
}

#[test]
fn workspace_dependency_kind_is_preserved() {
    // The `workspace = true` opt-in is only valid inside a
    // package-dep section; the kind on the resulting
    // `Dependency` matches the section it was declared in.
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { workspace = true }

            [dev-dependencies]
            gtest = { workspace = true }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].source, DependencySource::Workspace);
    let deps = deps_of_kind(&package, DependencyKind::Dev);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].source, DependencySource::Workspace);
}

// ---------------------------------------------------------------
// Foundation-port dependency form
// ---------------------------------------------------------------

#[test]
fn parses_port_dependency() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1" }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name.as_str(), "zlib");
    match &deps[0].source {
        DependencySource::Port(PortDepSource::Path(p)) => {
            assert_eq!(p, &Utf8PathBuf::from("../ports/zlib/1.3.1"));
        }
        other => panic!("expected Port, got {other:?}"),
    }
}

/// Shared body for the `rejects_port_combined_with_*` cases: a
/// `port-path` dependency combined with `extra` must surface
/// [`ManifestError::PortDependencyHasOtherSource`] naming
/// `conflicting_field`.
fn assert_port_rejects_other_source(extra: &str, conflicting_field: &str) {
    let err = parse_project_err(&format!(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = {{ port-path = "../ports/zlib/1.3.1", {extra} }}
        "#
    ));
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, conflicting_field, "case `{conflicting_field}`");
        }
        other => panic!(
            "case `{conflicting_field}`: expected PortDependencyHasOtherSource, got {other:?}"
        ),
    }
}

#[test]
fn rejects_port_combined_with_path() {
    assert_port_rejects_other_source(r#"path = "../zlib""#, "path");
}

#[test]
fn rejects_port_combined_with_version() {
    assert_port_rejects_other_source(r#"version = "1.0""#, "version");
}

#[test]
fn rejects_port_combined_with_workspace() {
    assert_port_rejects_other_source("workspace = true", "workspace");
}

#[test]
fn rejects_port_combined_with_system() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", system = true, version = "1.0" }
        "#,
    );
    // The system router runs first and surfaces a clear
    // SystemConflictsWith for the `port-path` field.
    match err {
        ManifestError::SystemConflictsWith { field, .. } => {
            assert_eq!(field, "port-path");
        }
        other => panic!("expected SystemConflictsWith on `port-path`, got {other:?}"),
    }
}

#[test]
fn port_path_dependency_honors_features() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", features = ["x"], default-features = false }
        "#,
    );
    let dep = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "zlib")
        .expect("zlib port dep");
    assert_eq!(dep.features, vec!["x".to_owned()]);
    assert!(!dep.default_features);
}

#[test]
fn port_builtin_dependency_honors_features() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            sqlite3 = { port = true, version = "^3", features = ["single-threaded"] }
        "#,
    );
    let dep = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "sqlite3")
        .expect("sqlite3 port dep");
    assert_eq!(dep.features, vec!["single-threaded".to_owned()]);
    assert!(dep.default_features);
}

#[test]
fn rejects_port_with_empty_feature_name() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", features = [""] }
        "#,
    );
    assert!(
        matches!(err, ManifestError::EmptyDependencyFeatureName { .. }),
        "expected EmptyDependencyFeatureName, got {err:?}"
    );
}

#[test]
fn rejects_port_with_optional() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", optional = true }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "optional");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
}

#[test]
fn treats_optional_false_as_absent_on_port_path_dep() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", optional = false }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert!(!deps[0].optional);
    match &deps[0].source {
        DependencySource::Port(PortDepSource::Path(p)) => {
            assert_eq!(p, &Utf8PathBuf::from("../ports/zlib/1.3.1"));
        }
        other => panic!("expected port path source, got {other:?}"),
    }
}

#[test]
fn parses_builtin_port_with_version_requirement() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3" }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name.as_str(), "zlib");
    match &deps[0].source {
        DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
            assert_eq!(name.as_str(), "zlib");
            assert_eq!(version_req.to_string(), "^1.3");
        }
        other => panic!("expected Builtin, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_without_version() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true }
        "#,
    );
    assert!(
        matches!(err, ManifestError::PortDependencyMissingVersion { ref name } if name == "zlib"),
        "expected PortDependencyMissingVersion, got {err:?}"
    );
}

#[test]
fn rejects_port_path_with_version() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", version = "^1.3" }
        "#,
    );
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, "version");
        }
        other => panic!("expected PortDependencyHasOtherSource for version, got {other:?}"),
    }
}

#[test]
fn parses_path_port_dependency_via_port_path_field() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1" }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    match &deps[0].source {
        DependencySource::Port(PortDepSource::Path(p)) => {
            assert_eq!(p, &Utf8PathBuf::from("../ports/zlib/1.3.1"));
        }
        other => panic!("expected Path, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_combined_with_port_path() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, port-path = "../ports/zlib/1.3.1" }
        "#,
    );
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, "port-path");
        }
        other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_combined_with_path() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", path = "../zlib" }
        "#,
    );
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, "path");
        }
        other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_combined_with_workspace() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", workspace = true }
        "#,
    );
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, "workspace");
        }
        other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
    }
}

#[test]
fn port_true_honors_features_and_default_features() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", features = ["x"], default-features = false }
        "#,
    );
    let dep = package
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "zlib")
        .expect("zlib port dep");
    assert_eq!(dep.features, vec!["x".to_owned()]);
    assert!(!dep.default_features);
}

#[test]
fn rejects_port_true_combined_with_optional() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", optional = true }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "optional");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
}

#[test]
fn treats_optional_false_as_absent_on_builtin_port_dep() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", optional = false }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    assert_eq!(deps.len(), 1);
    assert!(!deps[0].optional);
    match &deps[0].source {
        DependencySource::Port(PortDepSource::Builtin { name, .. }) => {
            assert_eq!(name.as_str(), "zlib");
        }
        other => panic!("expected builtin port source, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_with_invalid_version() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "not-a-version" }
        "#,
    );
    match err {
        ManifestError::InvalidDependencyRequirement {
            name, requirement, ..
        } => {
            assert_eq!(name, "zlib");
            assert_eq!(requirement, "not-a-version");
        }
        other => panic!("expected InvalidDependencyRequirement, got {other:?}"),
    }
}

#[test]
fn treats_port_false_as_absent() {
    let package = parse_project(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = false, path = "../zlib" }
        "#,
    );
    let deps = deps_of_kind(&package, DependencyKind::Normal);
    match &deps[0].source {
        DependencySource::Path(p) => assert_eq!(p, &Utf8PathBuf::from("../zlib")),
        other => panic!("expected Path (port = false is treated as absent), got {other:?}"),
    }
}

// ---------------------------------------------------------------
// [build] compiler-wrapper parsing
// ---------------------------------------------------------------

#[test]
fn build_parses_compiler_wrapper() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = "ccache"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Name("ccache".into()),
        })
    );
}

#[test]
fn build_accepts_none_to_explicitly_disable() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = "none"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Disabled)
    );
}

#[test]
fn build_parses_sccache_compiler_wrapper() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = "sccache"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Name("sccache".into()),
        })
    );
}

#[test]
fn build_accepts_compiler_wrapper_path() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = "/opt/bin/icecc"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Path(Utf8PathBuf::from("/opt/bin/icecc")),
        })
    );
}

#[test]
fn empty_build_compiler_wrapper_value_is_rejected() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = ""
        "#,
    )
    .unwrap_err();
    match err {
        ManifestError::InvalidCompilerWrapper { source, .. } => assert!(matches!(
            source,
            cabin_core::CompilerWrapperParseError::Empty
        )),
        other => panic!("expected InvalidCompilerWrapper, got {other:?}"),
    }
}

#[test]
fn whitespace_build_compiler_wrapper_value_is_rejected() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build]
            compiler-wrapper = "   "
        "#,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ManifestError::InvalidCompilerWrapper {
            source: cabin_core::CompilerWrapperParseError::Empty,
            ..
        }
    ));
}

#[test]
fn build_compiler_wrapper_does_not_alter_profile_flags() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            defines = ["FOO=1"]

            [build]
            compiler-wrapper = "ccache"
        "#,
    );
    assert_eq!(package.build.general.defines, vec!["FOO=1".to_owned()]);
    assert_eq!(
        package.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Name("ccache".into()),
        })
    );
}

// ---------------------------------------------------------------
// [patch] table parsing
// ---------------------------------------------------------------

#[test]
fn patch_table_parses_local_path_entry() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { path = "../fmt" }
        "#,
    );
    let entries = &package.patches.entries;
    assert_eq!(entries.len(), 1);
    let key = cabin_core::PackageName::new("fmt").unwrap();
    match entries.get(&key) {
        Some(cabin_core::PatchSource::Path { path }) => {
            assert_eq!(path, &Utf8PathBuf::from("../fmt"));
        }
        other => panic!("expected Path patch, got {other:?}"),
    }
}

#[test]
fn patch_table_rejects_unknown_field_via_serde() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { branch = "main" }
        "#,
    )
    .unwrap_err();
    // `deny_unknown_fields` on the row catches this.
    assert!(matches!(err, ManifestError::Toml(_)));
}

#[test]
fn patch_table_rejects_empty_path() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { path = "" }
        "#,
    )
    .unwrap_err();
    match err {
        ManifestError::InvalidPatch { package, source } => {
            assert_eq!(package, "fmt");
            assert!(matches!(
                source,
                cabin_core::PatchValidationError::MissingSource { .. }
            ));
        }
        other => panic!("expected InvalidPatch, got {other:?}"),
    }
}

#[test]
fn patch_table_rejects_invalid_package_name() {
    // Two slashes exceed the scoped `<scope>/<name>` form.
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            "evil/na/me" = { path = "../fmt" }
        "#,
    )
    .unwrap_err();
    // Goes through the shared PackageName validator.
    let message = err.to_string();
    assert!(
        message.contains("evil/na/me"),
        "expected the offending name in the error, got: {message}"
    );
}

#[test]
fn patch_table_accepts_scoped_package_key() {
    let package = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            "fmtlib/fmt" = { path = "../fmt" }
        "#,
    )
    .unwrap()
    .package
    .unwrap();
    let key = cabin_core::PackageName::new("fmtlib/fmt").unwrap();
    assert!(package.patches.entries.contains_key(&key));
}

#[test]
fn unknown_package_field_is_rejected() {
    // Generic coverage that any unrecognized field on
    // `[package]` is rejected by serde's `deny_unknown_fields`.
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"
            not-a-real-key = "x"
        "#,
    )
    .unwrap_err();
    match err {
        ManifestError::Toml(source) => {
            let message = source.to_string();
            assert!(
                message.contains("unknown field"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected TOML parse error, got {other:?}"),
    }
}

#[test]
fn package_standard_field_accepts_workspace_marker() {
    let parsed = parse_manifest_str(
        r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = { workspace = true }
"#,
    )
    .unwrap();
    let pkg = parsed.package.unwrap();
    assert_eq!(
        pkg.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Workspace)
    );
    assert_eq!(pkg.language.c_standard, None);
}

#[test]
fn workspace_false_standard_marker_is_rejected() {
    let err = parse_manifest_str(
        r#"[package]
name = "demo"
version = "0.1.0"
c-standard = { workspace = false }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("workspace = false"),
        "unexpected: {err}"
    );
}

#[test]
fn standard_marker_with_extra_keys_is_rejected() {
    let err = parse_manifest_str(
        r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = { workspace = true, value = "c++20" }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("unknown field"),
        "unexpected: {err}"
    );
}

#[test]
fn standard_marker_on_target_field_is_rejected() {
    let err = parse_manifest_str(
        r#"[package]
name = "demo"
version = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = { workspace = true }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("[package]-level"),
        "unexpected: {err}"
    );
}

#[test]
fn workspace_false_standard_marker_on_target_reports_disabled_not_target() {
    let err = parse_manifest_str(
        r#"[package]
name = "demo"
version = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = { workspace = false }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("workspace = false"),
        "unexpected: {err}"
    );
}

#[test]
fn workspace_table_standard_fields_parse_into_typed_defaults() {
    let parsed = parse_manifest_str(
        r#"[workspace]
members = ["packages/*"]
c-standard = "c11"
cxx-standard = "c++20"
"#,
    )
    .unwrap();
    let ws = parsed.workspace.unwrap();
    assert_eq!(ws.standards.c_standard, Some(cabin_core::CStandard::C11));
    assert_eq!(
        ws.standards.cxx_standard,
        Some(cabin_core::CxxStandard::Cxx20)
    );
    assert_eq!(ws.standards.interface_c_standard, None);
    assert_eq!(ws.standards.interface_cxx_standard, None);
}

#[test]
fn workspace_table_invalid_standard_value_is_rejected() {
    let err = parse_manifest_str(
        r#"[workspace]
members = ["packages/*"]
cxx-standard = "c++2x"
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("unknown C++ standard"),
        "unexpected: {err}"
    );
}

#[test]
fn workspace_table_marker_valued_standard_field_is_rejected() {
    // The root is the definition site; the opt-in marker is not a
    // value there.  Surfaces via the generic TOML type error.
    let err = parse_manifest_str(
        r#"[workspace]
members = ["packages/*"]
cxx-standard = { workspace = true }
"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("expected a string"),
        "unexpected: {err}"
    );
}
