//! Regression cases for the SQL consolidation guard (see
//! `registry/docs/architecture.md`, "Why no ORM"): the guard runs
//! against a scratch tree whose `src/` holds one synthetic call site, so
//! every way executed SQL could grow outside `src/sql.rs` - a literal, a
//! `format!`, a dynamic argument, the multi-line spelling, the
//! raw-identifier and UFCS spellings, and D1's unprepared `exec` - stays
//! caught. An untested guard is the one that rots.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::path::PathBuf;

use xtask_registry_guard::{registry_dir, sql};

/// A scratch registry tree whose `src/<file>` holds `call_site`.
fn scratch(file: &str, call_site: &str) -> assert_fs::TempDir {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::create_dir_all(dir.path().join("src")).expect("create scratch src/");
    fs::write(dir.path().join("src").join(file), call_site).expect("write the call site");
    dir
}

/// Runs the guard over a scratch tree containing `call_site` at
/// `src/<file>`; `true` means the guard accepted it.
fn guard_accepts_in(file: &str, call_site: &str) -> bool {
    let dir = scratch(file, call_site);
    sql::check(dir.path()).expect("run the guard").is_empty()
}

fn guard_accepts(call_site: &str) -> bool {
    guard_accepts_in("glue.rs", call_site)
}

/// The canonical spelling - and the shapes around it that are not
/// prepare calls at all - must pass, or the guard would block ordinary
/// work.
#[test]
fn the_canonical_call_site_passes() {
    let accepted = guard_accepts(concat!(
        "db.prepare(sql::META_VALUE).bind(&[key.into()])?;\n",
        "db.prepare(sql::UPSERT_META)\n",
        "    .bind(&[key.into(), value.into()])?;\n",
        // R2's builder ends in execute(), not exec().
        "bucket.put(&key, bytes).execute().await?;\n",
        // The dump scanner's expectations are not executed SQL.
        "let expected = format!(\"CREATE TABLE {table}\");\n",
        // Neighboring identifiers the guard must not mistake for a
        // D1 call, and a comment describing one.
        "let stmt = parser.prepare_statement(input);\n",
        "if state.prepared { runner.execute_all(); }\n",
        "// The call sites go through db.prepare(sql::CONST), never a literal.\n",
        // Commented-out code, including the nested block comment
        // Rust permits: still a comment, not a call.
        "/* was: /* older */ db.prepare(dynamic_sql) */\n",
        // A raw string body is not code either.
        "let ok = r#\"{\"call\":\"db.prepare(x)\"}\"#;\n",
        // Field access is not a call, and a lifetime is not a
        // character literal.
        "if config.prepare && config.exec { return; }\n",
        "fn take<'a>(sql: &'a str) -> &'a str { sql }\n",
        // A wrapped call whose argument carries a comment is still
        // the canonical call.
        "db.prepare(\n    // The generation stamp.\n    sql::REGISTRY_GENERATION,\n)\n",
        "    .first(None)\n    .await?;\n",
    ));
    assert!(accepted, "the guard rejected the canonical call site");
}

#[test]
fn executed_sql_outside_sql_rs_is_caught() {
    // Each is a distinct way the executed-SQL invariant could be broken.
    let cases: &[(&str, &str)] = &[
        (
            "literal",
            "db.prepare(\"SELECT 1 FROM meta\").run().await?;",
        ),
        (
            "format",
            "db.prepare(&format!(\"SELECT {column} FROM meta\")).run().await?;",
        ),
        ("dynamic", "db.prepare(dynamic_sql).run().await?;"),
        (
            // The line also carries a const call: a line-level filter
            // would drop it.
            "dynamic_beside_a_const",
            "db.prepare(order_by(col)) // unlike db.prepare(sql::META_VALUE)",
        ),
        (
            "multi_line_argument",
            "db.prepare(\n    dynamic_sql,\n)\n.run()\n.await?;",
        ),
        ("raw_identifier", "db.r#prepare(dynamic_sql).run().await?;"),
        (
            "ufcs",
            "D1Database::prepare(&db, dynamic_sql).run().await?;",
        ),
        (
            "comment_between_name_and_paren",
            "db.prepare /* sneaky */ (dynamic_sql).run().await?;",
        ),
        (
            "comment_between_receiver_and_name",
            "db./* sneaky */prepare(dynamic_sql).run().await?;",
        ),
        (
            // a line-oriented match would miss this; the scan must not be.
            "comment_between_receiver_and_name_across_lines",
            "db.\n/* explanation */\nprepare(dynamic_sql)\n.run()\n.await?;",
        ),
        ("exec", "db.exec(\"DROP TABLE users\").await?;"),
        ("exec_dynamic", "db.exec(&dynamic_sql).await?;"),
        (
            "exec_behind_a_comment",
            "db./* sneaky */exec(dynamic_sql).await?;",
        ),
        (
            // A `//` inside a string starts no comment: the call after
            // it on the same line must still be seen.
            "after_a_url_string",
            "let base = \"https://api.cloudflare.com\"; db.prepare(dynamic_sql).run().await?;",
        ),
        (
            // An accepted call must not consume the violation behind it.
            "behind_a_canonical_call",
            "db.prepare(sql::META_VALUE);\ndb.prepare(dynamic_sql);",
        ),
        (
            // A quote inside a character literal opens no string.
            "after_a_quote_char_literal",
            "let quote = '\"'; db.prepare(dynamic_sql).run().await?;",
        ),
        (
            "after_a_byte_quote_char_literal",
            "let quote = b'\"'; db.prepare(dynamic_sql).run().await?;",
        ),
        (
            // A path-form method item aliases the method; every later
            // call through the alias would evade the call scan.
            "method_item_alias",
            "let p = D1Database::prepare; p(&db, dynamic_sql).run().await?;",
        ),
    ];
    let escaped: Vec<&str> = cases
        .iter()
        .filter(|(_, call_site)| guard_accepts(call_site))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted executed SQL outside src/sql.rs: {escaped:?}"
    );
}

/// The literal patterns are matched on every file under `src/`, not
/// only the Rust ones - the one thing the lexical scan cannot see.
#[test]
fn a_literal_in_a_non_rust_file_is_caught() {
    let dir = scratch("schema.sql", "-- prepare(\"SELECT 1\")\n");
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec!["src/schema.sql:1:-- prepare(\"SELECT 1\")"]
    );
}

/// The governor's Durable Object statements are consolidated in
/// `src/governor.rs` (module-local consts, validated by its host
/// tests) and executed through the storage adapter in
/// `src/governor_do.rs` - both sanctioned, and only there.
#[test]
fn the_governor_carve_outs_are_file_scoped() {
    // The engine's const spelling passes in its own module...
    assert!(guard_accepts_in(
        "governor.rs",
        "store.exec(CONSUME_OPS, &[pool.as_str().into()])?;",
    ));
    // ...but nowhere else, and a dynamic argument fails even there.
    assert!(!guard_accepts_in(
        "glue.rs",
        "store.exec(CONSUME_OPS, &[pool.as_str().into()])?;",
    ));
    assert!(!guard_accepts_in(
        "governor.rs",
        "store.exec(dynamic_sql, &[])?;"
    ));
    // The host-test adapter's exact `prepare(sql)` pass-through is
    // file-scoped too, and any other prepare argument stays rejected.
    assert!(guard_accepts_in(
        "governor.rs",
        "let mut statement = self.0.prepare(sql).map_err(|err| err.to_string())?;",
    ));
    assert!(!guard_accepts_in(
        "glue.rs",
        "let mut statement = self.0.prepare(sql).map_err(|err| err.to_string())?;",
    ));
    assert!(!guard_accepts_in(
        "governor.rs",
        "self.0.prepare(dynamic_sql)?;"
    ));
    // The adapter's pass-through is scoped to its file the same way,
    // and even there only the named parameters and consts pass -
    // dynamic and literal spellings stay rejected.
    assert!(guard_accepts_in(
        "governor_do.rs",
        "self.0.exec(sql, Some(bindings(params)))?;",
    ));
    assert!(guard_accepts_in(
        "governor_do.rs",
        "sql.exec(statement, None)?;\nself.0.exec(CHANGED_ROWS, None)?;",
    ));
    assert!(!guard_accepts_in(
        "glue.rs",
        "self.0.exec(sql, Some(bindings(params)))?;",
    ));
    assert!(!guard_accepts_in(
        "governor_do.rs",
        "self.0.exec(dynamic_sql, None)?;"
    ));
    assert!(!guard_accepts_in(
        "governor_do.rs",
        "self.0.exec(\"DROP TABLE objects\", None)?;",
    ));
}

/// The carve-outs key on the reported path, so a same-named file in a
/// subdirectory is not the governor module.
#[test]
fn the_governor_carve_outs_do_not_follow_the_file_name() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    let nested = dir.path().join("src").join("nested");
    fs::create_dir_all(&nested).expect("create scratch src/nested/");
    fs::write(nested.join("governor.rs"), "store.exec(CONSUME_OPS, &[])?;")
        .expect("write the call site");
    // Not even a nested `src/` prefix, which the Perl guard's suffix
    // match would have accepted.
    let deep = dir.path().join("src/nested/src");
    fs::create_dir_all(&deep).expect("create scratch src/nested/src/");
    fs::write(deep.join("governor.rs"), "store.exec(CONSUME_OPS, &[])?;")
        .expect("write the call site");
    assert_eq!(
        sql::check(dir.path()).expect("run the guard").len(),
        2,
        "a nested governor.rs was treated as the governor module"
    );
}

/// A violation names the file, the line, and enough of the argument to
/// find the call - the diagnostic is the whole point of the guard.
#[test]
fn a_violation_names_its_call_site() {
    let dir = scratch(
        "glue.rs",
        "let x = 1;\ndb.prepare(dynamic_sql).run().await?;\n",
    );
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec!["src/glue.rs:2: prepare(dynamic_sql).run().await?; "]
    );
}

/// The reported line is the line of the call, even when the argument
/// wraps - the case the Perl guard this replaces got wrong.
#[test]
fn a_wrapped_call_is_reported_on_its_own_line() {
    let dir = scratch("glue.rs", "db.prepare(\n    dynamic_sql,\n)\n.run();");
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec!["src/glue.rs:1: prepare( dynamic_sql, ) .run();"]
    );
}

/// A long argument is echoed up to the reported cap, so one violation
/// cannot flood the log.
#[test]
fn a_long_argument_is_truncated() {
    let call = format!("db.prepare({});", "a".repeat(500));
    let dir = scratch("glue.rs", &call);
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![format!("src/glue.rs:1: prepare({}", "a".repeat(39))]
    );
}

/// The literal passes come first and the lexical scan last, each sorted
/// by path - the order the guard documents, so the same tree always
/// reports the same way.
#[test]
fn violations_are_reported_pass_by_pass_then_in_path_order() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    fs::create_dir_all(dir.path().join("src")).expect("create scratch src/");
    fs::write(dir.path().join("src/a.rs"), "db.exec(x);").expect("write a");
    fs::write(dir.path().join("src/z.rs"), "db.prepare(\"x\");").expect("write z");
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/z.rs:1:db.prepare(\"x\");",
            "src/a.rs:1: exec(x);",
            // The lexical pass reads the blanked copy, so the string
            // body is gone from the echoed argument.
            "src/z.rs:1: prepare( );",
        ]
    );
}

/// The committed Worker sources pass. `registry.yml` is path-filtered,
/// so this is what runs the guard against the real tree when only the
/// guard itself changes.
#[test]
fn the_committed_worker_sources_pass() {
    let violations = sql::check(&registry_dir()).expect("run the guard");
    assert!(
        violations.is_empty(),
        "the committed Worker sources: {violations:?}"
    );
}

/// The binary reports violations on stdout, names the remedy on stderr,
/// and exits non-zero - the contract CI depends on.
#[test]
fn the_binary_reports_and_exits_non_zero() {
    let dir = scratch("glue.rs", "db.prepare(dynamic_sql);\n");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-sql", "--registry-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .stdout(contains("src/glue.rs:1: prepare("))
        .stderr(contains("route the statements above"));

    let clean = scratch("glue.rs", "db.prepare(sql::META_VALUE);\n");
    Command::new(env!("CARGO_BIN_EXE_xtask-registry-guard"))
        .args(["check-sql", "--registry-dir"])
        .arg(clean.path())
        .assert()
        .success();
}

/// The two literal passes keep their order relative to each other, not
/// just relative to the lexical scan.
#[test]
fn the_literal_passes_keep_their_order() {
    let dir = scratch(
        "notes.txt",
        "db.prepare(&format!(\"a\"));\ndb.prepare(\"b\");\n",
    );
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/notes.txt:2:db.prepare(\"b\");",
            "src/notes.txt:1:db.prepare(&format!(\"a\"));",
        ]
    );
}

/// A symlink under `src/` is walked, never fatal: the guard aborting
/// would report no violations at all.
#[cfg(unix)]
#[test]
fn symlinks_are_walked_without_aborting() {
    let dir = assert_fs::TempDir::new().expect("scratch tree");
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("real")).expect("create scratch src/real/");
    fs::write(src.join("real/glue.rs"), "db.prepare(dynamic_sql);").expect("write the call site");
    // A directory link is not descended (its target is scanned once, by
    // its real path); a link to a file is read through; a dangling one
    // is skipped.
    std::os::unix::fs::symlink(src.join("real"), src.join("link-dir")).expect("dir symlink");
    std::os::unix::fs::symlink(src.join("real/glue.rs"), src.join("link.rs"))
        .expect("file symlink");
    std::os::unix::fs::symlink(src.join("gone"), src.join("dangling.rs"))
        .expect("dangling symlink");
    let violations = sql::check(dir.path()).expect("run the guard");
    assert_eq!(
        violations,
        vec![
            "src/link.rs:1: prepare(dynamic_sql);",
            "src/real/glue.rs:1: prepare(dynamic_sql);",
        ]
    );
}

/// The guard the workflow runs is the one under test.
#[test]
fn the_workflow_runs_this_guard() {
    let workflow =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/registry.yml");
    let text = fs::read_to_string(&workflow).expect("read the registry workflow");
    assert!(
        text.contains("cargo check-sql"),
        "the registry workflow no longer runs cargo check-sql"
    );
    // The job is path-filtered; an edit to this crate - or a root
    // manifest edit that drops it from the workspace - must still reach
    // it, on both the push and the pull_request trigger.
    assert_eq!(
        text.matches("      - \"crates/xtask-registry-guard/**\"")
            .count(),
        2,
        "the registry workflow does not trigger on changes to this guard"
    );
    assert_eq!(
        text.matches("      - \"Cargo.toml\"").count(),
        2,
        "the registry workflow does not trigger on root-manifest changes"
    );
}
