//! The runbook and the smoke test are load-bearing operational
//! surfaces: the runbook documents every governor knob and the guarded
//! scripts, and the smoke test is the executable spec for the governor
//! scenarios. Nothing else would notice a renamed limit var that
//! orphans the runbook's table, a runbook step naming a script that no
//! longer exists, or a smoke leg quietly deleted - so these tests pin
//! the cross-references. Lexical by design: they prove the documents
//! still talk about the behavior, not that the behavior works (the
//! smoke run itself proves that).

use std::fs;
use std::path::PathBuf;

use cabin_registry_worker::governor::{OpPool, StoragePool, op_env_var, storage_env_var};

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

// Compile-time exhaustiveness: a new pool variant must extend these
// lists (and then the runbook table, or the tests below fail).
const STORAGE_POOLS: [StoragePool; 3] =
    [StoragePool::Primary, StoragePool::Backup, StoragePool::Dump];
const OP_POOLS: [OpPool; 7] = [
    OpPool::APublish,
    OpPool::AInfra,
    OpPool::BOrdinary,
    OpPool::BSource,
    OpPool::BVerifier,
    OpPool::BPublish,
    OpPool::BInfra,
];
const _: fn(StoragePool) = |pool| match pool {
    StoragePool::Primary | StoragePool::Backup | StoragePool::Dump => (),
};
const _: fn(OpPool) = |pool| match pool {
    OpPool::APublish
    | OpPool::AInfra
    | OpPool::BOrdinary
    | OpPool::BSource
    | OpPool::BVerifier
    | OpPool::BPublish
    | OpPool::BInfra => (),
};

/// Every hard-limit env var the code reads appears in the runbook's
/// governor section (the operator's only complete table of them).
#[test]
fn every_governor_limit_var_is_in_the_runbook() {
    let runbook = read("docs/runbook.md");
    let missing: Vec<&str> = STORAGE_POOLS
        .iter()
        .map(|&pool| storage_env_var(pool))
        .chain(OP_POOLS.iter().map(|&pool| op_env_var(pool)))
        .filter(|var| !runbook.contains(var))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/runbook.md lost governor limit var(s): {missing:?}"
    );
}

/// Every pool name the ledger reports (snapshot rows, refusal bodies,
/// reconcile reports) is explained somewhere in the runbook.
#[test]
fn every_pool_name_is_in_the_runbook() {
    let runbook = read("docs/runbook.md");
    let missing: Vec<&str> = STORAGE_POOLS
        .iter()
        .map(|&pool| pool.as_str())
        .chain(OP_POOLS.iter().map(|&pool| pool.as_str()))
        .filter(|pool| !runbook.contains(pool))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/runbook.md never names pool(s): {missing:?}"
    );
}

/// Every `scripts/<name>` the runbook tells an operator to run exists
/// and is tracked - a runbook step naming a deleted script is a
/// mid-incident dead end.
#[test]
fn every_script_the_runbook_references_exists() {
    let runbook = read("docs/runbook.md");
    let mut missing = Vec::new();
    for (index, _) in runbook.match_indices("scripts/") {
        let tail = &runbook[index..];
        let name: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
            .collect();
        let name = name.trim_end_matches('.');
        if !PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(name)
            .exists()
        {
            missing.push(name.to_owned());
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "docs/runbook.md references missing script(s): {missing:?}"
    );
}

/// The guarded operator scripts this branch introduced stay documented:
/// each exists on disk and is named by the runbook, so neither side can
/// drop the other silently.
#[test]
fn the_operator_scripts_stay_documented() {
    let runbook = read("docs/runbook.md");
    let undocumented: Vec<&str> = [
        "scripts/governor.sh",
        "scripts/backup-audit.sh",
        "scripts/migrate.sh",
        "scripts/diagnose.sh",
        "scripts/check-deploy.sh",
    ]
    .into_iter()
    .filter(|script| {
        assert!(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(script)
                .exists(),
            "{script} is gone; remove it from this pin and the runbook together"
        );
        !runbook.contains(script)
    })
    .collect();
    assert!(
        undocumented.is_empty(),
        "docs/runbook.md never mentions: {undocumented:?}"
    );
}

/// The smoke scenarios that prove the governor's operational behavior
/// stay present, by their step labels. Deleting or renaming a leg must
/// update this pin consciously.
#[test]
fn the_smoke_test_keeps_its_governor_legs() {
    let smoke = read("scripts/smoke.sh");
    let missing: Vec<&str> = [
        "restarting wrangler dev with tiny governor pools",
        "cached verified downloads keep serving under an exhausted read pool",
        "an uncached verified download is refused with the budget envelope",
        "the verifier pool is isolated from the exhausted ordinary pool",
        "a fresh publish is refused before any r2 write when storage is exhausted",
        "source-viewer reads fail closed on an exhausted source pool",
        "the admin governor endpoint reports usage and takes operator actions",
        "an admin reconcile rebuilds the wiped primary ledger on demand",
        "reconciliation rebuilds the wiped primary ledger from d1",
        "concurrent downloads with retries never take the pool past its limit",
    ]
    .into_iter()
    .filter(|label| !smoke.contains(label))
    .collect();
    assert!(
        missing.is_empty(),
        "scripts/smoke.sh lost governor leg(s): {missing:?}"
    );
}
