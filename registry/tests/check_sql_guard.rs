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
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs the real guard over a scratch tree containing `call_site`;
/// `true` means the guard accepted it.
fn guard_accepts(name: &str, call_site: &str) -> bool {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create scratch src/");
    fs::create_dir_all(dir.join("scripts")).expect("create scratch scripts/");
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    for script in ["check-sql.sh", "check-sql.pl"] {
        fs::copy(scripts.join(script), dir.join("scripts").join(script)).expect("copy the guard");
    }
    fs::write(dir.join("src/glue.rs"), call_site).expect("write the call site");

    let status = Command::new("bash")
        .arg("scripts/check-sql.sh")
        .current_dir(&dir)
        .output()
        .expect("run the guard");
    status.status.success()
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
