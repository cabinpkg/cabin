//! Host-target tests for the launch guard (`registry/docs/runbook.md`,
//! "Data policy"): the real command runs against a fake `npx` shim on
//! `PATH` that logs every invocation and answers with a canned wrangler
//! response, so every refusal branch - and the single pass state - is
//! exercised hermetically.
//!
//! The shim shadows `npx`, not `wrangler`, because the pinned
//! constructor spawns `npx --yes wrangler@<version>`; that is what
//! lets these tests carry over unchanged from the bash guard they
//! replace.
//!
//! Unix-only: the shim is a shell script, and so are the destructive
//! paths the guard protects.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xtask_registry_admin::WRANGLER;

fn registry_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry")
}

/// The canned behavior of the fake `npx`: emit `response` on stdout
/// (exit 0), or fail outright.
#[derive(Clone, Copy)]
enum FakeWrangler<'a> {
    Respond(&'a str),
    Fail,
}

/// The `database_id` the real `wrangler.jsonc` currently binds - the
/// guard's remote mode cross-checks the account listing against it.
fn config_database_id() -> String {
    let text = fs::read_to_string(registry_dir().join("wrangler.jsonc")).expect("wrangler.jsonc");
    let start = text.find("\"database_id\": \"").expect("database_id") + 16;
    text[start..start + 36].to_owned()
}

struct Shim {
    dir: PathBuf,
}

impl Shim {
    /// `name` keys the shim's scratch directory under cargo's per-crate
    /// `target/tmp`, so parallel tests never share a log.
    ///
    /// The shim serves `d1 list` from its own canned response (a
    /// one-database account whose `cabin-registry` carries the config's
    /// bound id, so the guard's consistency check passes); `behavior`
    /// governs every other wrangler invocation.
    fn new(name: &str, behavior: FakeWrangler<'_>) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        // A previous run's log would corrupt the invocation asserts.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("shim dir");
        let (fail, response) = match behavior {
            FakeWrangler::Respond(body) => ("", body),
            FakeWrangler::Fail => ("1", ""),
        };
        fs::write(dir.join("response"), response).expect("write response");
        let list = format!(
            r#"[{{"name":"cabin-registry","uuid":"{}"}}]"#,
            config_database_id()
        );
        fs::write(dir.join("response-list"), list).expect("write list response");
        let npx = dir.join("npx");
        fs::write(
            &npx,
            format!(
                "#!/usr/bin/env bash\n\
                 printf '%s\\n' \"$*\" >>\"{log}\"\n\
                 if [[ \"$*\" == *\" d1 list \"* || \"$*\" == *\" d1 list\" ]]; then cat \"{list}\"; exit 0; fi\n\
                 if [[ -n \"{fail}\" ]]; then echo 'fake wrangler: boom' >&2; exit 1; fi\n\
                 cat \"{response}\"\n",
                log = dir.join("log").display(),
                list = dir.join("response-list").display(),
                fail = fail,
                response = dir.join("response").display(),
            ),
        )
        .expect("write npx shim");
        fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("chmod npx");
        Self { dir }
    }

    /// Rewrites the `d1 list` response so the account's `cabin-registry`
    /// carries `uuid` instead of the config's bound id.
    fn set_account_database_id(&self, uuid: &str) {
        let list = format!(r#"[{{"name":"cabin-registry","uuid":"{uuid}"}}]"#);
        fs::write(self.dir.join("response-list"), list).expect("rewrite list response");
    }

    /// Runs the guard with the shim first on `PATH`; the rest of `PATH`
    /// stays, so `bash` still resolves.
    fn run(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.display(),
            std::env::var("PATH").expect("PATH")
        );
        Command::new(assert_cmd::cargo::cargo_bin("xtask-registry-admin"))
            .arg("launch-guard")
            .args(args)
            .env("PATH", path)
            .output()
            .expect("run the guard")
    }

    /// One line per fake-`npx` invocation, in order.
    fn log(&self) -> Vec<String> {
        match fs::read_to_string(self.dir.join("log")) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn meta_response(value: &str) -> String {
    format!(r#"[{{"results":[{{"value":"{value}"}}],"success":true}}]"#)
}

const NO_ROW: &str = r#"[{"results":[],"success":true}]"#;

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn passes_when_not_launched_and_queries_the_flag() {
    let shim = Shim::new("pass-false", FakeWrangler::Respond(&meta_response("false")));
    let output = shim.run(&["--remote"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // A passing guard says nothing: destructive scripts run it for its
    // status, and a line on stdout would land in the middle of theirs.
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    // Exactly two wrangler calls, neither mutating: the account listing
    // for the binding/name consistency check, then the flag read.
    let log = shim.log();
    assert_eq!(log.len(), 2, "log: {log:?}");
    assert!(
        log[0].starts_with(&format!("--yes {WRANGLER} d1 list --json")),
        "log: {log:?}"
    );
    assert!(
        log[1].starts_with(&format!(
            "--yes {WRANGLER} d1 execute DB --remote --json --command"
        )),
        "log: {log:?}"
    );
    assert!(log[1].contains("key = 'launched'"), "log: {log:?}");
}

#[test]
fn refuses_when_the_binding_and_the_account_disagree() {
    // A stale wrangler.jsonc binding must refuse before the flag is even
    // read: the guard would otherwise read one database while a wipe
    // deletes another.
    let shim = Shim::new(
        "refuse-id-mismatch",
        FakeWrangler::Respond(&meta_response("false")),
    );
    shim.set_account_database_id("11111111-2222-3333-4444-555555555555");
    let output = shim.run(&["--remote"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("binds"), "stderr: {message}");
    assert!(message.contains("fail-safe"), "stderr: {message}");
    // Only the listing ran - the flag read never happened.
    assert_eq!(shim.log().len(), 1, "log: {:?}", shim.log());
}

#[test]
fn respects_the_local_mode() {
    let shim = Shim::new("pass-local", FakeWrangler::Respond(&meta_response("false")));
    let output = shim.run(&["--local"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // Local state has no name resolution; the DB binding is the state,
    // so the account listing never runs.
    let log = shim.log();
    assert_eq!(log.len(), 1, "log: {log:?}");
    assert!(
        log[0].starts_with(&format!("--yes {WRANGLER} d1 execute DB --local")),
        "log: {log:?}"
    );
}

#[test]
fn refuses_when_launched() {
    let shim = Shim::new(
        "refuse-launched",
        FakeWrangler::Respond(&meta_response("true")),
    );
    let output = shim.run(&["--remote"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("launched"), "stderr: {message}");
    assert!(message.contains("forbidden"), "stderr: {message}");
    // `cargo registry-smoke` greps a refused wipe's output for
    // exactly this substring; it is load-bearing, not decoration.
    assert!(
        message.contains("meta.launched = 'true'"),
        "stderr: {message}"
    );
}

#[test]
fn refuses_fail_safe_on_a_missing_row() {
    let shim = Shim::new("refuse-missing-row", FakeWrangler::Respond(NO_ROW));
    let output = shim.run(&["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn refuses_fail_safe_on_an_unexpected_value() {
    // Only the exact string 'false' passes - not casing variants, and
    // not a trailing newline, which command substitution used to strip.
    for value in ["False", "TRUE", "yes", "", "false "] {
        let shim = Shim::new(
            &format!("refuse-value-{}", value.replace(' ', "_")),
            FakeWrangler::Respond(&meta_response(value)),
        );
        let output = shim.run(&["--remote"]);
        assert!(!output.status.success(), "accepted {value:?}");
        assert!(
            stderr(&output).contains("fail-safe"),
            "stderr: {}",
            stderr(&output)
        );
    }
}

#[test]
fn refuses_fail_safe_when_wrangler_fails() {
    let shim = Shim::new("refuse-wrangler-failure", FakeWrangler::Fail);
    let output = shim.run(&["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn refuses_fail_safe_on_malformed_wrangler_output() {
    let shim = Shim::new("refuse-malformed", FakeWrangler::Respond("not json at all"));
    let output = shim.run(&["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn rejects_an_unknown_mode_without_querying() {
    for mode in [vec!["--both"], vec![], vec!["--local", "--remote"]] {
        let shim = Shim::new(
            &format!("unknown-mode-{}", mode.len()),
            FakeWrangler::Respond(&meta_response("false")),
        );
        let output = shim.run(&mode);
        assert!(!output.status.success(), "accepted {mode:?}");
        // Nothing was asked of the account before the mode was refused.
        assert_eq!(shim.log().len(), 0, "log: {:?}", shim.log());
    }
}

/// A JSON boolean and a one-element array both coerce to `false`
/// through `String()`, so the shell passed them and so does this.  D1
/// returns TEXT today, but the guard's contract is the coercion, and
/// narrowing it here would refuse a state the shell allowed.
#[test]
fn the_flag_is_read_through_javascript_string_coercion() {
    for (name, body, passes) in [
        // `String(false)` is "false".
        (
            "boolean",
            r#"[{"results":[{"value":false}],"success":true}]"#,
            true,
        ),
        // `String(["false"])` is "false" too - Array#toString joins.
        (
            "array",
            r#"[{"results":[{"value":["false"]}],"success":true}]"#,
            true,
        ),
        // ...and it recurses.
        (
            "nested array",
            r#"[{"results":[{"value":[["false"]]}],"success":true}]"#,
            true,
        ),
        // `String(["false","true"])` is "false,true", which is not the
        // flag and must refuse.
        (
            "two elements",
            r#"[{"results":[{"value":["false","true"]}],"success":true}]"#,
            false,
        ),
        // A row that is not an object at all is not the shape the query
        // asked for.
        (
            "row is an array",
            r#"[{"results":[["false"]],"success":true}]"#,
            false,
        ),
    ] {
        let shim = Shim::new(
            &format!("coercion-{}", name.replace(' ', "-")),
            FakeWrangler::Respond(body),
        );
        let output = shim.run(&["--remote"]);
        assert_eq!(
            output.status.success(),
            passes,
            "{name}: stderr {}",
            stderr(&output)
        );
    }
}
