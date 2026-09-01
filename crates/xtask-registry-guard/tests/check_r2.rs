//! Regression cases for the R2 acquisition guard (see
//! `registry/docs/architecture.md`, "The cost governor"): the guard runs
//! against a scratch tree whose `src/` holds synthetic call sites, so
//! every way a bucket handle could be acquired outside the pinned
//! governor-admitting functions - a new function, a second acquisition
//! inside a pinned one, the UFCS and raw-identifier spellings, the name
//! split from its receiver by a comment - stays caught, and the shapes
//! that are not acquisitions at all stay accepted. An untested guard is
//! the one that rots.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

use xtask_registry_guard::{r2, registry_dir};

/// A scratch registry tree whose `src/<file>` holds `call_site`.
fn scratch(file: &str, call_site: &str) -> assert_fs::TempDir {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    let path = dir.path().join("src").join(file);
    fs::create_dir_all(path.parent().expect("src parent")).expect("create scratch src/");
    fs::write(path, call_site).expect("write the call site");
    dir
}

/// Runs the guard over a scratch tree containing `call_site` at
/// `src/<file>`; `true` means the guard accepted it.
fn guard_accepts_in(file: &str, call_site: &str) -> bool {
    let dir = scratch(file, call_site);
    r2::check(dir.path()).expect("run the guard").is_empty()
}

/// A pinned function holding exactly its sanctioned acquisitions - and
/// the neighboring shapes that are not acquisitions at all - must pass,
/// or the guard would block ordinary work.
#[test]
fn the_canonical_call_sites_pass() {
    let accepted = guard_accepts_in(
        "glue/read.rs",
        concat!(
            "async fn artifact_response(env: &Env) -> worker::Result<Response> {\n",
            "    let Some(object) = env.bucket(\"BLOBS\")?.get(&key).execute().await? else {\n",
            "        return not_found();\n",
            "    };\n",
            "}\n",
            // Field access is not a call, a lookalike name is not the
            // method, and a comment describing one is not code.
            "fn bucket_from_columns(auth: &AuthContext) -> Option<quota::Bucket> {\n",
            "    if auth.bucket.is_some() { return None; }\n",
            "    // The call sites go through env.bucket(\"BLOBS\") after a decide.\n",
            "    let doc = r#\"{\"call\":\"env.bucket(x)\"}\"#;\n",
            "    read_bucket(db, &auth.token_id)\n",
            "}\n",
        ),
    );
    assert!(accepted, "the guard rejected the canonical call sites");
    // The queue drain's double acquisition is pinned under
    // backup_glue.rs, where the drain lives.
    let accepted = guard_accepts_in(
        "backup_glue.rs",
        concat!(
            "async fn drain_backup_queue(env: &Env) {\n",
            "    let (Ok(db), Ok(blobs), Ok(backup)) =\n",
            "        (env.d1(\"DB\"), env.bucket(\"BLOBS\"), env.bucket(\"BACKUP\"));\n",
            "}\n",
        ),
    );
    assert!(accepted, "the guard rejected the backup_glue drain");
}

#[test]
fn unsanctioned_acquisitions_are_caught() {
    // Each is a distinct way a bucket handle could be acquired outside
    // the pinned seam.
    let cases: &[(&str, &str)] = &[
        (
            "new_function",
            "async fn sneaky_reader(env: &Env) { let b = env.bucket(\"BLOBS\")?; }",
        ),
        (
            // The pin is a count: a second acquisition inside a
            // sanctioned function is a new seam to review.
            "second_acquisition_in_a_pinned_fn",
            concat!(
                "async fn artifact_response(env: &Env) {\n",
                "    let a = env.bucket(\"BLOBS\")?;\n",
                "    let b = env.bucket(\"BACKUP\")?;\n",
                "}\n",
            ),
        ),
        (
            "ufcs",
            "fn f(env: &Env) { let b = worker::Env::bucket(env, \"BLOBS\"); }",
        ),
        (
            "raw_identifier",
            "fn f(env: &Env) { let b = env.r#bucket(\"BLOBS\"); }",
        ),
        (
            "comment_between_receiver_and_name",
            "fn f(env: &Env) { let b = env./* sneaky */bucket(\"BLOBS\"); }",
        ),
        (
            // a line-oriented match would miss this; the scan must not be.
            "comment_between_name_and_paren_across_lines",
            "fn f(env: &Env) {\n    let b = env.\n/* explanation */\nbucket\n(\"BLOBS\");\n}",
        ),
        (
            // A `//` inside a string starts no comment: the call after
            // it on the same line must still be seen.
            "after_a_url_string",
            "fn f(env: &Env) { let u = \"https://api.cloudflare.com\"; let b = env.bucket(\"BLOBS\"); }",
        ),
        (
            "outside_any_fn",
            "static B: () = { env.bucket(\"BLOBS\") };",
        ),
        (
            // A path-form method item aliases the method; every later
            // call through the alias would evade the call scan.
            "method_item_alias",
            "fn f(env: &Env) { let acquire = worker::Env::bucket; acquire(env, \"BLOBS\"); }",
        ),
        (
            // The generic binding accessor yields a Bucket without the
            // `bucket` token ever appearing.
            "generic_get_binding",
            "fn f(env: &Env) { let b: worker::Bucket = env.get_binding(\"BLOBS\").unwrap(); }",
        ),
        (
            // So does an unchecked JS cast over the raw env object.
            "unchecked_cast",
            "fn f(v: JsValue) { let b = v.unchecked_into::<worker::Bucket>(); }",
        ),
    ];
    let escaped: Vec<&str> = cases
        .iter()
        .filter(|(_, call_site)| guard_accepts_in("glue/read.rs", call_site))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted an unsanctioned R2 acquisition: {escaped:?}"
    );
}

/// The pins are file-scoped: a sanctioned glue/read.rs function does not
/// sanction the same name elsewhere, and a pinned function that no
/// longer acquires its bucket is drift the pin must follow.
#[test]
fn the_pins_are_file_scoped_and_track_drift() {
    assert!(!guard_accepts_in(
        "verify.rs",
        "async fn artifact_response(env: &Env) { let b = env.bucket(\"BLOBS\")?; }",
    ));
    assert!(!guard_accepts_in(
        "glue/read.rs",
        "async fn artifact_response(env: &Env) { serve_from_cache(env).await }",
    ));
}

/// The drift diagnostic names the function, both counts, and where the
/// pin lives - a reviewer has to be able to act on it.
#[test]
fn a_drifted_pin_names_its_counts() {
    let dir = scratch(
        "backup_glue.rs",
        "async fn drain_backup_queue(env: &Env) { let b = env.bucket(\"BLOBS\"); }",
    );
    let violations = r2::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/backup_glue.rs: drain_backup_queue is pinned for 2 acquisition(s) \
             but has 1; update crates/xtask-registry-guard/src/r2.rs"
        ]
    );
}

/// Several drifted pins in one file come out sorted by function name.
#[test]
fn drifted_pins_are_reported_in_name_order() {
    let dir = scratch(
        "glue/bearer/package.rs",
        // Chosen so the allowlist order and the alphabetical order
        // disagree: `persist_new_revision` is pinned before
        // `heal_blobs_on_retry` but sorts after it.
        "fn persist_new_revision() {}\nfn heal_blobs_on_retry() {}\n",
    );
    let violations = r2::check(dir.path()).expect("run the guard");
    let names: Vec<&str> = violations
        .iter()
        .map(|line| line.split_whitespace().nth(1).expect("the function name"))
        .collect();
    assert_eq!(names, vec!["heal_blobs_on_retry", "persist_new_revision"]);
}

/// An acquisition outside every function is attributed to no function
/// rather than silently landing in the previous one.
#[test]
fn an_acquisition_outside_any_function_says_so() {
    let dir = scratch("verify.rs", "static B: () = { env.bucket(\"BLOBS\") };");
    let violations = r2::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/verify.rs:1: unsanctioned R2 bucket acquisition in (no enclosing fn) - \
             prove the governor admission and pin it in crates/xtask-registry-guard/src/r2.rs"
        ]
    );
}

/// The committed Worker sources pass.
#[test]
fn the_committed_worker_sources_pass() {
    let violations = r2::check(&registry_dir()).expect("run the guard");
    assert!(
        violations.is_empty(),
        "the committed Worker sources: {violations:?}"
    );
}

/// The binary reports violations on stdout, names the remedy on stderr,
/// and exits non-zero - the contract CI depends on.
#[test]
fn the_binary_reports_and_exits_non_zero() {
    let dir = scratch("verify.rs", "fn f(env: &Env) { env.bucket(\"BLOBS\"); }");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-r2", "--registry-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains("unsanctioned R2 bucket acquisition in f"))
        .stderr(contains("outside the pinned"));
}
