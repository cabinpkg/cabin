//! Regression cases for the deploy-configuration guard: it runs against
//! a scratch tree seeded with the real `wrangler.jsonc`, so every
//! deploy-breaking (or silently-spend-widening) config mutation it
//! exists to catch - a lost binding, a deleted or edited Durable Object
//! migration, a misspelled or unparsable hard-limit var, a missing cron,
//! a bundle that stopped exporting the class - stays caught, and the
//! shipped config itself stays accepted. An untested guard is the one
//! that rots.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::path::PathBuf;

use xtask_registry_guard::{deploy, registry_dir};

/// A minified bundle shape like worker-build's, exporting `Governor`.
const BUNDLE_WITH_GOVERNOR: &str =
    "var x=1;export{Xa as ContainerStartupOptions,Tb as Governor,Wc as IntoUnderlyingByteSource};";

fn real_config() -> String {
    fs::read_to_string(registry_dir().join("wrangler.jsonc")).expect("read wrangler.jsonc")
}

/// Runs the guard over a scratch tree holding `config` (and a bundle
/// unless `bundle` is None); `true` means the guard accepted.
fn guard_accepts(config: &str, bundle: Option<&str>, require_bundle: bool) -> bool {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::write(dir.path().join("wrangler.jsonc"), config).expect("write the config");
    if let Some(bundle) = bundle {
        fs::create_dir_all(dir.path().join("build")).expect("create scratch build/");
        fs::write(dir.path().join("build/index.js"), bundle).expect("write the bundle");
    }
    deploy::check(dir.path(), require_bundle).accepted()
}

/// The config that deploys is the config the guard accepts - with the
/// real bundle shape, and with the bundle absent (the local, pre-build
/// invocation).
#[test]
fn the_shipped_config_passes() {
    let config = real_config();
    assert!(guard_accepts(&config, Some(BUNDLE_WITH_GOVERNOR), true));
    assert!(guard_accepts(&config, None, false));
    // The D1_DATABASE_ID mirror check follows the DB binding, not
    // array position: a second binding ahead of it must not confuse it.
    let decoy = config.replace(
        r#""d1_databases": ["#,
        r#""d1_databases": [
        { "binding": "AUDIT", "database_name": "decoy",
          "database_id": "00000000-0000-0000-0000-000000000000" },"#,
    );
    assert_ne!(decoy, config, "the decoy mutation matched nothing");
    assert!(guard_accepts(&decoy, Some(BUNDLE_WITH_GOVERNOR), true));
    // The governor trims before parsing, so a padded GOVERNOR_* value
    // is valid at runtime and must stay accepted (unlike BUDGET_*).
    let padded = config.replace(
        r#""GITHUB_CLIENT_ID""#,
        r#""GOVERNOR_STORAGE_PRIMARY_BYTES": " 4294967296 ", "GITHUB_CLIENT_ID""#,
    );
    assert_ne!(padded, config, "the padded mutation matched nothing");
    assert!(guard_accepts(&padded, Some(BUNDLE_WITH_GOVERNOR), true));
}

// Each mutation is a distinct deploy-time (or silent-overspend)
// failure the guard must move into CI. Every `from` must exist in
// the real config, or the mutation would silently test nothing.
const BREAKAGES: &[(&str, &str, &str)] = &[
    (
        "renamed_do_binding",
        r#""name": "GOVERNOR""#,
        r#""name": "GOV""#,
    ),
    (
        "renamed_do_class",
        r#""class_name": "Governor""#,
        r#""class_name": "Gov""#,
    ),
    (
        "lost_d1_binding",
        r#""binding": "DB""#,
        r#""binding": "DATABASE""#,
    ),
    (
        "lost_blobs_bucket",
        r#""binding": "BLOBS""#,
        r#""binding": "BLOBSTORE""#,
    ),
    (
        "edited_v1_migration",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v2", "new_sqlite_classes": ["Governor"] }]"#,
    ),
    (
        "deleted_do_class",
        r#""new_sqlite_classes": ["Governor"]"#,
        r#""new_sqlite_classes": ["Governor"], "deleted_classes": ["Governor"]"#,
    ),
    (
        "mixed_exports_lifecycle",
        r#""durable_objects": {"#,
        r#""exports": [], "durable_objects": {"#,
    ),
    (
        "misspelled_limit_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""GOVERNOR_STORAGE_PRIMARY_BYTE": "1", "GITHUB_CLIENT_ID""#,
    ),
    (
        "unparsable_limit_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""GOVERNOR_STORAGE_PRIMARY_BYTES": "4 GiB", "GITHUB_CLIENT_ID""#,
    ),
    (
        "over_u64_limit_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""BUDGET_R2_STORAGE_BYTES": "99999999999999999999999", "GITHUB_CLIENT_ID""#,
    ),
    (
        // The breaker parses the raw string (no trim), so padding
        // silently reverts the override to the default at runtime.
        "whitespace_padded_budget_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""BUDGET_R2_STORAGE_BYTES": " 800000 ", "GITHUB_CLIENT_ID""#,
    ),
    (
        // A number is not a string, so the runtime parser never sees the
        // value it was meant to override.
        "numeric_limit_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""BUDGET_R2_STORAGE_BYTES": 800000, "GITHUB_CLIENT_ID""#,
    ),
    (
        "over_u64_by_one_limit_var",
        r#""GITHUB_CLIENT_ID""#,
        r#""BUDGET_R2_STORAGE_BYTES": "18446744073709551616", "GITHUB_CLIENT_ID""#,
    ),
    ("lost_breaker_cron", r#""*/15 * * * *", "#, ""),
    (
        "lost_dump_cron",
        r#""crons": ["*/15 * * * *", "0 3 * * *"]"#,
        r#""crons": ["*/15 * * * *"]"#,
    ),
    // (the stale-D1_DATABASE_ID case is built dynamically in the test:
    // its mutation target is the live database id, which every wipe
    // changes, and a hardcoded id would break the fixture on each one)
    (
        "drifted_migrations_dir",
        r#""migrations_dir": "migrations""#,
        r#""migrations_dir": "schema""#,
    ),
    (
        // A rename away plus a delete of the renamed class would
        // destroy the same storage the bound name once used;
        // deleted_classes is banned wherever it appears.
        "rename_then_delete",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [
                { "tag": "v1", "new_sqlite_classes": ["Governor"] },
                { "tag": "v2", "renamed_classes": [{ "from": "Governor", "to": "Retired" }] },
                { "tag": "v3", "deleted_classes": ["Retired"], "new_sqlite_classes": ["Governor"] }]"#,
    ),
    ("replaced_dump_cron", r#""0 3 * * *""#, r#""0 3 1 * *""#),
    (
        // A tag the platform cannot tell apart from another one.
        "duplicate_migration_tags",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] },
                { "tag": "v1", "new_classes": ["Other"] }]"#,
    ),
    ("unparsable_config", r#""vars": {"#, r#""vars": {{"#),
    (
        // `?? []` only substitutes for null/undefined, so the node guard
        // this replaces measured `.length` on whatever was there. A
        // non-array value refuses outright now.
        "stringy_deleted_classes",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] },
                { "tag": "v2", "deleted_classes": "Governor" }]"#,
    ),
    (
        "object_renamed_classes",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] },
                { "tag": "v2", "renamed_classes": { "from": "Governor", "to": "Retired" } }]"#,
    ),
    (
        // A migration with no tag at all is the empty tag.
        "untagged_migration",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] },
                { "new_classes": ["Other"] }]"#,
    ),
    (
        "non_string_tag",
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": 1, "new_sqlite_classes": ["Governor"] }]"#,
    ),
    (
        "lost_backup_bucket",
        r#""binding": "BACKUP""#,
        r#""binding": "BACKUPS""#,
    ),
    (
        "renamed_database",
        r#""database_name": "cabin-registry""#,
        r#""database_name": "cabin-registry-2""#,
    ),
    (
        // A second bound class that no migration ever introduces, with
        // migrations[0] left verbatim so only that check can fire.
        "uninvited_bound_class",
        r#""bindings": ["#,
        r#""bindings": [{ "name": "OTHER", "class_name": "Other" },"#,
    ),
];

/// The `D1_DATABASE_ID` mirror breakage, built against whatever id the
/// config currently carries (a wipe changes it, so no fixture may
/// hardcode it).
#[test]
fn a_stale_dump_database_id_is_caught() {
    let config = real_config();
    let marker = r#""D1_DATABASE_ID": ""#;
    let start = config
        .find(marker)
        .expect("the config carries D1_DATABASE_ID")
        + marker.len();
    let id = &config[start..start + 36];
    let stale = config.replacen(
        &format!("{marker}{id}"),
        r#""D1_DATABASE_ID": "00000000-0000-0000-0000-000000000000"#,
        1,
    );
    assert_ne!(stale, config, "the stale-id mutation matched nothing");
    assert!(!guard_accepts(&stale, Some(BUNDLE_WITH_GOVERNOR), true));
}

#[test]
fn config_breakage_is_caught() {
    let config = real_config();
    let escaped: Vec<&str> = BREAKAGES
        .iter()
        .map(|(name, from, to)| {
            assert!(
                config.contains(from),
                "{name}: mutation target not in the real config"
            );
            (name, config.replace(from, to))
        })
        .filter(|(_, mutated)| guard_accepts(mutated, Some(BUNDLE_WITH_GOVERNOR), true))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted a broken deploy config: {escaped:?}"
    );
}

/// A class introduced under an old name and renamed into the bound one
/// is still introduced - the walk resolves the chain backwards.
#[test]
fn a_renamed_class_is_still_introduced() {
    let config = real_config().replace(
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Governor"] }]"#,
        r#""migrations": [{ "tag": "v1", "new_sqlite_classes": ["Old"] },
            { "tag": "v2", "renamed_classes": [{ "from": "Old", "to": "Governor" }] }]"#,
    );
    // migrations[0] is pinned verbatim, so this shape is rejected - but
    // for that reason only, not for a missing introduction.
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::write(dir.path().join("wrangler.jsonc"), &config).expect("write the config");
    let report = deploy::check(dir.path(), false);
    assert_eq!(
        report.failures,
        vec!["migrations[0] must stay the deployed v1 Governor migration verbatim"]
    );
}

/// A block comment in the config is stripped like a line comment.
#[test]
fn a_block_comment_is_stripped() {
    let config = real_config().replace(r#""vars": {"#, r#"/* the runtime vars */ "vars": {"#);
    assert_ne!(
        config,
        real_config(),
        "the comment mutation matched nothing"
    );
    assert!(guard_accepts(&config, Some(BUNDLE_WITH_GOVERNOR), true));
}

/// An empty `script_name` is not a foreign class: the node guard read it
/// as falsy, and skipping the checks for it would be the fail-open
/// direction.
#[test]
fn an_empty_script_name_is_still_a_local_class() {
    let config = real_config().replace(
        r#""class_name": "Governor""#,
        r#""class_name": "Governor", "script_name": """#,
    );
    assert_ne!(
        config,
        real_config(),
        "the script_name mutation matched nothing"
    );
    assert!(!guard_accepts(
        &config,
        Some("var x=1;export{Xa as ContainerStartupOptions};"),
        true
    ));
}

/// A comment inside the export list is not an export: taking the last
/// whitespace-delimited token instead of the last ` as ` piece would
/// read the commented name as exported.
#[test]
fn a_commented_export_list_entry_is_not_an_export() {
    let config = real_config();
    assert!(!guard_accepts(
        &config,
        Some("const Foo=1;export{ Foo // Governor\n};"),
        true
    ));
    // ...while the real rename form still counts.
    assert!(guard_accepts(
        &config,
        Some("export{ Tb as Governor };"),
        true
    ));
}

/// An unreadable config is reported like an unparsable one - same
/// streams, same summary - so CI still sees the line it reads.
#[test]
fn a_missing_config_keeps_the_stream_contract() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    let report = deploy::check(dir.path(), false);
    assert_eq!(
        report.notes,
        vec!["==> validating wrangler.jsonc against the code's deploy assumptions"]
    );
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0].starts_with("wrangler.jsonc does not parse as JSONC:"),
        "{:?}",
        report.failures
    );
    assert_eq!(
        report.summary,
        Some("the deploy configuration no longer matches the code's assumptions")
    );
}

/// `--require-bundle` belongs to this guard alone.
#[test]
fn require_bundle_is_rejected_for_the_other_guards() {
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-sql", "--require-bundle"])
        .assert()
        .failure()
        .stderr(contains("only meaningful for check-deploy"));
}

/// The failure the wasm build cannot see: the class compiles but the
/// bundle stops exporting it, which today only `wrangler deploy`
/// against production would report.
#[test]
fn a_bundle_without_the_class_export_is_caught() {
    let config = real_config();
    assert!(!guard_accepts(
        &config,
        Some("var x=1;export{Xa as ContainerStartupOptions};"),
        true
    ));
    // CI must fail when the bundle it just built is missing entirely.
    assert!(!guard_accepts(&config, None, true));
    // The `export class` declaration form counts as an export too.
    assert!(guard_accepts(
        &config,
        Some("export class Governor {}"),
        true
    ));
}

/// The missing-bundle refusal is its own diagnostic, not a config
/// failure: it tells CI the build step did not run.
#[test]
fn a_missing_required_bundle_names_the_build_step() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::write(dir.path().join("wrangler.jsonc"), real_config()).expect("write the config");
    let report = deploy::check(dir.path(), true);
    assert!(report.failures.is_empty());
    // No progress line either: the guard never got as far as validating.
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert_eq!(
        report.summary,
        Some("build/index.js is missing; run worker-build before this guard (--require-bundle)")
    );
}

/// The binary prints its progress on stdout, its detail on stderr, and
/// exits non-zero - the contract CI depends on.
#[test]
fn the_binary_reports_and_exits_non_zero() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::write(
        dir.path().join("wrangler.jsonc"),
        real_config().replace(r#""binding": "DB""#, r#""binding": "DATABASE""#),
    )
    .expect("write the config");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-deploy", "--registry-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains("==> validating wrangler.jsonc"))
        .stderr(contains("d1_databases must bind DB"));

    let clean = assert_fs::TempDir::new().expect("scratch tree");
    fs::write(clean.path().join("wrangler.jsonc"), real_config()).expect("write the config");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-deploy", "--registry-dir"])
        .arg(clean.path())
        .assert()
        .success()
        .stdout(contains("deploy config OK"));
}

/// The guard the workflow runs is the one under test, after the build.
#[test]
fn the_workflow_runs_this_guard() {
    let workflow =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/registry.yml");
    let text = fs::read_to_string(&workflow).expect("read the registry workflow");
    assert!(
        text.contains("cargo check-deploy --require-bundle"),
        "the registry workflow no longer runs cargo check-deploy --require-bundle"
    );
}
