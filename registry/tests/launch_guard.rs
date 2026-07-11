//! Host-target tests for the launch guard (`scripts/launch-guard.sh`,
//! see `docs/runbook.md`, "Data policy"): the real script runs against
//! a fake `npx` shim on `PATH` that logs every invocation and answers
//! with a canned wrangler response, so every refusal branch - and the
//! single pass state - is exercised hermetically. Unix-only: the guard
//! is a bash script and so are the destructive paths it protects.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

struct Shim {
    dir: PathBuf,
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("wrangler.jsonc");
    let text = fs::read_to_string(config).expect("read wrangler.jsonc");
    let start = text.find("\"database_id\": \"").expect("database_id") + 16;
    text[start..start + 36].to_owned()
}

impl Shim {
    /// `name` keys the shim's scratch directory under cargo's
    /// per-crate `target/tmp`, so parallel tests never share a log.
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

    /// Runs `script` (relative to `scripts/`) with the shim first on
    /// `PATH`; the rest of `PATH` stays, so `bash` and `node` are real.
    fn run(&self, script: &str, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.display(),
            std::env::var("PATH").expect("PATH")
        );
        Command::new(scripts_dir().join(script))
            .args(args)
            .env("PATH", path)
            .output()
            .expect("run script")
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
    let output = shim.run("launch-guard.sh", &["--remote"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // Exactly two wrangler calls, neither mutating: the account listing
    // for the binding/name consistency check, then the flag read.
    let log = shim.log();
    assert_eq!(log.len(), 2, "log: {log:?}");
    assert!(
        log[0].starts_with("--yes wrangler d1 list --json"),
        "log: {log:?}"
    );
    assert!(
        log[1].starts_with("--yes wrangler d1 execute DB --remote --json --command"),
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
    let output = shim.run("launch-guard.sh", &["--remote"]);
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
    let output = shim.run("launch-guard.sh", &["--local"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // Local state has no name resolution; the DB binding is the state.
    assert!(
        shim.log()[0].starts_with("--yes wrangler d1 execute DB --local"),
        "log: {:?}",
        shim.log()
    );
}

#[test]
fn refuses_when_launched() {
    let shim = Shim::new(
        "refuse-launched",
        FakeWrangler::Respond(&meta_response("true")),
    );
    let output = shim.run("launch-guard.sh", &["--remote"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(message.contains("launched"), "stderr: {message}");
    assert!(message.contains("forbidden"), "stderr: {message}");
}

#[test]
fn refuses_fail_safe_on_a_missing_row() {
    let shim = Shim::new("refuse-missing-row", FakeWrangler::Respond(NO_ROW));
    let output = shim.run("launch-guard.sh", &["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn refuses_fail_safe_on_an_unexpected_value() {
    // Only the exact string 'false' passes - not casing variants.
    let shim = Shim::new(
        "refuse-casing",
        FakeWrangler::Respond(&meta_response("False")),
    );
    let output = shim.run("launch-guard.sh", &["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn refuses_fail_safe_when_wrangler_fails() {
    let shim = Shim::new("refuse-wrangler-failure", FakeWrangler::Fail);
    let output = shim.run("launch-guard.sh", &["--remote"]);
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
    let output = shim.run("launch-guard.sh", &["--remote"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("fail-safe"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn rejects_an_unknown_mode_without_querying() {
    let shim = Shim::new(
        "unknown-mode",
        FakeWrangler::Respond(&meta_response("false")),
    );
    let output = shim.run("launch-guard.sh", &["--both"]);
    assert!(!output.status.success());
    assert_eq!(shim.log().len(), 0, "log: {:?}", shim.log());
}

#[test]
fn wipe_refuses_when_launched_before_any_mutation() {
    // The integration that matters: wipe.sh runs the guard first, and a
    // refusal stops it before anything destructive - the only wrangler
    // call on the log is the guard's read of the flag, and the local
    // state directories survive (a sentinel file catches a reordering
    // that would `rm -rf` before the guard).
    let state = Path::new(env!("CARGO_MANIFEST_DIR")).join(".wrangler/state/v3/d1");
    fs::create_dir_all(&state).expect("state dir");
    let sentinel = state.join("__launch_guard_sentinel__");
    fs::write(&sentinel, b"still here").expect("write sentinel");

    let shim = Shim::new(
        "wipe-refusal",
        FakeWrangler::Respond(&meta_response("true")),
    );
    let output = shim.run("wipe.sh", &["--local"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("launched"),
        "stderr: {}",
        stderr(&output)
    );
    let log = shim.log();
    assert_eq!(log.len(), 1, "log: {log:?}");
    assert!(log[0].contains("SELECT value FROM meta"), "log: {log:?}");

    assert!(
        sentinel.exists(),
        "the refused wipe deleted the local state"
    );
    fs::remove_file(&sentinel).expect("remove sentinel");
}
