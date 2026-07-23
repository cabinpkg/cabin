//! Regression cases for the SQL consolidation guard
//! (`scripts/check-sql.sh`, see `docs/architecture.md`, "Why no ORM"):
//! the real script runs against a scratch tree whose `src/` holds one
//! synthetic call site, so every way executed SQL could grow outside
//! `src/sql.rs` - a literal, a `format!`, a dynamic argument, the
//! multi-line spelling, the raw-identifier and UFCS spellings, and
//! D1's unprepared `exec` - stays caught. An untested guard is the one
//! that rots. Unix-only: the guard is a bash script.
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

mod common;

/// Runs the real guard over a scratch tree containing `call_site` at
/// `src/<file>`; `true` means the guard accepted it.
fn guard_accepts_in(name: &str, file: &str, call_site: &str) -> bool {
    let dir = common::scratch(name);
    common::copy_scripts(&dir, &["check-sql.sh", "check-sql.pl", "lexical.pm"]);
    fs::create_dir_all(dir.join("src")).expect("create scratch src/");
    fs::write(dir.join("src").join(file), call_site).expect("write the call site");
    common::bash_accepts(&dir, "check-sql.sh", &[])
}

fn guard_accepts(name: &str, call_site: &str) -> bool {
    guard_accepts_in(name, "glue.rs", call_site)
}

/// The canonical spelling - and the shapes around it that are not
/// prepare calls at all - must pass, or the guard would block ordinary
/// work.
#[test]
fn the_canonical_call_site_passes() {
    let accepted = guard_accepts(
        "guard_canonical",
        concat!(
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
        ),
    );
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
            // grep is line-oriented; the scan must not be.
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
        .filter(|(name, call_site)| guard_accepts(&format!("guard_{name}"), call_site))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        escaped.is_empty(),
        "the guard accepted executed SQL outside src/sql.rs: {escaped:?}"
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
        "guard_governor_const_exec",
        "governor.rs",
        "store.exec(CONSUME_OPS, &[pool.as_str().into()])?;",
    ));
    // ...but nowhere else, and a dynamic argument fails even there.
    assert!(!guard_accepts_in(
        "guard_governor_const_exec_elsewhere",
        "glue.rs",
        "store.exec(CONSUME_OPS, &[pool.as_str().into()])?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_dynamic_exec",
        "governor.rs",
        "store.exec(dynamic_sql, &[])?;",
    ));
    // The host-test adapter's exact `prepare(sql)` pass-through is
    // file-scoped too, and any other prepare argument stays rejected.
    assert!(guard_accepts_in(
        "guard_governor_test_adapter_prepare",
        "governor.rs",
        "let mut statement = self.0.prepare(sql).map_err(|err| err.to_string())?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_test_adapter_prepare_elsewhere",
        "glue.rs",
        "let mut statement = self.0.prepare(sql).map_err(|err| err.to_string())?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_dynamic_prepare",
        "governor.rs",
        "self.0.prepare(dynamic_sql)?;",
    ));
    // The adapter's pass-through is scoped to its file the same way,
    // and even there only the named parameters and consts pass -
    // dynamic and literal spellings stay rejected.
    assert!(guard_accepts_in(
        "guard_governor_do_passthrough",
        "governor_do.rs",
        "self.0.exec(sql, Some(bindings(params)))?;",
    ));
    assert!(guard_accepts_in(
        "guard_governor_do_schema_and_const",
        "governor_do.rs",
        "sql.exec(statement, None)?;\nself.0.exec(CHANGED_ROWS, None)?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_do_passthrough_elsewhere",
        "glue.rs",
        "self.0.exec(sql, Some(bindings(params)))?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_do_dynamic",
        "governor_do.rs",
        "self.0.exec(dynamic_sql, None)?;",
    ));
    assert!(!guard_accepts_in(
        "guard_governor_do_literal",
        "governor_do.rs",
        "self.0.exec(\"DROP TABLE objects\", None)?;",
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
        text.contains("bash scripts/check-sql.sh"),
        "the registry workflow no longer runs scripts/check-sql.sh"
    );
}
