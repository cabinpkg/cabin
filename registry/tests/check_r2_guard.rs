//! Regression cases for the R2 acquisition guard
//! (`scripts/check-r2.sh`, see `docs/architecture.md`, "The cost
//! governor"): the real script runs against a scratch tree whose
//! `src/` holds synthetic call sites, so every way a bucket handle
//! could be acquired outside the pinned governor-admitting functions -
//! a new function, a second acquisition inside a pinned one, the UFCS
//! and raw-identifier spellings, the name split from its receiver by a
//! comment - stays caught, and the shapes that are not acquisitions at
//! all stay accepted. An untested guard is the one that rots.
//! Unix-only: the guard is a bash script.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs the real guard over a scratch tree containing `call_site` at
/// `src/<file>`; `true` means the guard accepted it.
fn guard_accepts_in(name: &str, file: &str, call_site: &str) -> bool {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create scratch src/");
    fs::create_dir_all(dir.join("scripts")).expect("create scratch scripts/");
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    for script in ["check-r2.sh", "check-r2.pl", "lexical.pm"] {
        fs::copy(scripts.join(script), dir.join("scripts").join(script)).expect("copy the guard");
    }
    fs::write(dir.join("src").join(file), call_site).expect("write the call site");

    let status = Command::new("bash")
        .arg("scripts/check-r2.sh")
        .current_dir(&dir)
        .output()
        .expect("run the guard");
    status.status.success()
}

/// A pinned function holding exactly its sanctioned acquisitions - and
/// the neighboring shapes that are not acquisitions at all - must
/// pass, or the guard would block ordinary work.
#[test]
fn the_canonical_call_sites_pass() {
    let accepted = guard_accepts_in(
        "r2_canonical",
        "glue.rs",
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
        "r2_canonical_backup_glue",
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
            // grep is line-oriented; the scan must not be.
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
        .filter(|(name, call_site)| guard_accepts_in(&format!("r2_{name}"), "glue.rs", call_site))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted an unsanctioned R2 acquisition: {escaped:?}"
    );
}

/// The pins are file-scoped: a sanctioned glue.rs function does not
/// sanction the same name elsewhere, and a pinned function that no
/// longer acquires its bucket is drift the pin must follow.
#[test]
fn the_pins_are_file_scoped_and_track_drift() {
    assert!(!guard_accepts_in(
        "r2_sanctioned_name_elsewhere",
        "verify.rs",
        "async fn artifact_response(env: &Env) { let b = env.bucket(\"BLOBS\")?; }",
    ));
    assert!(!guard_accepts_in(
        "r2_pinned_fn_lost_its_acquisition",
        "glue.rs",
        "async fn artifact_response(env: &Env) { serve_from_cache(env).await }",
    ));
}

/// The guard the workflow runs is the one under test.
#[test]
fn the_workflow_runs_this_guard() {
    let workflow = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.github/workflows/registry.yml")
        .canonicalize()
        .expect("locate the registry workflow");
    let text = fs::read_to_string(workflow).expect("read the registry workflow");
    assert!(
        text.contains("bash scripts/check-r2.sh"),
        "the registry workflow no longer runs scripts/check-r2.sh"
    );
}
