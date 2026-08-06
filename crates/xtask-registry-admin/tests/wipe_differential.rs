//! The whole-run differential for `scripts/wipe.sh`: the shell it
//! replaces and the port, run over one corpus of synthetic registry
//! roots, canned wrangler answers and a canned R2 account, compared on
//! stdout, stderr, exit status, the sequence of commands each side
//! issued, the requests each side made of R2, and everything each left
//! behind - `wrangler.jsonc`, `migrations-applied`, the `.wrangler`
//! tree and the surviving objects.
//!
//! `tests/fixtures/wipe.sh.orig` is the original, byte for byte:
//! `registry/scripts/wipe.sh` as it stood on `main` at `0d6cf8171`,
//! `sha256`
//! `bb111b6775bf2191620189cdec8cb77933d8f92dc8503e2a345765f44069c656`.
//! It sources `scripts/lib.sh` after `cd`-ing to the registry root, and
//! that file is the same one the migrate differential vendors, so this
//! suite reuses `tests/fixtures/migrate-lib.sh.orig` rather than
//! vendoring a second copy of identical bytes - `sha256`
//! `8d7a969ace6443efc5f3a478195da9c5e002a75cd7c2c2bc8140fa57edff556f`,
//! unchanged between `098cd643d` and `0d6cf8171`. Both are copied into
//! each scenario's root as `scripts/wipe.sh` and `scripts/lib.sh`.
//! Nothing is prepended and nothing is edited - this suite *runs* those
//! files, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # A synthetic registry root, per side
//!
//! The script's first act is `cd "$(dirname -- "${BASH_SOURCE[0]}")/.."`,
//! so it operates on whatever tree it sits in. Each side therefore gets
//! its own scratch root holding `wrangler.jsonc`, `migrations/`,
//! `migrations-applied`, a `.wrangler/` state tree and a `scripts/`
//! directory with both vendored files copied in. The port is pointed at
//! the same root through [`ROOT_VARIABLE`].
//!
//! The roots are per *side*, not per scenario, and so is the R2 mock:
//! this run MUTATES everything it reads. A shared root would let the
//! shell's rewritten `wrangler.jsonc` stand in for the port's, and a
//! shared bucket would let the shell's sweep drain the objects the port
//! was supposed to delete - the second side would then pass by finding
//! nothing to do. Each side gets its own of both, seeded identically,
//! and the LEFT STATE is compared like any other output. That is the
//! only way a scenario can say "and nothing was destroyed".
//!
//! Neither side is run against this checkout: [`the_real_registry_is_
//! never_touched`] backs that with a check on `registry/wrangler.jsonc`
//! and `registry/migrations-applied`, and every scenario re-checks both
//! across its two runs.
//!
//! # Three seams, because the script shells out three ways
//!
//! `tests/fixtures/wipe-bin` goes first on both sides' `PATH` and holds
//! a stand-in for each. All three append to one `$FAKE_NPX_LOG`, in the
//! order the side called them, each record leading with which shim
//! wrote it.
//!
//! **`npx`**, which is how `lib.sh`'s
//! `wrangler() { npx --yes wrangler@4.112.0 "$@"; }` and the port's
//! `xtask_registry_admin::wrangler` both reach wrangler. Its log is the
//! command-parity artifact, exactly as in the migrate differential: the
//! two sides must issue the identical sequence, so a port that
//! reordered arguments, reflowed SQL or skipped a call fails here even
//! when it reaches the same verdict. Each record carries WHERE the call
//! was made, since wrangler binds D1 and R2 through the
//! `wrangler.jsonc` of its working directory; [`diff`] requires every
//! call on both sides to carry `[root]` and checks that BEFORE
//! comparing the sequences, since two sides that were both wrong would
//! compare equal.
//!
//! The sharp part of that log is `d1 list --json`. The launch guard
//! issues it to prove the account's `cabin-registry` is the database
//! the config binds, and the wipe issues the identical argv again after
//! `d1 create` to learn the recreated id. One argv, two answers: the
//! fake `npx` flips a phase on `d1 create`, so the same call reports
//! [`OLD_ID`] before the recreate and [`NEW_ID`] after it. A port that
//! cached the first answer would skip a call and fail parity; one that
//! reused the old id would fail the `wrangler.jsonc` comparison.
//!
//! **`cargo`**, which is the launch-guard hop and the one deviation
//! this suite documents rather than compares. The shell runs
//! `(cd .. && cargo run --quiet --locked -p xtask-registry-admin -- \
//! launch-guard "$mode")`; the port calls the guard as a function,
//! because the hop closes exactly when this script becomes Rust
//! (`src/launch_guard.rs`, "One thing it is worse at"). The shim execs
//! the already-built binary with everything after the `--`, so the
//! guard that runs is the TRUE guard, reaching the same fake `npx`
//! through the same `PATH` - both sides then run the same guard through
//! the same seams and its own wrangler calls appear in BOTH logs and
//! must match. Only the one `[cargo]` record differs, and [`diff`]
//! filters exactly that leading field, asserting one such line on the
//! shell and none on the port rather than dropping it silently.
//!
//! The shim deliberately leaves the cwd alone. The subshell's `cd ..`
//! puts the guard one level ABOVE the registry root, which is the
//! point: the guard must find its `wrangler.jsonc` and its wrangler
//! working directory through [`ROOT_VARIABLE`], never through wherever
//! it was launched. A guard reading the cwd reaches the parent and is
//! caught by the `[root]` field.
//!
//! **`curl`**, which is how the R2 sweep runs. `wrangler r2 object` has
//! no list or bulk mode, so the script builds
//! `https://api.cloudflare.com/...` itself - a URL nothing but a PATH
//! stand-in can move. The shim rewrites only the origin, to
//! `$FAKE_R2_BASE`, and execs the real `curl`, so `-f`'s exit 22 and
//! its stderr wording stay the machine's own. The port reaches the same
//! server through [`R2_BASE_VARIABLE`] and its own HTTP client.
//!
//! # The sweep's parity oracle is the server, not the argv
//!
//! `curl` argv lines and the port's HTTP calls are not comparable one
//! to one - different clients, different flag surfaces - so [`diff`]
//! filters `[curl]` records out of the command parity. What IS compared
//! is what R2 was actually asked, from the other end: each side's mock
//! records `<METHOD> <path>?<query>` for every request, and the two
//! logs must be identical. That pins the whole sweep - the
//! `prefix=blobs/&per_page=500` query as literal text, the re-fetch
//! loop that drains the listing rather than paging a cursor, and the
//! delete URL for every key.
//!
//! The keys are chosen for that last one. The script encodes with
//! `obj.key.split("/").map(encodeURIComponent).join("/")`: slashes stay
//! LITERAL because the API requires it, and every other component
//! character is percent-encoded. So the corpus carries a key with
//! spaces, one with `+` and a literal `%`, one non-ASCII, and one
//! nested several segments deep. The mock decodes per segment and 404s
//! a key it does not hold, which `curl -f` turns into a failed run - so
//! an encoding that does not round-trip fails loudly rather than
//! deleting nothing quietly.
//!
//! The mock's page is capped at [`PAGE`] objects regardless of the
//! `per_page` asked for, which R2 is free to do. That is what makes a
//! multi-page drain affordable: five objects drain over three listings
//! and a fourth that comes back empty, where honoring `per_page=500`
//! would need 500 objects to exercise the loop at all.
//!
//! `node` is not stubbed. The script runs four `node -e` projections
//! and the `wrangler.jsonc` rewrite through it, so the canned answers
//! are real wrangler-shaped JSON and the shell really parses them.
//! `shasum`, `grep` and `cut` are likewise the machine's own.
//!
//! # What is compared, and where the comparison stops
//!
//! stdout is compared as bytes everywhere: it carries the step lines,
//! the confirmation prompt, the deleted-blob count, the final
//! generation line, the follow-ups heredoc, and whatever wrangler
//! itself printed through the descriptors the script never redirected -
//! `d1 delete`'s output is inherited where `d1 create`'s is sent to
//! `/dev/null`, and a port that captured either would diverge. The exit
//! status is compared exactly everywhere; the script chooses every one
//! of them.
//!
//! stderr is compared byte for byte wherever the script is the sole
//! writer, which is every refusal it reaches through `fail`. Three
//! scenarios narrow:
//!
//! - [`the_recreated_database_must_appear_in_the_listing`], where the
//!   shell's stderr also carries `node`'s own exit-1 noise, and
//!   [`a_failed_listing_stops_the_sweep_before_the_deploy`], where
//!   `curl` comments on the 500 in its own wording. Only the script's
//!   `FAIL:` line is compared.
//! - [`an_absent_api_token_stops_before_the_database_is_dropped`],
//!   where `${CLOUDFLARE_API_TOKEN:?...}` makes bash itself write
//!   `<path>: line 112: CLOUDFLARE_API_TOKEN: ...`, naming the
//!   fixture's own path and line number - not something a port can
//!   reproduce, and not something it should.
//! - [`an_unknown_argument_refuses_before_anything_runs`], whose
//!   `usage: scripts/wipe.sh [--local]` names a script that no longer
//!   exists once the command is `cargo registry-wipe`.
//!
//! The last two are compared as refusal semantics - each side refused,
//! said something, ran nothing, destroyed nothing - with the shell's
//! exact texts pinned beside the assertion so the move is visible
//! rather than silent.
//!
//! # Two arguments this suite deliberately does NOT compare
//!
//! The port refuses two inputs the shell accepted, and both are
//! fail-closed - the port refuses where the shell WIPED, which is the
//! only direction an argument surface guarding an `rm -rf` may differ
//! in. A parity scenario for either would assert the divergence away,
//! so they are named here and left to the port's own unit tests:
//!
//! - an EMPTY first argument. `[[ -n "${1:-}" ]]` is false for `""`,
//!   so the shell fell through to its `--remote` default and wiped the
//!   deployed registry. An empty argument is far likelier to be an
//!   unset variable that expanded to nothing than a considered request
//!   to wipe production.
//! - a SECOND argument. The shell read `$1` and ignored the rest, so
//!   `--local --remote` ran in local mode - and, read the other way
//!   round, `--remote --local` wiped the deployed registry while
//!   naming `--local`. That is the same ceiling the launch guard
//!   already states for its own mode argument.
//!
//! # Why `LC_ALL=C`
//!
//! Both sides run under it, mirroring the migrate differential. The
//! `migrations/*.sql` glob fixes the concatenation order the refreshed
//! stamp hashes, and the corpus pins that order to byte order.
//!
//! # Not covered here, and why
//!
//! - **Whether wrangler and R2 really answer the way the corpus says.**
//!   Both sides are handed the same stand-ins by construction, so a
//!   wrong response shape is one both sides get equally wrong. What
//!   this suite covers is that the two *ask* the same things and read
//!   the answers the same way.
//! - **A real `cargo run` of the guard.** The shim execs the built
//!   binary instead, so what is exercised is the guard, not Cargo's
//!   ability to build it. `--quiet --locked -p xtask-registry-admin` is
//!   in the `[cargo]` record either way.
//! - **An interactive terminal.** The confirmation is fed from a file
//!   on both sides. A tty would change nothing the script observes.
//! - **A cursor-paginated listing.** The script deliberately does not
//!   page: deleting drains the listing, so it re-fetches the first page
//!   until nothing matches. The mock's repeated first pages are that
//!   behavior, and a port that started following cursors would show up
//!   as an extra query parameter in the server log.
//!
//! The suite is Unix-only outright. The original is a bash script whose
//! tools are matched by name; a Windows host's lookalikes EXIST on
//! `PATH` and would pass a presence check while meaning something else.
//! Every test skips rather than fails when a tool it needs is missing,
//! and the harness's own failures panic.
//!
//! # Negative proofs
//!
//! Both were run by hand against the port, from a green suite, then
//! reverted, with both fixtures' `sha256` re-checked afterwards.
//!
//! - **The left state is load-bearing, on its own.** Seeding the PORT
//!   side's mock with one extra object the sweep must NOT touch -
//!   `index/extra.json`, outside the `blobs/` prefix - is a port
//!   pointed at a different bucket than the shell's, and nothing else
//!   about the run changes: the same keys are listed, the same keys
//!   are deleted, and stdout, the command log and the server request
//!   log all still match byte for byte. It failed 12 of the 13, every
//!   one of them on `the objects each run left in R2` and on nothing
//!   else. The survivor is [`the_real_registry_is_never_touched`],
//!   which runs neither side. So a divergence visible ONLY in what
//!   survived is caught, which is the shape a wipe gets wrong.
//! - **The `[cargo]` filter hides exactly one line, and the command
//!   parity catches a guard that did not run.** Removing the filter
//!   failed 10 of the 13, each naming the `[cargo]` record as the
//!   offending line - so the filter masks that one record and nothing
//!   else. Separately, dropping the guard's own two `npx` calls from
//!   the port side alone, which is what a port that skipped the guard
//!   would produce, failed the same 10 on `the two sides ran different
//!   commands`. The three survivors of both are the three that never
//!   reach the guard: the argument surface, the declined confirmation,
//!   and the harness's own check. A skipped guard is therefore caught
//!   by the command log rather than only by whatever it would have
//!   refused - which matters because the guard's refusal is the last
//!   thing between a launched registry and an `rm -rf`.
//!
//! Separately, the harness was validated without the port at all, by
//! pointing [`Side::Port`] at the fixture as well and running the
//! corpus shell against shell. That passed 13 of 13, which is what
//! establishes that the expected byte strings throughout this file -
//! the follow-ups heredoc, the percent-encoded delete paths, the
//! refusal texts - are the shell's real output rather than a
//! transcription of it, and that no state leaks between the two sides.
#![cfg(unix)]

use std::fs::{self, File};
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use assert_fs::TempDir;
use sha2::{Digest as _, Sha256};

/// Points the port at a scenario's synthetic registry root instead of
/// this checkout's own `registry/`. The shell needs no equivalent: it
/// derives its root from its own path, and the harness copies it into
/// the root it is meant to operate on.
const ROOT_VARIABLE: &str = "CABIN_REGISTRY_DIR";

/// Points the port's R2 sweep at the scenario's mock. The shell reaches
/// the same server through the `curl` shim's origin rewrite, which is
/// the only seam a URL built inside the script has.
///
/// Not a new name: this is the variable the Worker
/// (`registry/src/backup_glue.rs`) and the smoke run
/// (`xtask-registry-smoke`) already point Cloudflare API calls at a
/// local server with.
const R2_BASE_VARIABLE: &str = "CF_API_BASE";

/// The account the corpus's `wrangler.jsonc` declares, in the shape
/// `declared_account_id` matches: 32 lower-case hex digits.
const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

/// The database the config binds before the wipe, and what the
/// account's `cabin-registry` answers to while the launch guard is
/// proving the two agree.
const OLD_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The recreated database's id, which `d1 list` reports only after
/// `d1 create` has flipped the fake account's phase.
const NEW_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The bucket the sweep drains.
const BLOBS: &str = "cabin-registry-blobs";

/// The bucket the sweep must never reach. Its `blobs/` namespace is
/// append-only, and it holds a `blobs/`-prefixed object here precisely
/// so that a sweep filtering on the prefix alone - and not on the
/// bucket - would delete it.
const BACKUP: &str = "cabin-registry-backup";

/// How many objects the mock returns per listing, whatever `per_page`
/// asked for. R2 is free to answer with fewer, and this is what makes a
/// multi-page drain cost five objects rather than five hundred.
const PAGE: usize = 2;

/// The blobs the sweep is meant to delete, chosen for the encoding rule
/// the script applies to each: slashes stay literal, everything else in
/// a path component is percent-encoded.
const SWEPT: [&str; 5] = [
    "blobs/sha256/aa/bb/ccddeeff.zip",
    "blobs/a name with spaces.zip",
    "blobs/plus+and%literal.zip",
    "blobs/caf\u{e9}-non-ascii.zip",
    "blobs/nested/deeper/still/x.zip",
];

/// What the corpus writes into `migrations-applied` before a run.
/// Distinctive on purpose: it is not a hash of anything, so a side that
/// echoed it back would be visible.
const STALE: &str = "0000000000000000000000000000000000000000000000000000000000000000\n";

/// A migration file: its name, and the bytes that go into the stamp.
type Migration = (&'static str, &'static str);

const MIGRATIONS: [Migration; 2] = [
    ("0001_init.sql", "CREATE TABLE alpha (id INTEGER);\n"),
    ("0002_more.sql", "CREATE TABLE beta (id INTEGER);\n"),
];

/// The tools every scenario drives, on top of the port itself.
const TOOLS: [&str; 6] = ["bash", "node", "shasum", "grep", "curl", "tr"];

/// What the fake wrangler prints where the script left a descriptor
/// alone. Each reaches stdout and is part of the compared bytes; a port
/// that captured one would diverge. The wording is arbitrary and tracks
/// nothing - what is tested is that it survives byte for byte,
/// non-ASCII included.
const DROPPED: &str = "\u{1f5d1}  Deleted cabin-registry.\n";
const APPLIED: &str = "\u{1f300} Executing on remote database DB: 2 commands\n";
const BUMPED: &str = "\u{1f300} Executing on remote database DB: 1 command\n";
const DEPLOYED: &str = "Uploaded cabin-registry (1.23 sec)\n";

/// What `d1 create` prints. The script sends it to `/dev/null`, so this
/// string appearing in either side's stdout is a port that stopped
/// redirecting.
const CREATED: &str = "created cabin-registry, THIS LINE IS REDIRECTED\n";

/// The corpus's `wrangler.jsonc`. Both id sites carry [`OLD_ID`], which
/// is what makes the post-wipe `grep -c "$new_id"` expect exactly two.
const CONFIG: &str = r#"{
  "name": "cabin-registry",
  "main": "src/lib.rs",
  "compatibility_date": "2025-06-01",
  // Not a secret: the account id is in every dashboard URL.
  "vars": {
    "CF_ACCOUNT_ID": "0123456789abcdef0123456789abcdef",
    "D1_DATABASE_ID": "11111111-2222-3333-4444-555555555555"
  },
  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "cabin-registry",
      "database_id": "11111111-2222-3333-4444-555555555555"
    }
  ],
  "r2_buckets": [
    { "binding": "BLOBS", "bucket_name": "cabin-registry-blobs" }
  ]
}
"#;

/// The same config carrying a second D1 binding. The DB binding stays
/// first, so the launch guard still cross-checks the right database and
/// the run reaches the `grep -c '"database_id"'` refusal rather than
/// stopping earlier.
const TWO_BINDINGS: &str = r#"{
  "name": "cabin-registry",
  "main": "src/lib.rs",
  "compatibility_date": "2025-06-01",
  "vars": {
    "CF_ACCOUNT_ID": "0123456789abcdef0123456789abcdef",
    "D1_DATABASE_ID": "11111111-2222-3333-4444-555555555555"
  },
  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "cabin-registry",
      "database_id": "11111111-2222-3333-4444-555555555555"
    },
    {
      "binding": "SHADOW",
      "database_name": "cabin-shadow",
      "database_id": "99999999-8888-7777-6666-000000000000"
    }
  ]
}
"#;

/// The emulated state `--local` deletes, and the two decoys it must
/// leave alone. `kv` is a sibling state directory the script does not
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

/// How far stderr can be compared.
enum Diagnostics<'a> {
    /// The script was the only writer: compare byte for byte.
    Quiet,
    /// `node` or `curl` also wrote. Assert both sides emitted each of
    /// these as a whole line and leave the rest to the ceiling.
    Lines(&'a [&'a str]),
    /// bash named the fixture's own path, or the script named a script
    /// the port is not. Assert both refused and both said something.
    Refused,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// Every shelled-out call the side made, in order, as the three
    /// shims recorded them.
    log: Vec<String>,
    /// `wrangler.jsonc` as the run left it: the one file a remote wipe
    /// rewrites and the operator is told to commit.
    config: Vec<u8>,
    /// `migrations-applied` as the run left it.
    stamp: Vec<u8>,
    /// Every path under `.wrangler/`, relative and sorted.
    tree: Vec<String>,
    /// `<METHOD> <path>?<query>` for every request the side made of R2.
    requests: Vec<String>,
    /// `<bucket>\t<key>` for every object that survived, sorted.
    objects: Vec<String>,
}

impl Outcome {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    fn configured(&self) -> String {
        String::from_utf8_lossy(&self.config).into_owned()
    }

    fn stamped(&self) -> String {
        String::from_utf8_lossy(&self.stamp).into_owned()
    }

    /// The `npx` records alone: the calls both sides must make
    /// identically. Arguments are tab separated, so a fragment spanning
    /// two of them spells the tab.
    fn wrangler(&self) -> Vec<&String> {
        self.log
            .iter()
            .filter(|call| !call.starts_with("[cargo]\t") && !call.starts_with("[curl]\t"))
            .collect()
    }

    /// How many `npx` calls carried `fragment`.
    fn commands(&self, fragment: &str) -> usize {
        self.wrangler()
            .iter()
            .filter(|call| call.contains(fragment))
            .count()
    }

    fn shims(&self, kind: &str) -> usize {
        self.log
            .iter()
            .filter(|call| call.starts_with(&format!("[{kind}]\t")))
            .count()
    }
}

/// One canned R2 account: what each bucket holds, what it was asked
/// for, and which listing it is told to fail.
///
/// One per side. The sweep DELETES, so a shared mock would let the
/// shell's run drain the objects the port was meant to delete, and the
/// port would then pass by finding an empty bucket.
struct R2 {
    base: String,
    state: Arc<Mutex<Vec<(String, String)>>>,
    log: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl R2 {
    /// Every failure here is the harness failing, not a tool missing,
    /// so every one of them panics.
    fn start(objects: &[(&str, &str)], fail_listing: usize) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("the R2 mock could not bind");
        let port = server
            .server_addr()
            .to_ip()
            .expect("the R2 mock bound something that is not an ip")
            .port();
        let state = Arc::new(Mutex::new(
            objects
                .iter()
                .map(|(bucket, key)| ((*bucket).to_owned(), (*key).to_owned()))
                .collect::<Vec<_>>(),
        ));
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let held = Arc::clone(&state);
        let recorded = Arc::clone(&log);
        let halt = Arc::clone(&stop);
        thread::spawn(move || {
            let mut listings = 0_usize;
            while !halt.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(25)) else {
                    continue;
                };
                let method = request.method().as_str().to_owned();
                let url = request.url().to_owned();
                recorded
                    .lock()
                    .expect("the R2 log is not poisoned")
                    .push(format!("{method} {url}"));
                let (status, body) = answer(&held, &method, &url, &mut listings, fail_listing);
                let length = Some(body.len());
                request
                    .respond(tiny_http::Response::new(
                        tiny_http::StatusCode(status),
                        Vec::new(),
                        Cursor::new(body),
                        length,
                        None,
                    ))
                    .ok();
            }
        });

        Self {
            base: format!("http://127.0.0.1:{port}/client/v4"),
            state,
            log,
            stop,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().expect("the R2 log is not poisoned").clone()
    }

    /// Every surviving object, sorted, as `<bucket>\t<key>`.
    fn objects(&self) -> Vec<String> {
        let mut held: Vec<String> = self
            .state
            .lock()
            .expect("the R2 state is not poisoned")
            .iter()
            .map(|(bucket, key)| format!("{bucket}\t{key}"))
            .collect();
        held.sort();
        held
    }
}

impl Drop for R2 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One request against the canned account.
fn answer(
    state: &Mutex<Vec<(String, String)>>,
    method: &str,
    url: &str,
    listings: &mut usize,
    fail_listing: usize,
) -> (u16, Vec<u8>) {
    let refused = |message: &str| {
        (
            404,
            serde_json::json!({ "success": false, "errors": [{ "message": message }] })
                .to_string()
                .into_bytes(),
        )
    };
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let Some(rest) = path.strip_prefix(&format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/"))
    else {
        return refused("not an R2 objects route");
    };
    let Some((bucket, tail)) = rest.split_once("/objects") else {
        return refused("not an R2 objects route");
    };

    if method == "GET" && tail.is_empty() {
        *listings += 1;
        if *listings == fail_listing {
            return (
                500,
                serde_json::json!({ "success": false, "errors": [{ "code": 10001 }] })
                    .to_string()
                    .into_bytes(),
            );
        }
        let prefix = parameter(query, "prefix").unwrap_or_default();
        let per_page = parameter(query, "per_page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(PAGE);
        let held = state.lock().expect("the R2 state is not poisoned");
        let mut keys: Vec<&str> = held
            .iter()
            .filter(|(held, key)| held == bucket && key.starts_with(&prefix))
            .map(|(_, key)| key.as_str())
            .collect();
        keys.sort_unstable();
        keys.truncate(per_page.min(PAGE));
        let result: Vec<serde_json::Value> = keys
            .into_iter()
            .map(|key| serde_json::json!({ "key": key }))
            .collect();
        return (
            200,
            serde_json::json!({ "success": true, "errors": [], "result": result })
                .to_string()
                .into_bytes(),
        );
    }

    if method == "DELETE" && !tail.is_empty() {
        // The mirror of the script's
        // `key.split("/").map(encodeURIComponent).join("/")`: slashes
        // are structure, everything else is an encoded component.
        let key = tail
            .trim_start_matches('/')
            .split('/')
            .map(decode)
            .collect::<Vec<_>>()
            .join("/");
        let mut held = state.lock().expect("the R2 state is not poisoned");
        let before = held.len();
        held.retain(|(held, existing)| !(held == bucket && *existing == key));
        if held.len() == before {
            // Not a no-op: `curl -f` turns this into a failed run, so
            // an encoding that does not round-trip is loud rather than
            // quietly deleting nothing.
            return refused("no such object");
        }
        return (
            200,
            serde_json::json!({ "success": true, "errors": [] })
                .to_string()
                .into_bytes(),
        );
    }

    refused("unsupported method")
}

/// One query parameter's value, percent-decoded.
fn parameter(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|field| field.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode(value))
}

/// Percent-decoding, over bytes rather than characters, because a
/// non-ASCII key arrives as several `%XX` escapes that only mean
/// something reassembled.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%' && index + 3 <= bytes.len())
            .then(|| u8::from_str_radix(&text[index + 1..index + 3], 16).ok())
            .flatten();
        if let Some(byte) = escape {
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// One scenario: the registry root to materialize, the canned answers
/// the fake wrangler and the mock R2 serve, and how the run is invoked.
struct World {
    /// The argument the run is given. `None` is the shell's default,
    /// which is remote.
    mode: Option<&'static str>,
    /// `CABIN_WIPE_YES=1`.
    confirmed: bool,
    /// What `read -r answer` is fed.
    stdin: &'static str,
    /// The bytes `wrangler.jsonc` starts with.
    config: &'static str,
    /// `(<kind>[.<phase>], exit status, stdout)`.
    responses: Vec<(&'static str, i32, String)>,
    /// `CLOUDFLARE_API_TOKEN`, or the variable left unset.
    token: Option<&'static str>,
    /// What each side's mock holds before the run.
    objects: Vec<(&'static str, &'static str)>,
    /// Which listing (1-based) answers 500. Zero never fails.
    fail_listing: usize,
}

impl World {
    /// A confirmed remote wipe of a registry that is not launched,
    /// whose generation is 7 and whose blobs bucket holds [`SWEPT`]
    /// beside objects the sweep must leave alone. Scenarios override
    /// the fields they are about.
    fn remote() -> Self {
        let mut objects: Vec<(&'static str, &'static str)> = SWEPT
            .iter()
            .map(|key| (BLOBS, *key))
            .chain([
                // Same bucket, outside the prefix.
                (BLOBS, "index/cabin-core.json"),
                // The append-only bucket, holding a `blobs/`-prefixed
                // object: a sweep matching the prefix without the
                // bucket deletes this.
                (BACKUP, "blobs/sha256/aa/bb/ccddeeff.zip"),
            ])
            .collect();
        objects.sort_unstable();
        Self {
            mode: None,
            confirmed: true,
            stdin: "",
            config: CONFIG,
            responses: vec![
                ("launched", 0, value("false")),
                ("list.before", 0, listing(OLD_ID, None)),
                ("generation", 0, value("7")),
                ("drop", 0, DROPPED.to_owned()),
                ("create", 0, CREATED.to_owned()),
                ("list.after", 0, listing(NEW_ID, None)),
                ("apply", 0, APPLIED.to_owned()),
                ("bump", 0, BUMPED.to_owned()),
                ("deploy", 0, DEPLOYED.to_owned()),
            ],
            token: Some("test-token"),
            objects,
            fail_listing: 0,
        }
    }

    /// A confirmed local wipe, which reads no ids, sweeps nothing and
    /// deploys nothing.
    fn local() -> Self {
        Self {
            mode: Some("--local"),
            responses: vec![
                ("launched", 0, value("false")),
                ("generation", 0, value("7")),
                ("apply", 0, APPLIED.to_owned()),
                ("bump", 0, BUMPED.to_owned()),
            ],
            ..Self::remote()
        }
    }

    /// Replaces the canned answer for `kind`, or adds it.
    fn respond(&mut self, kind: &'static str, status: i32, body: String) {
        self.responses.retain(|(name, _, _)| *name != kind);
        self.responses.push((kind, status, body));
    }

    /// Runs both sides over their own copies of the same root, each
    /// with its own command log and its own R2 mock.
    fn both(&self) -> (Outcome, Outcome) {
        for shim in ["npx", "curl", "cargo"] {
            let path = wipe_bin().join(shim);
            let mode = fs::metadata(&path)
                .unwrap_or_else(|_| panic!("the fake {shim}"))
                .permissions()
                .mode();
            assert!(
                mode & 0o111 != 0,
                "{} lost its executable bit, so PATH would find the real {shim}",
                show(&path)
            );
        }

        let before = real_registry();
        let shell = self.side(Side::Shell);
        let port = self.side(Side::Port);
        // Restored before the panic, not merely detected: a port still
        // reading `registry_dir()` rewrites the repository's own
        // config, and a suite that left that behind would have edited
        // the checkout it was run in.
        let after = real_registry();
        if after != before {
            for (path, bytes) in &before {
                fs::write(path, bytes).expect("restoring this checkout's registry");
            }
            panic!(
                "a side rewrote this checkout's own registry/ (restored): the port is \
                 reading `registry_dir()` rather than {ROOT_VARIABLE}"
            );
        }
        (shell, port)
    }

    /// Materializes one side's registry root and the canned answers
    /// beside it.
    fn plant(&self, root: &Path, responses: &Path) {
        let migrations = root.join("migrations");
        let scripts = root.join("scripts");
        for made in [&migrations, &scripts, &responses.to_path_buf()] {
            fs::create_dir_all(made).expect("a directory of the scenario's root");
        }

        for (name, body) in &MIGRATIONS {
            fs::write(migrations.join(name), body).expect("a migration file");
        }
        fs::write(root.join("wrangler.jsonc"), self.config).expect("the scenario's wrangler.jsonc");
        fs::write(root.join("migrations-applied"), STALE).expect("the stamp file");
        // What the fake wrangler recognizes this root by. A dotfile, so
        // the `migrations/*.sql` glob never sees it.
        fs::write(root.join(".differential-root"), b"").expect("the root marker");
        for (name, source) in [("wipe.sh", "wipe.sh"), ("lib.sh", "migrate-lib.sh")] {
            let vendored = fixtures().join(format!("{source}.orig"));
            fs::copy(&vendored, scripts.join(name)).expect("the vendored script");
        }
        for (path, _) in EMULATED {
            let file = root.join(".wrangler").join(path);
            fs::create_dir_all(file.parent().expect("an emulated state directory"))
                .expect("an emulated state directory");
            fs::write(&file, b"emulated").expect("an emulated state file");
        }

        fs::write(responses.join("phase"), "before").expect("the fake account's phase");
        for (name, status, body) in &self.responses {
            fs::write(responses.join(name), format!("{status}\n{body}"))
                .expect("a canned wrangler answer");
        }
    }

    fn side(&self, side: Side) -> Outcome {
        let dir = TempDir::new().expect("a scratch directory");
        // A level below the scratch root, because the script's guard
        // hop runs `cd ..` and must land somewhere real.
        let root = dir.path().join("checkout/registry");
        let scripts = root.join("scripts");
        let responses = dir.path().join("responses");
        self.plant(&root, &responses);
        let config = root.join("wrangler.jsonc");
        let stamp = root.join("migrations-applied");

        let r2 = R2::start(&self.objects, self.fail_listing);

        let answers = dir.path().join("stdin");
        fs::write(&answers, self.stdin).expect("the confirmation's answers");
        let log = dir.path().join("commands");
        fs::write(&log, b"").expect("the command log");

        let mut command = match side {
            Side::Shell => {
                let mut bash = Command::new("bash");
                bash.arg(scripts.join("wipe.sh"));
                bash
            }
            Side::Port => {
                let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-registry-admin"));
                ported.arg("wipe");
                ported
            }
        };
        if let Some(mode) = self.mode {
            command.arg(mode);
        }
        command
            // Neither side is started inside the root: the shell `cd`s
            // there from its own path and the port resolves it from
            // the variable, so a side that leaned on the cwd is caught
            // by the `[root]` field rather than passing by accident.
            .current_dir(dir.path())
            .env(ROOT_VARIABLE, &root)
            // The port's seam and the `curl` shim's, pointed at the
            // same mock: one canned account, reached two ways.
            .env(R2_BASE_VARIABLE, &r2.base)
            .env("FAKE_R2_BASE", &r2.base)
            .env("PATH", path_through_the_shims())
            .env("FAKE_NPX_LOG", &log)
            .env("FAKE_NPX_DIR", &responses)
            .env("REAL_CURL", real("curl"))
            .env("REAL_ADMIN_BIN", env!("CARGO_BIN_EXE_xtask-registry-admin"))
            // The `migrations/*.sql` glob order the refreshed stamp
            // hashes is pinned to byte order.
            .env("LC_ALL", "C")
            .stdin(Stdio::from(
                File::open(&answers).expect("the confirmation's answers"),
            ));
        match self.token {
            Some(token) => command.env("CLOUDFLARE_API_TOKEN", token),
            None => command.env_remove("CLOUDFLARE_API_TOKEN"),
        };
        if self.confirmed {
            command.env("CABIN_WIPE_YES", "1");
        } else {
            command.env_remove("CABIN_WIPE_YES");
        }

        let produced: Output = command.output().expect("running one side of the scenario");
        Outcome {
            stdout: produced.stdout,
            stderr: produced.stderr,
            status: produced.status.code(),
            log: fs::read_to_string(&log)
                .expect("the command log")
                .lines()
                .map(str::to_owned)
                .collect(),
            config: fs::read(&config).expect("the wrangler.jsonc the run left behind"),
            stamp: fs::read(&stamp).expect("the stamp file the run left behind"),
            tree: tree(&root.join(".wrangler")),
            requests: r2.requests(),
            objects: r2.objects(),
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Shell,
    Port,
}

/// Every path under `root`, relative and sorted, directories included -
/// a `--local` wipe removes directories, and "the tree the run left" is
/// what says which.
fn tree(root: &Path) -> Vec<String> {
    fn walk(base: &Path, current: &Path, found: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            found.push(
                path.strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                walk(base, &path, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// `SELECT value FROM meta WHERE key = ...` in wrangler's `--json`
/// shape, which the script's `node` reads as `out[0].results[0].value`.
fn value(held: &str) -> String {
    format!("[{{\"results\":[{{\"value\":\"{held}\"}}],\"success\":true,\"meta\":{{}}}}]\n")
}

/// `wrangler d1 list --json`, which answers a bare array. `uuid` is
/// what the script prefers; `also` fills `database_id` beside it.
fn listing(uuid: &str, also: Option<&str>) -> String {
    let mut entry = serde_json::json!({ "name": "cabin-registry", "uuid": uuid });
    if let Some(id) = also {
        entry["database_id"] = serde_json::Value::String(id.to_owned());
    }
    format!(
        "{}\n",
        serde_json::json!([entry, { "name": "cabin-other", "uuid": OLD_ID }])
    )
}

/// The stamp the script writes: `sha256` of `migrations/*.sql`
/// concatenated in glob order, as
/// `cat migrations/*.sql | shasum -a 256 | cut -d' ' -f1` computes it.
/// Recomputed here rather than read off either side, so a scenario
/// asserting a stamp is asserting the rule and not one
/// implementation's answer.
fn digest() -> String {
    let mut hasher = Sha256::new();
    for (_, body) in &MIGRATIONS {
        hasher.update(body.as_bytes());
    }
    cabin_core::hash::hex_digest(&hasher.finalize())
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn wipe_bin() -> PathBuf {
    fixtures().join("wipe-bin")
}

/// This checkout's own `registry/`, which no scenario may write.
fn real_registry() -> Vec<(PathBuf, Vec<u8>)> {
    ["wrangler.jsonc", "migrations-applied"]
        .into_iter()
        .map(|name| {
            let path = xtask_registry_admin::registry_dir().join(name);
            let bytes = fs::read(&path)
                .unwrap_or_else(|_| panic!("this checkout's registry/{name}"))
                .clone();
            (path, bytes)
        })
        .collect()
}

fn path_through_the_shims() -> std::ffi::OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = vec![wipe_bin()];
    directories.extend(std::env::split_paths(&inherited));
    std::env::join_paths(directories).expect("a PATH with the shims first")
}

/// The real `tool`, resolved against the PATH the shims are prepended
/// to, so a shim's `exec` cannot land back on itself.
fn real(tool: &str) -> String {
    let found = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .unwrap_or_else(|_| panic!("looking for {tool}"));
    String::from_utf8_lossy(&found.stdout).trim().to_owned()
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn ready(test: &str) -> bool {
    for tool in TOOLS {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, diagnostics: &Diagnostics) {
    // First, because a scenario missing a canned answer would otherwise
    // report as whatever the two sides did about it.
    for (side, outcome) in [("shell", shell), ("port", port)] {
        assert!(
            !outcome.err().contains("fake npx:"),
            "{case}: the {side}'s fake npx refused a call: {}",
            outcome.err()
        );
    }
    // Before the sequences are compared against each other, because two
    // sides that both ran in the wrong tree would compare equal.
    for (side, outcome) in [("shell", shell), ("port", port)] {
        for call in outcome.wrangler() {
            assert!(
                call.starts_with("[root]\t"),
                "{case}: the {side} ran wrangler outside the scenario's registry root, \
                 where a different wrangler.jsonc binds a different database: {call}"
            );
        }
    }
    // The launch-guard hop, which is the deviation this suite carries
    // rather than compares: the shell spawns the guard through `cargo
    // run`, the port calls it as a function. Asserted on both sides
    // rather than filtered silently - the guard's own wrangler calls
    // stay in the compared sequence either way.
    assert_eq!(
        port.shims("cargo"),
        0,
        "{case}: the port ran a `cargo`, so the guard is still a separate process \
         rather than a function call: {:#?}",
        port.log
    );
    assert!(
        shell.shims("cargo") <= 1,
        "{case}: the shell ran more than one `cargo`: {:#?}",
        shell.log
    );
    assert!(
        shell.wrangler() == port.wrangler(),
        "{case}: the two sides ran different commands\nshell: {:#?}\nport:  {:#?}",
        shell.wrangler(),
        port.wrangler()
    );
    assert!(
        shell.requests == port.requests,
        "{case}: the two sides asked R2 for different things\nshell: {:#?}\nport:  {:#?}",
        shell.requests,
        port.requests
    );
    assert!(
        shell.stdout == port.stdout,
        "{case}: stdout\nshell: {}\nport:  {}",
        shell.stdout.escape_ascii(),
        port.stdout.escape_ascii()
    );
    assert_eq!(shell.status, port.status, "{case}: exit status");
    assert!(
        shell.config == port.config,
        "{case}: the wrangler.jsonc each run left behind\nshell: {}\nport:  {}",
        shell.config.escape_ascii(),
        port.config.escape_ascii()
    );
    assert!(
        shell.stamp == port.stamp,
        "{case}: the stamp each run left behind\nshell: {}\nport:  {}",
        shell.stamp.escape_ascii(),
        port.stamp.escape_ascii()
    );
    assert_eq!(
        shell.tree, port.tree,
        "{case}: the .wrangler tree each run left behind"
    );
    assert_eq!(
        shell.objects, port.objects,
        "{case}: the objects each run left in R2"
    );
    match *diagnostics {
        Diagnostics::Quiet => {
            assert!(
                shell.stderr == port.stderr,
                "{case}: stderr\nshell: {}\nport:  {}",
                shell.stderr.escape_ascii(),
                port.stderr.escape_ascii()
            );
        }
        Diagnostics::Lines(lines) => {
            for line in lines {
                for (side, text) in [("shell", &shell.err()), ("port", &port.err())] {
                    assert!(
                        text.lines().any(|emitted| emitted == *line),
                        "{case}: {side} stderr is missing `{line}`, got:\n{text}"
                    );
                }
            }
        }
        Diagnostics::Refused => {
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert!(
                    !outcome.stderr.is_empty(),
                    "{case}: the {side} refused without saying why"
                );
            }
        }
    }
}

/// The paths in `paths` plus every directory on the way to them, which
/// is the shape [`tree`] reports.
fn expanded(paths: impl Iterator<Item = &'static str>) -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    for path in paths {
        let mut current = Path::new(path);
        loop {
            found.insert(current.to_string_lossy().into_owned());
            match current.parent() {
                Some(parent) if parent != Path::new("") => current = parent,
                _ => break,
            }
        }
    }
    found.into_iter().collect()
}

/// The objects a remote wipe is supposed to leave: the append-only
/// backup bucket untouched, and the blobs bucket holding only what the
/// `blobs/` prefix never covered.
fn survivors() -> Vec<String> {
    vec![
        format!("{BACKUP}\tblobs/sha256/aa/bb/ccddeeff.zip"),
        format!("{BLOBS}\tindex/cabin-core.json"),
    ]
}

/// A complete sweep of the default corpus, read off the mock: three
/// listings that returned keys, a fourth that came back empty and ended
/// the loop, and one DELETE per key.
///
/// The delete paths are the encoding rule spelled out. Every one of
/// these was produced by the shell itself - `encodeURIComponent` per
/// component, slashes left alone - so a port matching them is matching
/// the original rather than a restatement of it.
fn drained(outcome: &Outcome) {
    let objects = format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BLOBS}/objects");
    assert_eq!(
        outcome
            .requests
            .iter()
            .filter(|line| line.starts_with("GET"))
            .count(),
        4,
        "the drain re-fetches the first page until it is empty: {:#?}",
        outcome.requests
    );
    assert_eq!(
        outcome.requests[0],
        format!("GET {objects}?prefix=blobs/&per_page=500"),
        "the listing query is literal text, `/` included"
    );
    let deletes: Vec<&String> = outcome
        .requests
        .iter()
        .filter(|line| line.starts_with("DELETE"))
        .collect();
    let wanted: Vec<String> = [
        "blobs/a%20name%20with%20spaces.zip",
        "blobs/caf%C3%A9-non-ascii.zip",
        "blobs/nested/deeper/still/x.zip",
        "blobs/plus%2Band%25literal.zip",
        "blobs/sha256/aa/bb/ccddeeff.zip",
    ]
    .into_iter()
    .map(|encoded| format!("DELETE {objects}/{encoded}"))
    .collect();
    for expected in &wanted {
        assert!(
            deletes.contains(&expected),
            "each component is percent-encoded and each slash is not: {expected} is \
             missing from {deletes:#?}"
        );
    }
    assert_eq!(deletes.len(), wanted.len(), "one DELETE per key");
    assert_eq!(
        outcome.objects,
        survivors(),
        "the append-only backup bucket and the non-blobs key survive"
    );
}

/// The whole local procedure: the four named state directories go, the
/// two decoys stay, migrations replay and the generation is bumped. No
/// ids are read, no config is rewritten, nothing is swept and nothing
/// is deployed.
#[test]
fn a_local_wipe_clears_the_emulated_state_and_nothing_else() {
    if !ready("a_local_wipe_clears_the_emulated_state_and_nothing_else") {
        return;
    }
    let world = World::local();

    let (shell, port) = world.both();
    diff("a local wipe", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        format!(
            "==> launch guard\n==> reading the pre-wipe registry generation\n==> deleting the \
             local D1, R2, Durable Object, and cache state\n==> reapplying migrations from \
             zero\n{APPLIED}==> bumping the registry generation to 8\n{BUMPED}local wipe OK \
             (generation 7 -> 8)\n"
        )
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a local wipe is silent");
    assert_eq!(
        shell.configured(),
        CONFIG,
        "a local wipe reads no ids and rewrites no config"
    );
    assert_eq!(
        shell.stamped(),
        STALE,
        "the stamp attests the LIVE database; a local wipe never refreshes it"
    );
    // Spelled out rather than derived, because the decoys are the
    // scenario: `kv` is a sibling state directory the script does not
    // name, and `keep-me.json` proves the removal is four named paths
    // rather than `state/v3` wholesale.
    assert_eq!(
        shell.tree.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "state",
            "state/v3",
            "state/v3/keep-me.json",
            "state/v3/kv",
            "state/v3/kv/miniflare-KVNamespaceObject",
            "state/v3/kv/miniflare-KVNamespaceObject/kv.sqlite",
        ],
        "the four named directories go and the two decoys stay"
    );
    assert!(
        shell.requests.is_empty(),
        "a local wipe never reaches the R2 API: {:?}",
        shell.requests
    );
    assert_eq!(
        shell.objects.len(),
        7,
        "every object survives a local wipe: {:?}",
        shell.objects
    );

    // The account is never consulted, and nothing is deployed.
    for absent in ["d1\tlist", "d1\tdelete", "d1\tcreate", "\tdeploy"] {
        assert_eq!(
            shell.commands(absent),
            0,
            "a local wipe ran `{absent}`: {:#?}",
            shell.wrangler()
        );
    }
    assert_eq!(
        shell.commands("migrations\tapply\tDB\t--local"),
        1,
        "the apply went to the local state: {:#?}",
        shell.wrangler()
    );
    // The guard's read, the generation read, the apply, the bump. The
    // guard's `d1 list` cross-check is remote-only.
    assert_eq!(shell.wrangler().len(), 4, "{:#?}", shell.wrangler());
}

/// The argument surface, whose text is the stated ceiling: the script's
/// `usage: scripts/wipe.sh [--local]` names a script the port is not.
/// What is compared is that both refused, both said something, and
/// neither reached the prompt, the guard or anything destructive.
#[test]
fn an_unknown_argument_refuses_before_anything_runs() {
    if !ready("an_unknown_argument_refuses_before_anything_runs") {
        return;
    }
    for argument in ["--nope", "--Local", "local", "--local=1"] {
        let world = World {
            mode: Some(argument),
            ..World::remote()
        };

        let (shell, port) = world.both();
        diff(
            &format!("the argument {argument:?}"),
            &shell,
            &port,
            &Diagnostics::Refused,
        );
        assert!(
            shell.stdout.is_empty(),
            "{argument:?}: the refusal belongs on stderr"
        );
        assert_eq!(shell.status, Some(1), "{argument:?}");
        assert!(
            shell.log.is_empty(),
            "{argument:?}: nothing was run at all: {:#?}",
            shell.log
        );
        assert_eq!(
            shell.configured(),
            CONFIG,
            "{argument:?}: the config is untouched"
        );
        assert_eq!(shell.objects, port.objects, "{argument:?}");
    }

    // The shell's own wording, pinned rather than compared.
    let wrong = World {
        mode: Some("--nope"),
        ..World::remote()
    };
    let (shell, _) = wrong.both();
    assert_eq!(
        shell.err(),
        "usage: scripts/wipe.sh [--local]\n",
        "the script's own usage line is the whole of its stderr"
    );
}

/// The confirmation is the outermost gate, and it sits BEFORE the
/// launch guard on purpose (the script's own comment: a flag flipped
/// while the prompt waited must still refuse). So a declined run leaves
/// the command log completely empty - not even the guard ran.
#[test]
fn a_declined_confirmation_stops_before_the_launch_guard() {
    if !ready("a_declined_confirmation_stops_before_the_launch_guard") {
        return;
    }
    // `wipe\r\n` is the sharp one: the default `IFS` strips spaces and
    // tabs and nothing else, so the `\r` a CRLF line leaves behind is
    // part of the answer and refuses. A port reaching for `trim()`
    // rather than the two characters bash strips would confirm here and
    // wipe a registry the shell did not.
    for answer in ["no\n", "\n", "WIPE\n", "wipe now\n", "wipe\r\n"] {
        let world = World {
            confirmed: false,
            stdin: answer,
            ..World::remote()
        };

        let (shell, port) = world.both();
        diff(
            &format!("the answer {answer:?}"),
            &shell,
            &port,
            &Diagnostics::Quiet,
        );
        assert_eq!(
            shell.out(),
            "About to WIPE the deployed registry (cabin-registry, cabin-registry-blobs). Type \
             \"wipe\" to confirm: ",
            "{answer:?}"
        );
        assert_eq!(shell.err(), "FAIL: not confirmed\n", "{answer:?}");
        assert_eq!(shell.status, Some(1), "{answer:?}");
        assert!(
            shell.log.is_empty(),
            "{answer:?}: the prompt precedes the guard, so a declined run runs nothing \
             at all: {:#?}",
            shell.log
        );
        assert_eq!(shell.configured(), CONFIG, "{answer:?}");
        assert_eq!(shell.stamped(), STALE, "{answer:?}");
        assert_eq!(
            shell.objects.len(),
            7,
            "{answer:?}: nothing was swept: {:?}",
            shell.objects
        );
    }
}

/// The whole remote procedure end to end, confirmed through the
/// environment: the guard passes, the generation is read, the database
/// is dropped and recreated, both id sites are rewritten, migrations
/// replay, the stamp is refreshed, the bucket drains over several
/// listings, the generation is bumped and the Worker is redeployed.
///
/// The follow-ups heredoc is part of stdout, so its wording is pinned
/// by the byte comparison.
#[test]
fn a_confirmed_remote_wipe_runs_the_whole_procedure() {
    if !ready("a_confirmed_remote_wipe_runs_the_whole_procedure") {
        return;
    }
    let world = World::remote();

    let (shell, port) = world.both();
    diff(
        "a confirmed remote wipe",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a clean wipe is silent");
    assert_eq!(
        shell.out(),
        format!(
            "==> launch guard\n==> reading the pre-wipe registry generation\n==> dropping and \
             recreating the cabin-registry database\n{DROPPED}==> baking the new database id into \
             wrangler.jsonc ({NEW_ID})\n==> applying all migrations from zero\n{APPLIED}==> \
             refreshing the migrations-applied stamp\n==> deleting blobs/ from \
             cabin-registry-blobs\n    deleted 5 blob(s)\n==> bumping the registry generation to \
             8\n{BUMPED}==> redeploying (bakes the new database id into the \
             bindings)\n{DEPLOYED}wipe OK (generation 7 -> 8)\n\nFollow-ups, IN THIS ORDER \
             (docs/runbook.md, \"Post-wipe re-provisioning\"):\n  1. commit the wrangler.jsonc \
             database-id change and the refreshed\n     migrations-applied stamp\n  2. sign in \
             again and re-claim scopes (/claim/<scope>; a GitHub org's\n     OAuth app grant \
             survives the wipe, so re-claims grant immediately)\n  3. mint a verify-scoped token \
             FIRST and update the GitHub secret\n     (gh secret set \
             REGISTRY_VERIFY_TOKEN)\n  4. run cargo registry-governor wipe (from the repository \
             root) with it\n     BEFORE minting any\n     publish-capable token - its \
             no-delayed-publisher evidence gate\n     requires zero live publish tokens (refused \
             once launched)\n  5. only then mint publish tokens and update their secrets\n     (gh \
             secret set CABIN_PORTS_TOKEN)\n  6. re-promote the operator quota class - the wipe \
             reset every user to\n     'default', and a 17-package ports run exhausts the default \
             daily\n     new-package quota (registry/docs/architecture.md, \"Quota classes\")\n  \
             7. rerun whatever main CI went red against the old registry\n     (gh run rerun <id> \
             --failed)\n"
        )
    );
    assert!(
        !shell.out().contains(CREATED),
        "`d1 create`'s output is redirected to /dev/null: {:?}",
        shell.out()
    );

    // Both id sites carry the recreated id, and the file is otherwise
    // byte for byte what it was - the operator commits this diff.
    assert_eq!(shell.configured(), CONFIG.replace(OLD_ID, NEW_ID));
    assert_eq!(
        shell.configured().matches(NEW_ID).count(),
        2,
        "the binding and D1_DATABASE_ID both move"
    );
    assert_eq!(
        shell.stamped(),
        format!("{}\n", digest()),
        "the database now runs exactly the files' content"
    );
    assert_eq!(
        shell.tree,
        expanded(EMULATED.into_iter().map(|(path, _)| path)),
        "a remote wipe never touches the emulated state"
    );

    drained(&shell);

    // The two `d1 list --json` calls are byte-identical argv answered
    // differently across the recreate.
    assert_eq!(shell.commands("d1\tlist\t--json"), 2);
    assert_eq!(shell.commands("d1\tdelete\tcabin-registry\t-y"), 1);
    assert_eq!(shell.commands("d1\tcreate\tcabin-registry"), 1);
    assert_eq!(shell.commands("migrations\tapply\tDB\t--remote"), 1);
    assert_eq!(shell.commands("\tdeploy"), 1);
    assert_eq!(shell.shims("cargo"), 1, "one guard hop, and it ran");
    assert_eq!(shell.wrangler().len(), 9, "{:#?}", shell.wrangler());
}

/// The same run, confirmed at the prompt rather than through the
/// environment. The prompt is on stdout, unterminated, so the next step
/// line continues it.
///
/// The padded answers are not decoration: `read -r answer` splits on
/// the default `IFS`, so bash hands the comparison a line already
/// stripped of leading and trailing spaces and tabs - and a port that
/// compared the raw line would refuse all three of these where the
/// shell wipes.
#[test]
fn the_confirmation_is_accepted_interactively() {
    if !ready("the_confirmation_is_accepted_interactively") {
        return;
    }
    for answer in ["wipe\n", "  wipe \n", "\twipe\t\n"] {
        let world = World {
            confirmed: false,
            stdin: answer,
            ..World::remote()
        };

        let (shell, port) = world.both();
        diff(
            &format!("the answer {answer:?}"),
            &shell,
            &port,
            &Diagnostics::Quiet,
        );
        assert!(
            shell
                .out()
                .contains("Type \"wipe\" to confirm: ==> launch guard\n"),
            "{answer:?}: the prompt is unterminated, so the step line continues it: {:?}",
            shell.out()
        );
        assert!(
            shell.out().ends_with("(gh run rerun <id> --failed)\n"),
            "{answer:?}"
        );
        assert_eq!(shell.status, Some(0), "{answer:?}");
        assert_eq!(
            shell.configured(),
            CONFIG.replace(OLD_ID, NEW_ID),
            "{answer:?}"
        );
        assert_eq!(shell.stamped(), format!("{}\n", digest()), "{answer:?}");
        assert_eq!(shell.objects, survivors(), "{answer:?}");
    }
}

/// A launched registry: the guard refuses, and the run stops there.
/// Nothing destructive follows - no `d1 delete`, no sweep, no deploy -
/// and neither side's config, stamp or bucket moves.
///
/// stderr is compared byte for byte, which is the point of running the
/// TRUE guard on both sides: the shell reaches it through `cargo run`
/// and the port calls it as a function, and the operator must see the
/// same refusal either way.
#[test]
fn a_launched_registry_is_refused_by_the_guard() {
    if !ready("a_launched_registry_is_refused_by_the_guard") {
        return;
    }
    let mut world = World::remote();
    world.respond("launched", 0, value("true"));

    let (shell, port) = world.both();
    diff("a launched registry", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.out(), "==> launch guard\n");
    assert!(
        shell.err().contains(
            "the registry is launched (meta.launched = 'true'); its data is permanent and \
             destructive maintenance is forbidden"
        ),
        "the guard's own refusal reaches the operator: {:?}",
        shell.err()
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.configured(),
        CONFIG,
        "a refused guard rewrites nothing"
    );
    assert_eq!(shell.stamped(), STALE);
    assert_eq!(
        shell.objects.len(),
        7,
        "nothing was swept: {:?}",
        shell.objects
    );
    assert!(
        shell.requests.is_empty(),
        "the R2 API was never reached: {:?}",
        shell.requests
    );
    for absent in ["d1\tdelete", "d1\tcreate", "migrations\tapply", "\tdeploy"] {
        assert_eq!(
            shell.commands(absent),
            0,
            "a refused guard ran `{absent}`: {:#?}",
            shell.wrangler()
        );
    }
    // The guard's cross-check and its read, and nothing after them.
    assert_eq!(shell.wrangler().len(), 2, "{:#?}", shell.wrangler());
    assert_eq!(shell.shims("cargo"), 1, "the guard really ran");
}

/// A refused guard in LOCAL mode, which is the one that matters most:
/// local is where the run reaches `rm -rf`, and the guard's refusal is
/// the only thing between a launched registry and it.
///
/// This carries over the property `registry/tests/launch_guard.rs`
/// held before it was deleted with the script it drove:
/// `.wrangler/state/v3/d1` survives, and the ONLY wrangler call on the
/// log is the guard's own read. One call, not two - local has no
/// account-level name resolution, so the guard skips the `d1 list`
/// cross-check it makes on remote.
#[test]
fn a_refused_guard_leaves_the_local_state_intact() {
    if !ready("a_refused_guard_leaves_the_local_state_intact") {
        return;
    }
    let mut world = World::local();
    world.respond("launched", 0, value("true"));

    let (shell, port) = world.both();
    diff("a refused local guard", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(shell.out(), "==> launch guard\n");
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.tree,
        expanded(EMULATED.into_iter().map(|(path, _)| path)),
        "the emulated D1, R2, Durable Object and cache state all survive a \
         refused guard"
    );
    assert_eq!(shell.wrangler().len(), 1, "{:#?}", shell.wrangler());
    assert_eq!(
        shell.commands("key = 'launched'"),
        1,
        "the guard's read is the only thing that ran: {:#?}",
        shell.wrangler()
    );
    assert_eq!(shell.stamped(), STALE, "a refusal stamps nothing");
}

/// A `meta.registry_generation` that is not a number: the bump would be
/// meaningless, so the run refuses - and it refuses BEFORE anything
/// destructive, which is what the empty destructive-command set pins.
#[test]
fn a_non_numeric_generation_refuses_before_anything_destructive() {
    if !ready("a_non_numeric_generation_refuses_before_anything_destructive") {
        return;
    }
    for held in ["", "7x", "-1", "1 2", "null"] {
        let mut world = World::remote();
        world.respond("generation", 0, value(held));

        let (shell, port) = world.both();
        diff(
            &format!("the generation {held:?}"),
            &shell,
            &port,
            &Diagnostics::Quiet,
        );
        assert_eq!(
            shell.out(),
            "==> launch guard\n==> reading the pre-wipe registry generation\n",
            "{held:?}"
        );
        assert_eq!(
            shell.err(),
            format!("FAIL: meta.registry_generation is not numeric: '{held}'\n"),
            "{held:?}"
        );
        assert_eq!(shell.status, Some(1), "{held:?}");
        assert_eq!(shell.configured(), CONFIG, "{held:?}");
        assert_eq!(shell.objects.len(), 7, "{held:?}");
        for absent in ["d1\tdelete", "d1\tcreate", "migrations\tapply", "\tdeploy"] {
            assert_eq!(
                shell.commands(absent),
                0,
                "{held:?}: ran `{absent}`: {:#?}",
                shell.wrangler()
            );
        }
    }
}

/// The recreated database has to come back in `d1 list`, and the id it
/// reports is `db.uuid || db.database_id` - a *falsy* fallback, so an
/// EMPTY `uuid` falls through to `database_id` exactly as a missing one
/// does. A port reading `uuid` alone would refuse the second case where
/// the shell proceeds.
///
/// The shell's stderr also carries `node`'s own noise on the missing
/// case, so only the script's `FAIL:` line is compared there.
#[test]
fn the_recreated_database_must_appear_in_the_listing() {
    if !ready("the_recreated_database_must_appear_in_the_listing") {
        return;
    }
    // Absent from the listing entirely: `node` exits 1 and the `||
    // fail` fires.
    let mut missing = World::remote();
    missing.respond(
        "list.after",
        0,
        format!(
            "{}\n",
            serde_json::json!([{ "name": "cabin-other", "uuid": OLD_ID }])
        ),
    );

    let (shell, port) = missing.both();
    diff(
        "a database missing from d1 list",
        &shell,
        &port,
        &Diagnostics::Lines(&["FAIL: the recreated database is missing from d1 list"]),
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.configured(),
        CONFIG,
        "the id could not be learned, so nothing is baked in"
    );
    assert_eq!(shell.stamped(), STALE, "a refusal stamps nothing");
    assert_eq!(shell.objects.len(), 7, "nothing was swept");
    assert_eq!(shell.commands("\tdeploy"), 0, "nothing was deployed");

    // An empty `uuid` beside a valid `database_id`: JavaScript's `||`
    // takes the second, and the run carries on with it.
    let mut empty = World::remote();
    empty.respond("list.after", 0, listing("", Some(NEW_ID)));

    let (shell, port) = empty.both();
    diff(
        "an empty uuid beside a database_id",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.status,
        Some(0),
        "an empty uuid is falsy, not fatal: {:?}",
        shell.err()
    );
    assert_eq!(
        shell.configured(),
        CONFIG.replace(OLD_ID, NEW_ID),
        "the fallback id is the one baked in"
    );
    assert_eq!(shell.objects, survivors());
}

/// An id that is not 36 characters of the expected alphabet: the run
/// refuses rather than writing it into the config, where it would bind
/// the Worker to nothing.
#[test]
fn a_malformed_database_id_is_refused() {
    if !ready("a_malformed_database_id_is_refused") {
        return;
    }
    for malformed in [
        "aaaaaaaa-bbbb-cccc-",
        "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE",
    ] {
        let mut world = World::remote();
        world.respond("list.after", 0, listing(malformed, None));

        let (shell, port) = world.both();
        diff(
            &format!("the id {malformed:?}"),
            &shell,
            &port,
            &Diagnostics::Quiet,
        );
        assert_eq!(
            shell.err(),
            format!("FAIL: unexpected database id: '{malformed}'\n"),
            "{malformed:?}"
        );
        assert_eq!(shell.status, Some(1), "{malformed:?}");
        assert_eq!(
            shell.configured(),
            CONFIG,
            "{malformed:?}: a malformed id is never written"
        );
        assert_eq!(shell.stamped(), STALE, "{malformed:?}");
        assert_eq!(shell.objects.len(), 7, "{malformed:?}");
        assert_eq!(shell.commands("\tdeploy"), 0, "{malformed:?}");
    }
}

/// Two D1 bindings: the textual replace targets the FIRST
/// `database_id`, which is only the DB binding's while it is the only
/// one. The run refuses and tells the operator to bake it by hand.
///
/// The launch guard still passes - the DB binding is first and matches
/// the account - so this is a refusal reached AFTER the database was
/// dropped and recreated, which is exactly the state the message is
/// written for.
#[test]
fn two_database_id_bindings_refuse_to_bake_by_hand() {
    if !ready("two_database_id_bindings_refuse_to_bake_by_hand") {
        return;
    }
    let world = World {
        config: TWO_BINDINGS,
        ..World::remote()
    };

    let (shell, port) = world.both();
    diff("two d1 bindings", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.err(),
        "FAIL: wrangler.jsonc carries more than one d1 binding; bake the DB binding's new id in \
         by hand\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.configured(),
        TWO_BINDINGS,
        "the file is left exactly as it was"
    );
    assert_eq!(shell.stamped(), STALE, "a refusal stamps nothing");
    assert_eq!(shell.objects.len(), 7, "nothing was swept");
    assert_eq!(
        shell.commands("d1\tcreate\tcabin-registry"),
        1,
        "the refusal comes after the recreate, which is why it says `by hand`"
    );
    assert_eq!(shell.commands("migrations\tapply"), 0);
    assert_eq!(shell.commands("\tdeploy"), 0);
}

/// A listing that fails mid-drain: the sweep stops, the run refuses,
/// and the deploy never happens - a half-swept bucket must not be
/// certified by a redeploy.
///
/// The 500 lands on the SECOND listing, after the first page was
/// already deleted, so this also pins that a partial sweep is a
/// refusal rather than a resumable state.
///
/// `curl -fsS` comments on the 500 in its own wording, so only the
/// script's `FAIL:` line is compared.
#[test]
fn a_failed_listing_stops_the_sweep_before_the_deploy() {
    if !ready("a_failed_listing_stops_the_sweep_before_the_deploy") {
        return;
    }
    let world = World {
        fail_listing: 2,
        ..World::remote()
    };

    let (shell, port) = world.both();
    diff(
        "a failed listing",
        &shell,
        &port,
        &Diagnostics::Lines(&["FAIL: listing cabin-registry-blobs failed"]),
    );
    assert_eq!(shell.status, Some(1));
    assert!(
        shell
            .out()
            .ends_with("==> deleting blobs/ from cabin-registry-blobs\n"),
        "the run stopped inside the sweep: {:?}",
        shell.out()
    );
    assert!(
        !shell.out().contains("deleted"),
        "the count line belongs to a completed drain: {:?}",
        shell.out()
    );
    assert_eq!(
        shell.requests.len(),
        4,
        "one listing, two deletes, then the failing listing: {:#?}",
        shell.requests
    );
    assert_eq!(
        shell.objects.len(),
        5,
        "the first page really was deleted before the failure: {:?}",
        shell.objects
    );
    assert_eq!(
        shell.commands("\tdeploy"),
        0,
        "a half-swept bucket is never certified by a redeploy"
    );
    assert_eq!(
        shell.commands("UPDATE meta"),
        0,
        "the generation is bumped only after a complete sweep"
    );
    // The config and the stamp DID move: they precede the sweep, and
    // the operator is left with a rewritten config to commit and a
    // bucket to finish by hand.
    assert_eq!(shell.configured(), CONFIG.replace(OLD_ID, NEW_ID));
    assert_eq!(shell.stamped(), format!("{}\n", digest()));
}

/// `: "${CLOUDFLARE_API_TOKEN:?...}"` sits between the account-id read
/// and the `d1 delete`, so an unset token ends the run with the
/// database still intact. That ordering is the whole scenario: a check
/// one line later would leave a dropped database and no way to sweep.
///
/// bash writes the diagnostic itself, naming the fixture's own path and
/// line 112, so stderr is compared as refusal semantics with the
/// shell's text pinned below.
#[test]
fn an_absent_api_token_stops_before_the_database_is_dropped() {
    if !ready("an_absent_api_token_stops_before_the_database_is_dropped") {
        return;
    }
    let world = World {
        token: None,
        ..World::remote()
    };

    let (shell, port) = world.both();
    diff("an absent api token", &shell, &port, &Diagnostics::Refused);
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.out(),
        "==> launch guard\n==> reading the pre-wipe registry generation\n",
        "the run stopped before the first destructive step line"
    );
    assert_eq!(
        shell.commands("d1\tdelete"),
        0,
        "the token check precedes the drop: {:#?}",
        shell.wrangler()
    );
    assert_eq!(shell.commands("d1\tcreate"), 0);
    assert_eq!(shell.configured(), CONFIG, "nothing was rewritten");
    assert_eq!(shell.stamped(), STALE);
    assert_eq!(shell.objects.len(), 7, "nothing was swept");
    assert!(shell.requests.is_empty(), "the R2 API was never reached");
    // The guard's two calls and the generation read, and nothing else.
    assert_eq!(shell.wrangler().len(), 3, "{:#?}", shell.wrangler());

    // The shell's own wording, pinned rather than compared: bash's
    // `${VAR:?}` names the script's path and line.
    assert!(
        shell.err().contains(
            ": line 112: CLOUDFLARE_API_TOKEN: CLOUDFLARE_API_TOKEN is required for the R2 sweep\n"
        ),
        "bash's parameter-expansion diagnostic changed shape: {:?}",
        shell.err()
    );
}

/// The harness's own guard, asserted once on its own: every scenario
/// compares this checkout's `registry/wrangler.jsonc` and
/// `registry/migrations-applied` across its two runs, and these are the
/// files it reads. A rename would otherwise turn that comparison into a
/// panic in every test at once.
#[test]
fn the_real_registry_is_never_touched() {
    for (path, bytes) in real_registry() {
        assert!(
            path.is_file(),
            "{} is gone, so the guard every scenario runs cannot read it",
            show(&path)
        );
        assert!(!bytes.is_empty(), "{} is empty", show(&path));
    }
}
