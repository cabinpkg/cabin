//! Regression cases for the deploy-configuration guard
//! (`scripts/check-deploy.sh`): the real script runs against a scratch
//! tree seeded with the real wrangler.jsonc, so every deploy-breaking
//! (or silently-spend-widening) config mutation it exists to catch - a
//! lost binding, a deleted or edited Durable Object migration, a
//! misspelled or unparsable hard-limit var, a missing cron, a bundle
//! that stopped exporting the class - stays caught, and the shipped
//! config itself stays accepted. An untested guard is the one that
//! rots. Unix-only: the guard is a bash script.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A minified bundle shape like worker-build's, exporting `Governor`.
const BUNDLE_WITH_GOVERNOR: &str =
    "var x=1;export{Xa as ContainerStartupOptions,Tb as Governor,Wc as IntoUnderlyingByteSource};";

fn real_config() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("wrangler.jsonc");
    fs::read_to_string(path).expect("read wrangler.jsonc")
}

/// Runs the real guard over a scratch tree holding `config` (and a
/// bundle unless `bundle` is None); `true` means the guard accepted.
fn guard_accepts(name: &str, config: &str, bundle: Option<&str>, require_bundle: bool) -> bool {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scripts")).expect("create scratch scripts/");
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    fs::copy(
        scripts.join("check-deploy.sh"),
        dir.join("scripts/check-deploy.sh"),
    )
    .expect("copy the guard");
    fs::write(dir.join("wrangler.jsonc"), config).expect("write the config");
    if let Some(bundle) = bundle {
        fs::create_dir_all(dir.join("build")).expect("create scratch build/");
        fs::write(dir.join("build/index.js"), bundle).expect("write the bundle");
    }
    let mut command = Command::new("bash");
    command.arg("scripts/check-deploy.sh");
    if require_bundle {
        command.arg("--require-bundle");
    }
    let output = command.current_dir(&dir).output().expect("run the guard");
    output.status.success()
}

/// The config that deploys is the config the guard accepts - with the
/// real bundle shape, and with the bundle absent (the local, pre-build
/// invocation).
#[test]
fn the_shipped_config_passes() {
    let config = real_config();
    assert!(guard_accepts(
        "deploy_canonical",
        &config,
        Some(BUNDLE_WITH_GOVERNOR),
        true,
    ));
    assert!(guard_accepts("deploy_no_bundle", &config, None, false));
    // The D1_DATABASE_ID mirror check follows the DB binding, not
    // array position: a second binding ahead of it must not confuse it.
    let decoy = config.replace(
        r#""d1_databases": ["#,
        r#""d1_databases": [
        { "binding": "AUDIT", "database_name": "decoy",
          "database_id": "00000000-0000-0000-0000-000000000000" },"#,
    );
    assert_ne!(decoy, config, "the decoy mutation matched nothing");
    assert!(guard_accepts(
        "deploy_decoy_d1_binding",
        &decoy,
        Some(BUNDLE_WITH_GOVERNOR),
        true,
    ));
    // The governor trims before parsing, so a padded GOVERNOR_* value
    // is valid at runtime and must stay accepted (unlike BUDGET_*).
    let padded = config.replace(
        r#""GITHUB_CLIENT_ID""#,
        r#""GOVERNOR_STORAGE_PRIMARY_BYTES": " 4294967296 ", "GITHUB_CLIENT_ID""#,
    );
    assert_ne!(padded, config, "the padded mutation matched nothing");
    assert!(guard_accepts(
        "deploy_padded_governor_var",
        &padded,
        Some(BUNDLE_WITH_GOVERNOR),
        true,
    ));
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
    ("lost_breaker_cron", r#""*/15 * * * *", "#, ""),
    (
        "lost_dump_cron",
        r#""crons": ["*/15 * * * *", "0 3 * * *"]"#,
        r#""crons": ["*/15 * * * *"]"#,
    ),
    (
        "stale_dump_database_id",
        r#""D1_DATABASE_ID": "481e7566"#,
        r#""D1_DATABASE_ID": "00000000"#,
    ),
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
];

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
        .filter(|(name, mutated)| {
            guard_accepts(
                &format!("deploy_{name}"),
                mutated,
                Some(BUNDLE_WITH_GOVERNOR),
                true,
            )
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted a broken deploy config: {escaped:?}"
    );
}

/// The failure the wasm build cannot see: the class compiles but the
/// bundle stops exporting it, which today only `wrangler deploy`
/// against production would report.
#[test]
fn a_bundle_without_the_class_export_is_caught() {
    let config = real_config();
    assert!(!guard_accepts(
        "deploy_no_export",
        &config,
        Some("var x=1;export{Xa as ContainerStartupOptions};"),
        true,
    ));
    // CI must fail when the bundle it just built is missing entirely.
    assert!(!guard_accepts(
        "deploy_bundle_required",
        &config,
        None,
        true
    ));
}

/// The guard the workflow runs is the one under test, after the build.
#[test]
fn the_workflow_runs_this_guard() {
    let workflow = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.github/workflows/registry.yml")
        .canonicalize()
        .expect("locate the registry workflow");
    let text = fs::read_to_string(workflow).expect("read the registry workflow");
    assert!(
        text.contains("bash scripts/check-deploy.sh --require-bundle"),
        "the registry workflow no longer runs scripts/check-deploy.sh --require-bundle"
    );
}
