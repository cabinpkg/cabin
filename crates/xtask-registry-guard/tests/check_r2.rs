//! Regression cases for the R2 acquisition guard: the guard runs
//! against a scratch tree whose `src/` holds synthetic call sites, so
//! every way a bucket handle could be acquired outside the pinned
//! governor-admitting functions - a new function, a second acquisition
//! inside a pinned one, the UFCS and raw-identifier spellings, the name
//! split from its receiver by a comment - stays caught, and the shapes
//! that are not acquisitions at all stay accepted.

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
            "    let Some(object) = env.bucket(\"UPLOADS\")?.get(&key).execute().await? else {\n",
            "        return not_found();\n",
            "    };\n",
            "}\n",
            // Field access is not a call, a lookalike name is not the
            // method, and a comment describing one is not code.
            "fn bucket_from_columns(auth: &AuthContext) -> Option<quota::Bucket> {\n",
            "    if auth.bucket.is_some() { return None; }\n",
            "    // The call sites go through env.bucket(\"UPLOADS\") after a decide.\n",
            "    let doc = r#\"{\"call\":\"env.bucket(x)\"}\"#;\n",
            "    read_bucket(db, &auth.token_id)\n",
            "}\n",
        ),
    );
    assert!(accepted, "the guard rejected the canonical call sites");
}

#[test]
fn unsanctioned_acquisitions_are_caught() {
    // Each is a distinct way a bucket handle could be acquired outside
    // the pinned seam.
    let cases: &[(&str, &str)] = &[
        (
            "new_function",
            "async fn sneaky_reader(env: &Env) { let b = env.bucket(\"UPLOADS\")?; }",
        ),
        (
            // The pin is a count: a second acquisition inside a
            // sanctioned function is a new seam to review.
            "second_acquisition_in_a_pinned_fn",
            concat!(
                "async fn artifact_response(env: &Env) {\n",
                "    let a = env.bucket(\"UPLOADS\")?;\n",
                "    let b = env.bucket(\"OTHER\")?;\n",
                "}\n",
            ),
        ),
        (
            "ufcs",
            "fn f(env: &Env) { let b = worker::Env::bucket(env, \"UPLOADS\"); }",
        ),
        (
            "raw_identifier",
            "fn f(env: &Env) { let b = env.r#bucket(\"UPLOADS\"); }",
        ),
        (
            "comment_between_receiver_and_name",
            "fn f(env: &Env) { let b = env./* sneaky */bucket(\"UPLOADS\"); }",
        ),
        (
            // A line-oriented match would miss this.
            "comment_between_name_and_paren_across_lines",
            r#"fn f(env: &Env) {
    let b = env.
/* explanation */
bucket
("UPLOADS");
}"#,
        ),
        (
            // A `//` inside a string starts no comment: the call after
            // it on the same line must still be seen.
            "after_a_url_string",
            "fn f(env: &Env) { let u = \"https://api.cloudflare.com\"; let b = env.bucket(\"UPLOADS\"); }",
        ),
        (
            "outside_any_fn",
            "static B: () = { env.bucket(\"UPLOADS\") };",
        ),
        (
            // A path-form method item aliases the method; every later
            // call through the alias would evade the call scan.
            "method_item_alias",
            "fn f(env: &Env) { let acquire = worker::Env::bucket; acquire(env, \"UPLOADS\"); }",
        ),
        (
            // The generic binding accessor yields a Bucket without the
            // `bucket` token ever appearing.
            "generic_get_binding",
            "fn f(env: &Env) { let b: worker::Bucket = env.get_binding(\"UPLOADS\").unwrap(); }",
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
        "async fn artifact_response(env: &Env) { let b = env.bucket(\"UPLOADS\")?; }",
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
        "web_glue.rs",
        "async fn package_source(env: &Env) { serve_from_cache(env).await }",
    );
    let violations = r2::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/web_glue.rs: package_source is pinned for 1 acquisition(s) \
             but has 0; update crates/xtask-registry-guard/src/r2.rs"
        ]
    );
}

/// An acquisition outside every function is attributed to no function
/// rather than silently landing in the previous one.
#[test]
fn an_acquisition_outside_any_function_says_so() {
    let dir = scratch("verify.rs", "static B: () = { env.bucket(\"UPLOADS\") };");
    let violations = r2::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/verify.rs:1: unsanctioned R2 bucket acquisition in (no enclosing fn) - \
             prove the governor admission and pin it in crates/xtask-registry-guard/src/r2.rs"
        ]
    );
}

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
    let dir = scratch("verify.rs", "fn f(env: &Env) { env.bucket(\"UPLOADS\"); }");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-r2", "--registry-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains("unsanctioned R2 bucket acquisition in f"))
        .stderr(contains("outside the pinned"));
}
