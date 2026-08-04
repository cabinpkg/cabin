//! The one launch-guard behavior that is still a property of a shell
//! script: `scripts/wipe.sh` runs the guard before anything
//! destructive, and a refusal must stop it before the first mutation.
//!
//! The guard itself moved to `cargo registry-launch-guard`
//! (`crates/xtask-registry-admin`), and its own branches - every
//! refusal, the single pass state, the binding/name consistency
//! check - are tested beside it. What is left here is the integration:
//! that wipe.sh calls it at all, early enough, and dies on its
//! refusal. Testing that from the guard's own crate would prove
//! nothing about the script.
//!
//! Unix-only: wipe.sh is a bash script and so are the destructive
//! paths it drives.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

/// The `database_id` the real `wrangler.jsonc` binds - the guard's
/// remote mode cross-checks the account listing against it, so the
/// canned listing has to carry the same one.
fn config_database_id() -> String {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("wrangler.jsonc");
    let text = fs::read_to_string(config).expect("read wrangler.jsonc");
    let start = text.find("\"database_id\": \"").expect("database_id") + 16;
    text[start..start + 36].to_owned()
}

struct Shim {
    dir: PathBuf,
}

impl Shim {
    /// A fake `npx` first on `PATH` that logs every invocation and
    /// answers with `response`. It shadows `npx` rather than
    /// `wrangler`, so it intercepts both the script's own
    /// `wrangler()` helper and the guard binary's pinned constructor.
    fn new(name: &str, response: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        // A previous run's log would corrupt the invocation asserts.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("shim dir");
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
                 cat \"{response}\"\n",
                log = dir.join("log").display(),
                list = dir.join("response-list").display(),
                response = dir.join("response").display(),
            ),
        )
        .expect("write npx shim");
        fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("chmod npx");
        Self { dir }
    }

    /// Runs `script` (relative to `scripts/`) with the shim first on
    /// `PATH`; the rest of `PATH` stays, so `bash`, `node` and the
    /// `cargo` the script now reaches the guard through all resolve.
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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
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
        r#"[{"results":[{"value":"true"}],"success":true}]"#,
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
