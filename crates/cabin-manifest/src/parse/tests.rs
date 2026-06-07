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

const MINIMAL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"
    "#;

const FULL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"

        [target.hello]
        type = "executable"
        sources = ["src/main.cc"]
        include_dirs = ["include"]
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

            [target.hello]
            type = "executable"
            sources = ["src/my source.cc"]
            include_dirs = ["my include dir"]
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
            include-dirs = ["include"]
        "#;
    let err = parse_project_err(manifest);
    match err {
        ManifestError::Toml(source) => {
            let message = source.to_string();
            assert!(
                message.contains("unknown field `include-dirs`"),
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
            type = "header_only"
            include_dirs = ["include"]
        "#;
    let package = parse_project(manifest);
    let target = &package.targets[0];
    assert_eq!(target.kind, TargetKind::HeaderOnly);
    assert!(target.sources.is_empty());
}

#[test]
fn header_only_rejects_sources() {
    let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header_only"
            sources = ["src/empty.cc"]
            include_dirs = ["include"]
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
fn executable_accepts_mixed_c_and_cpp_sources() {
    // Target kinds describe artifact role only; the parser
    // accepts both C/C++ source extensions under any
    // executable / library / test / example target. Source-
    // language classification is per-file in the planner.
    let manifest = r#"
            [package]
            name = "exe"
            version = "0.1.0"

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
            type = "header_only"
            include_dirs = ["include"]
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
/// recognized. A manifest using either must fail with
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
    assert_eq!(package.targets[0].deps[0].as_str(), "external");
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
/// at manifest-parse time. The build planner joins
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

/// Cross-package dep references use the `package:target` form. The
/// `:` is outside the path-safe target-name grammar, so deps are
/// stored as raw strings (not `TargetName`) and validated only at
/// resolution time. Pin the round-trip so the type relaxation does
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
    assert_eq!(package.targets[0].deps, vec!["other-pkg:lib".to_owned()]);
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

            [profile.cache]
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
        parsed.root_settings.compiler_wrapper.general,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::CompilerWrapperKind::Ccache,
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
    // rejected by serde's `deny_unknown_fields`. Use a token
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
    // `system = true` dep is required. The unknown field is
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
    // Dev deps with `path = "..."` are declaration-only; we
    // accept them at parse time and surface them through
    // `package.dependencies` so `cabin metadata` lists them.
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
    // `default-features`, `git`, `registry`, `source`). The
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
fn unsupported_dependency_section_yields_toml_error() {
    // `[test-dependencies]` is not a recognized top-level
    // section. `RawManifest` declares `deny_unknown_fields`
    // so a typo cannot silently drop dependencies — the
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
    // `[profile.<name>]`. The field name is intentionally a
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

#[test]
fn rejects_port_combined_with_path() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", path = "../zlib" }
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
fn rejects_port_combined_with_version() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", version = "1.0" }
        "#,
    );
    match err {
        ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
            assert_eq!(conflicting, "version");
        }
        other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
    }
}

#[test]
fn rejects_port_combined_with_workspace() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", workspace = true }
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
fn rejects_port_with_features() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", features = ["x"] }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "features");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
}

#[test]
fn rejects_port_with_default_features() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", default-features = false }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "default-features");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
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
fn rejects_port_true_combined_with_features() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", features = ["x"] }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "features");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
}

#[test]
fn rejects_port_true_combined_with_default_features() {
    let err = parse_project_err(
        r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", default-features = false }
        "#,
    );
    match err {
        ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
            assert_eq!(conflicting, "default-features");
        }
        other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
    }
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
// [profile.cache] / [target.'cfg(...)'.profile.cache] parsing
// ---------------------------------------------------------------

#[test]
fn build_cache_parses_general_compiler_wrapper() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "ccache"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper.general,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::CompilerWrapperKind::Ccache,
        })
    );
    assert!(package.compiler_wrapper.conditional.is_empty());
}

#[test]
fn build_cache_accepts_none_to_explicitly_disable() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "none"
        "#,
    );
    assert_eq!(
        package.compiler_wrapper.general,
        Some(cabin_core::CompilerWrapperRequest::Disabled)
    );
}

#[test]
fn target_conditional_build_cache_collects_per_condition() {
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile.cache]
            compiler-wrapper = "sccache"
        "#,
    );
    assert!(package.compiler_wrapper.general.is_none());
    assert_eq!(package.compiler_wrapper.conditional.len(), 1);
    let entry = &package.compiler_wrapper.conditional[0];
    assert_eq!(
        entry.request,
        cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::CompilerWrapperKind::Sccache,
        }
    );
}

#[test]
fn unsupported_compiler_wrapper_value_is_rejected() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "fastcache"
        "#,
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("[profile.cache]") && message.contains("fastcache"),
        "expected error to point at the offending section + value, got: {message}"
    );
}

#[test]
fn empty_compiler_wrapper_value_is_rejected() {
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
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
fn build_cache_does_not_alter_build_flags_decl() {
    // The cache sub-table must not bleed into the per-package
    // `[profile]` flag layers — defines / include dirs etc. stay
    // exactly what the user declared, so existing manifests
    // without a `[profile.cache]` block continue to round-trip
    // byte-for-byte.
    let package = parse_project(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            defines = ["FOO=1"]

            [profile.cache]
            compiler-wrapper = "ccache"
        "#,
    );
    assert_eq!(package.build.general.defines, vec!["FOO=1".to_owned()]);
    assert_eq!(
        package.compiler_wrapper.general,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::CompilerWrapperKind::Ccache,
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
    let err = parse_manifest_str(
        r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            "evil/name" = { path = "../fmt" }
        "#,
    )
    .unwrap_err();
    // Goes through the shared PackageName validator.
    let message = err.to_string();
    assert!(
        message.contains("evil/name"),
        "expected the offending name in the error, got: {message}"
    );
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
