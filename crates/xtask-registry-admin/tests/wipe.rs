//! Host-target tests for the wipe's own contracts, extracted from the
//! retired shell-vs-port differential: the real command runs against a
//! fake `npx` shim on `PATH` (the same pattern as `launch_guard.rs`)
//! and a scratch registry root reached through `CABIN_REGISTRY_DIR`.
//!
//! Unix-only: the shim is a shell script, and so are the destructive
//! paths the wipe protects.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

/// The emulated state `--local` deletes, and the two decoys it must
/// leave alone: `kv` is a sibling state directory the wipe does not
/// name, and the loose file proves the removal is four named paths
/// rather than `.wrangler/state/v3` wholesale.
const EMULATED: [(&str, bool); 6] = [
    ("state/v3/d1/miniflare-D1DatabaseObject/db.sqlite", false),
    ("state/v3/r2/miniflare-R2BucketObject/blob.bin", false),
    ("state/v3/do/miniflare-DurableObject/ledger.sqlite", false),
    ("state/v3/cache/default/entry.bin", false),
    ("state/v3/kv/miniflare-KVNamespaceObject/kv.sqlite", true),
    ("state/v3/keep-me.json", true),
];

/// A scratch registry root plus a fake `npx` that logs every call and
/// answers the two `SELECT`s the wipe makes with canned rows.
struct World {
    dir: assert_fs::TempDir,
}

impl World {
    fn new(launched: &str) -> Self {
        let dir = assert_fs::TempDir::new().expect("a scratch directory");
        for (path, _) in EMULATED {
            let file = dir.path().join("root/.wrangler").join(path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("emulated state");
            fs::write(file, b"state").expect("emulated state");
        }
        let shim = dir.path().join("bin");
        fs::create_dir_all(&shim).expect("the shim directory");
        let npx = shim.join("npx");
        fs::write(
            &npx,
            format!(
                "#!/usr/bin/env bash\n\
                 printf '%s\\n' \"$*\" >>\"{log}\"\n\
                 case \"$*\" in\n\
                 *\"SELECT value\"*launched*) echo '[{{\"results\":[{{\"value\":\"{launched}\"}}],\"success\":true}}]' ;;\n\
                 *\"SELECT value\"*registry_generation*) echo '[{{\"results\":[{{\"value\":\"7\"}}],\"success\":true}}]' ;;\n\
                 *) echo '[{{\"results\":[],\"success\":true}}]' ;;\n\
                 esac\n",
                log = dir.path().join("log").display(),
            ),
        )
        .expect("the npx shim");
        fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("chmod npx");
        Self { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    fn wipe(&self, arguments: &[&str]) -> assert_cmd::Command {
        let mut command =
            assert_cmd::Command::cargo_bin("xtask-registry-admin").expect("the binary");
        command.arg("wipe").args(arguments);
        command
            .env("CABIN_REGISTRY_DIR", self.root())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.path().join("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env_remove("CABIN_WIPE_YES")
            .env_remove("CLOUDFLARE_API_TOKEN");
        command
    }

    /// One line per fake-`npx` invocation, in order.
    fn log(&self) -> Vec<String> {
        match fs::read_to_string(self.dir.path().join("log")) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn surviving(&self) -> Vec<&'static str> {
        EMULATED
            .iter()
            .filter(|(path, _)| self.root().join(".wrangler").join(path).exists())
            .map(|(path, _)| *path)
            .collect()
    }
}

/// A declined confirmation stops the remote wipe before anything runs:
/// no launch-guard read, no wrangler call at all.
#[test]
fn a_declined_confirmation_stops_before_the_launch_guard() {
    let world = World::new("false");
    world
        .wipe(&[])
        .write_stdin("no\n")
        .assert()
        .failure()
        .stderr(predicates::str::contains("FAIL: not confirmed"));
    assert_eq!(world.log(), Vec::<String>::new(), "nothing ran at all");
}

/// A refused launch guard leaves the local state exactly as it was:
/// the guard's one read is the only wrangler call.
#[test]
fn a_refused_guard_leaves_the_local_state_intact() {
    let world = World::new("true");
    world
        .wipe(&["--local"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("launched"));
    let expected: Vec<&str> = EMULATED.iter().map(|(path, _)| *path).collect();
    assert_eq!(world.surviving(), expected, "every path is still there");
    assert_eq!(world.log().len(), 1, "only the guard's read ran");
}

/// The local wipe deletes the four named state directories and leaves
/// the decoys: a sibling `kv` store and a loose file under `v3`.
#[test]
fn a_local_wipe_clears_the_emulated_state_and_nothing_else() {
    let world = World::new("false");
    world
        .wipe(&["--local"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "local wipe OK (generation 7 -> 8)",
        ));
    let decoys: Vec<&str> = EMULATED
        .iter()
        .filter(|(_, decoy)| *decoy)
        .map(|(path, _)| *path)
        .collect();
    assert_eq!(world.surviving(), decoys, "only the decoys survive");
    let log = world.log();
    let expected = [
        "SELECT value FROM meta WHERE key = 'launched'",
        "SELECT value FROM meta WHERE key = 'registry_generation'",
        "d1 migrations apply DB --local",
        "UPDATE meta SET value = '8' WHERE key = 'registry_generation'",
    ];
    assert_eq!(log.len(), expected.len(), "{log:?}");
    for (call, fragment) in log.iter().zip(expected) {
        assert!(call.contains(fragment), "expected {fragment:?} in {call:?}");
    }
}
