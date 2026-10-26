//! Host-target tests for the wipe's own contracts: the real command
//! runs against a fake `node` shim on `PATH` (the same pattern as
//! `launch_guard.rs`) and a scratch registry root reached through
//! `CABIN_REGISTRY_DIR`.
//!
//! Unix-only: the shim is a shell script.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

/// The database id the scratch `wrangler.jsonc` binds before a remote
/// wipe, and the one the fake account hands back once it is recreated.
const OLD_ID: &str = "00000000-0000-0000-0000-000000000000";
const NEW_ID: &str = "1234abcd-1234-abcd-1234-abcd1234abcd";

/// A scratch registry root plus a fake `node` that logs every call and
/// answers the two `SELECT`s the wipe makes with canned rows.  For the
/// remote mode the root also carries a `wrangler.jsonc` and a migration,
/// and the fake account lists `cabin-registry` under [`OLD_ID`] until a
/// `d1 create` swaps in [`NEW_ID`].
struct World {
    dir: assert_fs::TempDir,
}

impl World {
    fn new(launched: &str) -> Self {
        Self::with_launched_rows(&format!(r#"[{{"value":"{launched}"}}]"#))
    }

    fn with_launched_rows(rows: &str) -> Self {
        let dir = assert_fs::TempDir::new().expect("a scratch directory");
        for (path, _) in EMULATED {
            let file = dir.path().join("root/.wrangler").join(path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("emulated state");
            fs::write(file, b"state").expect("emulated state");
        }
        fs::write(
            dir.path().join("root/wrangler.jsonc"),
            format!(
                "{{\n  \"vars\": {{\n    \"CF_ACCOUNT_ID\": \"{}\",\n    \
                 \"D1_DATABASE_ID\": \"{OLD_ID}\"\n  }},\n  \"d1_databases\": [\n    \
                 {{ \"binding\": \"DB\", \"database_id\": \"{OLD_ID}\" }}\n  ]\n}}\n",
                "0123456789abcdef".repeat(2)
            ),
        )
        .expect("the scratch wrangler.jsonc");
        fs::create_dir_all(dir.path().join("root/migrations")).expect("migrations/");
        fs::write(dir.path().join("root/migrations/0001_init.sql"), b"one").expect("a migration");
        let shim = dir.path().join("bin");
        fs::create_dir_all(&shim).expect("the shim directory");
        let node = shim.join("node");
        let created = dir.path().join("created");
        fs::write(
            &node,
            format!(
                "#!/usr/bin/env bash\n\
                 printf '%s\\n' \"$*\" >>\"{log}\"\n\
                 case \"$*\" in\n\
                 *\"d1 list\"*) if [[ -e \"{created}\" ]]; then id={NEW_ID}; else id={OLD_ID}; fi\n\
                 echo \"[{{\\\"name\\\":\\\"cabin-registry\\\",\\\"uuid\\\":\\\"$id\\\"}}]\" ;;\n\
                 *\"d1 create\"*) touch \"{created}\" ;;\n\
                 *\"SELECT value\"*launched*) echo '[{{\"results\":{rows},\"success\":true}}]' ;;\n\
                 *\"SELECT value\"*registry_generation*) echo '[{{\"results\":[{{\"value\":\"7\"}}],\"success\":true}}]' ;;\n\
                 *) echo '[{{\"results\":[],\"success\":true}}]' ;;\n\
                 esac\n",
                log = dir.path().join("log").display(),
                created = created.display(),
            ),
        )
        .expect("the node shim");
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).expect("chmod node");
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
            .env_remove("CLOUDFLARE_API_TOKEN")
            .env_remove("CF_API_BASE");
        command
    }

    /// The confirmed remote wipe, its R2 calls sent to `r2`.
    fn remote(&self, r2: &R2) -> assert_cmd::Command {
        let mut command = self.wipe(&[]);
        command
            .env("CABIN_WIPE_YES", "1")
            .env("CLOUDFLARE_API_TOKEN", "token")
            .env("CF_API_BASE", &r2.url);
        command
    }

    /// Whether any call that changes the database, its config, or the
    /// deployment ran.
    fn destroyed(&self) -> bool {
        self.log().iter().any(|call| {
            [
                "d1 delete",
                "d1 create",
                "migrations apply",
                "UPDATE meta",
                " deploy",
            ]
            .iter()
            .any(|fragment| call.contains(fragment))
        })
    }

    fn config(&self) -> String {
        fs::read_to_string(self.root().join("wrangler.jsonc")).expect("wrangler.jsonc")
    }

    /// One line per fake-`node` invocation, in order.
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

/// A stand-in for the R2 REST API: every request gets one canned
/// answer, and every request line is recorded.
struct R2 {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl R2 {
    fn answering(status: u16, body: &'static str) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind the R2 stand-in");
        let url = format!("http://{}", server.server_addr().to_ip().expect("an IP"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                seen.lock().expect("the request log").push(format!(
                    "{} {}",
                    request.method(),
                    request.url()
                ));
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_status_code(status));
            }
        });
        Self { url, requests }
    }

    /// An account whose buckets are both already empty.
    fn empty() -> Self {
        Self::answering(200, r#"{"success":true,"errors":[],"result":[]}"#)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the request log").clone()
    }
}

/// A launch guard that refuses - the registry launched, or no flag to
/// read - stops the remote wipe before R2 or the database is touched.
#[test]
fn a_refused_guard_stops_the_remote_wipe_before_anything_destructive() {
    for rows in [r#"[{"value":"true"}]"#, "[]"] {
        let world = World::with_launched_rows(rows);
        let r2 = R2::empty();
        world
            .remote(&r2)
            .assert()
            .failure()
            .stderr(predicates::str::contains("launch guard"));
        assert!(!world.destroyed(), "{rows}: {:?}", world.log());
        assert_eq!(r2.requests(), Vec::<String>::new(), "{rows}");
    }
}

/// Without an R2 token the sweeps cannot run, so nothing runs at all.
#[test]
fn a_remote_wipe_without_an_r2_token_destroys_nothing() {
    let world = World::new("false");
    let r2 = R2::empty();
    world
        .remote(&r2)
        .env_remove("CLOUDFLARE_API_TOKEN")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "CLOUDFLARE_API_TOKEN is required",
        ));
    assert!(!world.destroyed(), "{:?}", world.log());
    assert_eq!(r2.requests(), Vec::<String>::new());
}

/// A token R2 refuses fails the first sweep, while the database still
/// exists: nothing past it runs, and the config is untouched.
#[test]
fn a_refused_r2_listing_stops_the_remote_wipe_before_the_database() {
    let world = World::new("false");
    let before = world.config();
    let r2 = R2::answering(403, r#"{"success":false,"errors":[{"code":10000}]}"#);
    world
        .remote(&r2)
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "FAIL: listing cabin-registry-blobs failed",
        ));
    assert!(!world.destroyed(), "{:?}", world.log());
    assert_eq!(world.config(), before);
}

/// The whole remote procedure, in order: both buckets swept (the backup
/// one without a prefix) before the database is dropped, then the
/// recreated id baked into both config sites, the migrations applied and
/// stamped, the generation bumped, and the Worker redeployed.
#[test]
fn a_remote_wipe_sweeps_both_buckets_before_recreating_the_database() {
    let world = World::new("false");
    let r2 = R2::empty();
    world
        .remote(&r2)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "wipe OK (generation 7 -> 8; 0 primary and 0 backup object(s) deleted)",
        ));
    let account = "0123456789abcdef".repeat(2);
    assert_eq!(
        r2.requests(),
        [
            format!(
                "GET /accounts/{account}/r2/buckets/cabin-registry-blobs/objects\
                 ?per_page=500&prefix=blobs/"
            ),
            format!(
                "GET /accounts/{account}/r2/buckets/cabin-registry-backup/objects?per_page=500"
            ),
        ]
    );
    let log = world.log();
    let expected = [
        " d1 list --json",
        "SELECT value FROM meta WHERE key = 'launched'",
        "SELECT value FROM meta WHERE key = 'registry_generation'",
        " d1 delete cabin-registry -y",
        " d1 create cabin-registry",
        " d1 list --json",
        " d1 migrations apply DB --remote",
        "UPDATE meta SET value = '8' WHERE key = 'registry_generation'",
        " deploy",
    ];
    assert_eq!(log.len(), expected.len(), "{log:?}");
    for (call, fragment) in log.iter().zip(expected) {
        assert!(call.contains(fragment), "expected {fragment:?} in {call:?}");
    }
    let config = world.config();
    assert_eq!(config.matches(NEW_ID).count(), 2, "{config}");
    assert!(!config.contains(OLD_ID), "{config}");
    // `printf one | shasum -a 256`.
    assert_eq!(
        fs::read_to_string(world.root().join("migrations-applied")).expect("the stamp"),
        "7692c3ad3540bb803c020b3aee66cd8887123234ea0c6e7143c0add73ff431ed\n"
    );
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
