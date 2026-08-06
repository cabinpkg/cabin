//! The whole-run differential for `scripts/migrate.sh`: the shell it
//! replaces and the port, run over one corpus of synthetic registry
//! roots and canned D1 answers, compared on stdout, stderr, exit
//! status, the sequence of commands each side issued, and the bytes
//! each left in `migrations-applied`.
//!
//! `tests/fixtures/migrate.sh.orig` is the original, byte for byte:
//! `registry/scripts/migrate.sh` as it stood on `main` at `098cd643d`,
//! `sha256`
//! `136357c5c32f31c70e32b695d237cdb43c04ce86ce0aa87d42a872170f2ac0a0`.
//! It is a standalone script rather than a workflow block, so the whole
//! file is the fixture and nothing is dedented. It sources
//! `scripts/lib.sh` after `cd`-ing to the registry root, so
//! `tests/fixtures/migrate-lib.sh.orig` vendors
//! `registry/scripts/lib.sh` from the same commit, `sha256`
//! `8d7a969ace6443efc5f3a478195da9c5e002a75cd7c2c2bc8140fa57edff556f`.
//! Nothing is prepended and nothing is edited - this suite *runs* those
//! files, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # A synthetic registry root, per side
//!
//! The script's first act is `cd "$(dirname -- "${BASH_SOURCE[0]}")/.."`,
//! so it operates on whatever tree it sits in. Each side therefore gets
//! its own scratch root holding `migrations/`, `migrations-applied` and
//! a `scripts/` directory with both vendored files copied in, which is
//! what makes `. scripts/lib.sh` resolve inside the corpus and never
//! against this checkout's own `registry/`. The port is pointed at the
//! same root through [`ROOT_VARIABLE`] and its working directory.
//!
//! The roots are per *side*, not per scenario: the run rewrites
//! `migrations-applied`, and a shared root would let the shell's stamp
//! stand in for the port's. [`Outcome::stamp`] is those bytes after the
//! run, compared like any other output - which is the only way a
//! scenario can say "and nothing was stamped".
//!
//! [`the_real_stamp_is_never_touched`] backs that with a check on this
//! checkout: a port still wired to `registry_dir()` would rewrite the
//! repository's own stamp, and every scenario asserts it did not.
//!
//! # The seam: one fake `npx`, two callers
//!
//! `lib.sh` reaches wrangler through
//! `wrangler() { npx --yes wrangler@4.112.0 "$@"; }`, and the port
//! spawns that same argv through `xtask_registry_admin::wrangler`. So
//! `tests/fixtures/fake-bin` goes first on both sides' `PATH` and a
//! fake `npx` answers both from `$FAKE_NPX_DIR`, logging every argv it
//! was called with to `$FAKE_NPX_LOG`.
//!
//! That log is what makes this a differential of *commands* and not
//! only of output. Each side gets its own, and [`diff`] asserts the two
//! are the identical sequence, so a port that reordered its arguments,
//! reflowed the SQL, or skipped a call fails here even when it reaches
//! the same verdict. The SQL is the sharp part: the has-table query is
//! one argument carrying a real newline and five spaces of continuation
//! indent, exactly as the script wrote it across two lines, and a port
//! that tidied it asks D1 the same question while failing parity. The
//! log maps newlines to `RS` so one call stays one line.
//!
//! Several scenarios lean on the log directly. Every refusal is pinned
//! by the *absence* of an apply call, which is the only observable
//! difference between "refused before applying" and "applied and then
//! refused to stamp" - and the second would have changed the live
//! database.
//!
//! Each record also carries WHERE the call was made, because wrangler
//! resolves its D1 and R2 bindings through the `wrangler.jsonc` of its
//! working directory: a side that ran the right argv in the wrong tree
//! would reach a different database while matching on every other byte
//! this suite compares - the fake `npx` answers the same either way.
//! The shell cannot get this wrong, since it `cd`s once and stays
//! there; a port that set the child's directory separately from where
//! it read the files can. The field is the presence of a marker file
//! rather than the path, because the two sides' roots are different
//! scratch directories. [`diff`] requires every call on both sides to
//! carry `[root]`, and it checks that before comparing the sequences,
//! since two sides that were both wrong would compare equal.
//!
//! `node` is not stubbed. The shell pipes wrangler's `--json` output
//! through two `node -e` projections, so the canned answers are real
//! wrangler-shaped JSON and the shell really parses them; the port
//! parses the same bytes its own way. `shasum`, `grep` and `cut` are
//! likewise the machine's own.
//!
//! # What is compared, and where the comparison stops
//!
//! stdout is compared as bytes everywhere - it carries the step lines,
//! the partition count, the confirmation prompt and whatever wrangler
//! itself printed through the inherited descriptor. The exit status is
//! compared exactly everywhere, because the script chooses every one of
//! them: `fail` exits 1, both success paths exit 0, and the argument
//! guard exits 1.
//!
//! stderr is compared byte for byte wherever the script is the sole
//! writer, which is every refusal it reaches through `fail`. Three
//! scenarios narrow:
//!
//! - [`a_failed_bookkeeping_read_refuses_rather_than_reading_empty`],
//!   where the shell's stderr also carries `node`'s own `SyntaxError`
//!   stack from parsing an empty capture. The port never runs `node`,
//!   so the assertion is that both sides emitted the script's `FAIL:`
//!   line whole.
//! - [`an_answer_without_a_newline_is_end_of_input`], where the shell
//!   says nothing at all: `read` fails at EOF and `set -e` ends the run
//!   before the refusal it was about to print. The two sides' stderr is
//!   asserted to differ in that specific way - silence against a
//!   diagnosed refusal - rather than left uncompared.
//! - [`the_argument_surface_refuses_the_same_inputs`], below.
//!
//! # The argument surface: a stated ceiling
//!
//! `${1:?usage: ...}` makes bash itself write
//! `<path>: line 28: 1: usage: ...`, naming the fixture's own path and
//! line number - not something a port can reproduce, and not something
//! it should. The wrong-argument arm's `usage: scripts/migrate.sh
//! <--remote|--local>` names a script that no longer exists once the
//! command is `cargo registry-migrate`. Both are compared as refusal
//! semantics: each side refused, said something, ran no wrangler
//! command and left the stamp alone. The shell's exact texts are pinned
//! beside the assertion so the move is visible rather than silent.
//!
//! # Why `LC_ALL=C`
//!
//! Both sides run under it, mirroring the checksums port's locale pin.
//! Two of the script's decisions are collation-dependent: the
//! `migrations/*.sql` glob that fixes concatenation order for the
//! stamp, and `[[ "$name" < "$last_applied" ]]`, which compares under
//! `LC_COLLATE`. A port comparing bytes agrees with the shell only
//! where the shell is also comparing bytes, so the corpus pins the byte
//! order and [`a_pending_migration_that_sorts_early_is_refused`] uses a
//! name (`0001a_wedge.sql`) that sits between two applied ones only
//! under it.
//!
//! # Not covered here, and why
//!
//! - **Whether wrangler really answers the way the corpus says.** Both
//!   sides are handed the same stand-in by construction, so a wrong
//!   response shape is one both sides get equally wrong. What this
//!   suite covers is that the two *ask* the same things and read the
//!   answers the same way.
//! - **A `d1_migrations` table that exists but is unreadable.** The
//!   script's two reads fail identically - the first is the scenario
//!   below, and the second differs only in which `fail` message it
//!   reaches - so the second adds a message, not a path.
//! - **An interactive terminal.** `read -r answer` is fed from a file
//!   on both sides. A tty would change nothing the script observes; it
//!   would only make the suite need one.
//!
//! The suite is Unix-only outright. The original is a bash script whose
//! tools are matched by name; a Windows host's lookalikes EXIST on
//! `PATH` and would pass a presence check while meaning something else.
//! Every test skips rather than fails when a tool it needs is missing,
//! and the harness's own failures panic.
//!
//! # Negative proofs
//!
//! All three were run by hand against the port, from a green suite,
//! then reverted, with both fixtures' `sha256` re-checked afterwards.
//!
//! - **A one-sided divergence in behavior is caught.** Supplying the
//!   port side alone with `apply` where the shell got `migrate`, which
//!   is what a port that accepted a different keyword would do, failed
//!   [`the_confirmation_is_accepted_interactively`] and nothing else at
//!   all - 13 of 14 still passed.
//!   [`a_fresh_database_applies_everything_and_stamps`], which reaches
//!   the same apply through `CABIN_MIGRATE_YES`, stayed green, as did
//!   [`an_unconfirmed_run_applies_nothing`], where both sides refuse
//!   either way. So the prompt scenarios really do exercise the keyword
//!   and the env-bypass scenario really does bypass it. It failed on
//!   `the two sides ran different commands` rather than on stdout,
//!   because a side that refuses at the prompt never issues the apply -
//!   the command log sees it first.
//! - **The command-parity assertion is load-bearing.** Dropping the
//!   port side's last log line in [`World::side`], which is a port that
//!   issued one command fewer with its output, status and stamp
//!   unchanged, failed 12 of the 14, every one of them on `the two
//!   sides ran different commands` and on nothing else. A skipped
//!   wrangler call is therefore caught on its own, rather than only
//!   when it happens to change the verdict. The two survivors are the
//!   two that issue no wrangler command at all -
//!   [`the_real_stamp_is_never_touched`], which runs neither side, and
//!   [`the_argument_surface_refuses_the_same_inputs`], which is an
//!   independent confirmation that the argument guard refuses before
//!   reaching wrangler.
//! - **The working-directory field reads the working directory.**
//!   Writing the marker one level above the root the run is given -
//!   a side whose wrangler child runs outside the tree it read its
//!   files from - failed the same 12 on `ran wrangler outside the
//!   scenario's registry root` and nothing else, so the field is not
//!   quietly constant.
//!
//! Separately, the harness was validated without the port at all, by
//! pointing [`Side::Port`] at the fixture as well and running the
//! corpus shell-against-shell. That passed 13 of 14 - everything but
//! the last assertion of
//! [`an_answer_without_a_newline_is_end_of_input`], which is about the
//! port speaking where the shell is silent and so cannot hold when both
//! sides are the shell. It is what establishes that the expected byte
//! strings throughout this file are the shell's real output rather than
//! a transcription of it, and that no state leaks between the two
//! sides.
#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use assert_fs::TempDir;
use sha2::{Digest as _, Sha256};

/// Points the port at a scenario's synthetic registry root instead of
/// this checkout's own `registry/`. The shell needs no equivalent: it
/// derives its root from its own path, and the harness copies it into
/// the root it is meant to operate on.
const ROOT_VARIABLE: &str = "CABIN_REGISTRY_DIR";

/// A migration file: its name, and the bytes that go into the stamp.
type Migration = (&'static str, &'static str);

const INIT: Migration = ("0001_init.sql", "CREATE TABLE alpha (id INTEGER);\n");
const MORE: Migration = ("0002_more.sql", "CREATE TABLE beta (id INTEGER);\n");
const LATER: Migration = ("0003_later.sql", "CREATE TABLE gamma (id INTEGER);\n");

/// Sorts between [`INIT`] and [`MORE`] under `LC_ALL=C`, where `_`
/// (0x5f) precedes `a` (0x61). That is the whole point of it: as a
/// pending file beside those two applied ones it is the ordering
/// violation the script refuses.
const WEDGE: Migration = ("0001a_wedge.sql", "CREATE TABLE delta (id INTEGER);\n");

/// What the corpus writes into `migrations-applied` when a scenario
/// wants a stamp that is merely *wrong*. Distinctive on purpose: it is
/// not a hash, so a side that echoed it back would be visible.
const STALE: &str = "0000000000000000000000000000000000000000000000000000000000000000\n";

/// What the fake wrangler prints when it applies. It reaches stdout
/// through the descriptor the script never redirected, so it is part of
/// the compared bytes and a port that captured it would diverge. The
/// wording is arbitrary and tracks nothing in the corpus - what is
/// tested is that it survives byte for byte, non-ASCII included.
const APPLIED: &str = "🌀 Executing on remote database DB: 2 commands\n";

/// The tools every scenario drives, on top of the port itself.
const TOOLS: [&str; 5] = ["bash", "node", "shasum", "grep", "tr"];

/// How far stderr can be compared.
enum Diagnostics<'a> {
    /// The script was the only writer: compare byte for byte.
    Quiet,
    /// `node` also wrote its own stack. Assert both sides emitted each
    /// of these as a whole line and leave the rest to the ceiling.
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
    /// The argv of every `npx` call the side made, in order.
    log: Vec<String>,
    /// `migrations-applied` as the run left it. Compared like any other
    /// output: it is the one thing the script writes that outlives it.
    stamp: Vec<u8>,
}

impl Outcome {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    fn stamped(&self) -> String {
        String::from_utf8_lossy(&self.stamp).into_owned()
    }

    /// How many commands carried `fragment`. Arguments are tab
    /// separated, so a fragment spanning two of them spells the tab.
    fn commands(&self, fragment: &str) -> usize {
        self.log
            .iter()
            .filter(|call| call.contains(fragment))
            .count()
    }
}

/// One scenario: the registry root to materialize, the canned answers
/// the fake wrangler serves, and how the run is invoked.
struct World {
    /// In glob order, which under `LC_ALL=C` is byte order.
    migrations: Vec<Migration>,
    /// The bytes `migrations-applied` starts with.
    stamp: String,
    /// `(<kind>[.<phase>], exit status, stdout)`.
    responses: Vec<(&'static str, i32, String)>,
    /// The argument the run is given, if any.
    mode: Option<&'static str>,
    /// `CABIN_MIGRATE_YES=1`.
    confirmed: bool,
    /// What `read -r answer` is fed.
    stdin: &'static str,
}

impl World {
    /// A remote run over a live database that has already applied
    /// `applied` of `migrations`, with the stamp those applied files
    /// hash to and the confirmation bypassed. Scenarios override the
    /// fields they are about.
    fn remote(migrations: &[Migration], applied: &[Migration]) -> Self {
        Self {
            migrations: migrations.to_vec(),
            stamp: format!("{}\n", digest(applied)),
            responses: vec![
                ("has-table", 0, count(1)),
                ("names", 0, names(filenames(applied))),
                ("apply", 0, APPLIED.to_owned()),
            ],
            mode: Some("--remote"),
            confirmed: true,
            stdin: "",
        }
    }

    /// A remote run against a database that has never been migrated:
    /// no `d1_migrations` table before the apply, and every file
    /// recorded after it.
    fn fresh(migrations: &[Migration]) -> Self {
        Self {
            stamp: String::new(),
            responses: vec![
                ("has-table.before", 0, count(0)),
                ("has-table.after", 0, count(1)),
                ("names.after", 0, names(filenames(migrations))),
                ("apply", 0, APPLIED.to_owned()),
            ],
            ..Self::remote(migrations, &[])
        }
    }

    /// Replaces the canned answer for `kind`, or adds it.
    fn respond(&mut self, kind: &'static str, status: i32, body: String) {
        self.responses.retain(|(name, _, _)| *name != kind);
        self.responses.push((kind, status, body));
    }

    /// Runs both sides over their own copies of the same root, each
    /// with its own command log.
    fn both(&self) -> (Outcome, Outcome) {
        let npx = fake_bin().join("npx");
        let mode = fs::metadata(&npx)
            .expect("the fake npx")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "{} lost its executable bit, so PATH would find the real npx",
            show(&npx)
        );

        let before = real_stamp();
        let shell = self.side(Side::Shell);
        let port = self.side(Side::Port);
        // Restored before the panic, not merely detected: a port still
        // reading `registry_dir()` writes the repository's own stamp,
        // and a suite that left that behind would have edited the
        // checkout it was run in.
        let after = real_stamp();
        if after != before {
            fs::write(real_stamp_path(), &before).expect("restoring this checkout's stamp");
            panic!(
                "a side rewrote this checkout's own registry/migrations-applied \
                 (restored): the port is reading `registry_dir()` rather than \
                 {ROOT_VARIABLE}"
            );
        }
        (shell, port)
    }

    fn side(&self, side: Side) -> Outcome {
        let dir = TempDir::new().expect("a scratch directory");
        let root = dir.path().join("root");
        let migrations = root.join("migrations");
        let scripts = root.join("scripts");
        let responses = dir.path().join("responses");
        for made in [&migrations, &scripts, &responses] {
            fs::create_dir_all(made).expect("a directory of the scenario's root");
        }

        for (name, body) in &self.migrations {
            fs::write(migrations.join(name), body).expect("a migration file");
        }
        let stamp = root.join("migrations-applied");
        fs::write(&stamp, &self.stamp).expect("the stamp file");
        // What the fake wrangler recognizes this root by. A dotfile, so
        // the `migrations/*.sql` glob never sees it.
        fs::write(root.join(".differential-root"), b"").expect("the root marker");
        for (name, source) in [("migrate.sh", "migrate.sh"), ("lib.sh", "migrate-lib.sh")] {
            let vendored = fixtures().join(format!("{source}.orig"));
            fs::copy(&vendored, scripts.join(name)).expect("the vendored script");
        }

        fs::write(responses.join("phase"), "before").expect("the fake database's phase");
        for (name, status, body) in &self.responses {
            fs::write(responses.join(name), format!("{status}\n{body}"))
                .expect("a canned wrangler answer");
        }

        let answers = dir.path().join("stdin");
        fs::write(&answers, self.stdin).expect("the confirmation's answers");
        let log = dir.path().join("commands");
        fs::write(&log, b"").expect("the command log");

        let mut command = match side {
            Side::Shell => {
                let mut bash = Command::new("bash");
                bash.arg(scripts.join("migrate.sh"));
                bash
            }
            Side::Port => {
                let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-registry-admin"));
                ported.arg("migrate");
                ported
            }
        };
        if let Some(mode) = self.mode {
            command.arg(mode);
        }
        command
            .current_dir(&root)
            .env(ROOT_VARIABLE, &root)
            .env("PATH", path_through_the_fake_npx())
            .env("FAKE_NPX_LOG", &log)
            .env("FAKE_NPX_DIR", &responses)
            // Both collation-dependent decisions - the glob order the
            // stamp concatenates in, and the `<` that refuses an
            // early-sorting pending file - are pinned to byte order.
            .env("LC_ALL", "C")
            .stdin(Stdio::from(
                File::open(&answers).expect("the confirmation's answers"),
            ));
        if self.confirmed {
            command.env("CABIN_MIGRATE_YES", "1");
        } else {
            command.env_remove("CABIN_MIGRATE_YES");
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
            stamp: fs::read(&stamp).expect("the stamp file the run left behind"),
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Shell,
    Port,
}

/// The stamp the script writes: `sha256` of the files' bytes
/// concatenated in glob order, as `cat migrations/*.sql | shasum -a
/// 256 | cut -d' ' -f1` computes it. Recomputed here rather than read
/// off either side, so a scenario asserting a stamp is asserting the
/// rule and not one implementation's answer.
fn digest(migrations: &[Migration]) -> String {
    let mut hasher = Sha256::new();
    for (_, body) in migrations {
        hasher.update(body.as_bytes());
    }
    cabin_core::hash::hex_digest(&hasher.finalize())
}

/// `SELECT COUNT(*) AS n ...` in wrangler's `--json` shape. D1 answers
/// a count as a JSON number, and the shell's `console.log` renders it
/// the same way it renders the string - which is what the script
/// compares against `"0"`.
fn count(n: u8) -> String {
    format!("[{{\"results\":[{{\"n\":{n}}}],\"success\":true,\"meta\":{{\"duration\":1}}}}]\n")
}

/// `SELECT name FROM d1_migrations ORDER BY name` in the same shape.
/// Takes names rather than files, because the state this refuses to
/// stamp is exactly the one where the two disagree.
fn names<'a>(recorded: impl IntoIterator<Item = &'a str>) -> String {
    let rows: Vec<String> = recorded
        .into_iter()
        .map(|name| format!("{{\"name\":\"{name}\"}}"))
        .collect();
    format!(
        "[{{\"results\":[{}],\"success\":true,\"meta\":{{\"duration\":1}}}}]\n",
        rows.join(",")
    )
}

fn filenames(migrations: &[Migration]) -> Vec<&'static str> {
    migrations.iter().map(|(name, _)| *name).collect()
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fake_bin() -> PathBuf {
    fixtures().join("fake-bin")
}

fn real_stamp_path() -> PathBuf {
    xtask_registry_admin::registry_dir().join("migrations-applied")
}

/// This checkout's own stamp, which no scenario may write.
fn real_stamp() -> Vec<u8> {
    fs::read(real_stamp_path()).expect("this checkout's registry/migrations-applied")
}

fn path_through_the_fake_npx() -> std::ffi::OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut directories = vec![fake_bin()];
    directories.extend(std::env::split_paths(&inherited));
    std::env::join_paths(directories).expect("a PATH with the fake npx first")
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
    // Before the sequences are compared against each other, because
    // two sides that both ran in the wrong tree would compare equal.
    for (side, outcome) in [("shell", shell), ("port", port)] {
        for call in &outcome.log {
            assert!(
                call.starts_with("[root]\t"),
                "{case}: the {side} ran wrangler outside the scenario's registry root, \
                 where a different wrangler.jsonc binds a different database: {call}"
            );
        }
    }
    assert!(
        shell.log == port.log,
        "{case}: the two sides ran different commands\nshell: {:#?}\nport:  {:#?}",
        shell.log,
        port.log
    );
    assert!(
        shell.stdout == port.stdout,
        "{case}: stdout\nshell: {}\nport:  {}",
        shell.stdout.escape_ascii(),
        port.stdout.escape_ascii()
    );
    assert_eq!(shell.status, port.status, "{case}: exit status");
    assert!(
        shell.stamp == port.stamp,
        "{case}: the stamp each run left behind\nshell: {}\nport:  {}",
        shell.stamp.escape_ascii(),
        port.stamp.escape_ascii()
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

/// A local apply is the one mode that reads nothing and stamps nothing:
/// it proves only that the files run, and the stamp attests the LIVE
/// schema. One command, and the stamp is left exactly as it was - here
/// deliberately a value that is not the right one, so a side that
/// refreshed it would be caught rather than accidentally correct.
#[test]
fn a_local_apply_never_touches_the_stamp() {
    if !ready("a_local_apply_never_touches_the_stamp") {
        return;
    }
    let world = World {
        stamp: STALE.to_owned(),
        mode: Some("--local"),
        ..World::remote(&[INIT, MORE], &[INIT, MORE])
    };

    let (shell, port) = world.both();
    diff("a local apply", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        format!(
            "==> applying migrations to the local database\n{APPLIED}local migrate OK (the \
             migrations-applied stamp tracks the live\ndatabase only; a local apply never touches \
             it)\n"
        )
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a local apply is silent");
    assert_eq!(
        shell.stamped(),
        STALE,
        "the stamp is not the local run's to refresh"
    );
    assert_eq!(shell.log.len(), 1, "nothing is read, only applied");
    assert_eq!(
        shell.commands("apply\tDB\t--local"),
        1,
        "the apply went to the local state: {:?}",
        shell.log
    );
}

/// The argument guard, whose texts are the stated ceiling: bash's
/// `${1:?}` names the fixture's own path and line, and the wrong-
/// argument arm names `scripts/migrate.sh`, which the port is not. What
/// is compared is that both refused, both said something, neither
/// reached wrangler and neither wrote the stamp. The shell's exact
/// texts are pinned here so the move is visible.
#[test]
fn the_argument_surface_refuses_the_same_inputs() {
    if !ready("the_argument_surface_refuses_the_same_inputs") {
        return;
    }
    for (case, mode) in [("no argument", None), ("a wrong argument", Some("--nope"))] {
        let world = World {
            mode,
            stamp: STALE.to_owned(),
            ..World::remote(&[INIT, MORE], &[INIT, MORE])
        };

        let (shell, port) = world.both();
        diff(case, &shell, &port, &Diagnostics::Refused);
        assert!(
            shell.stdout.is_empty(),
            "{case}: the refusal belongs on stderr"
        );
        assert_eq!(shell.status, Some(1), "{case}");
        assert!(shell.log.is_empty(), "{case}: nothing was run against D1");
        assert_eq!(shell.stamped(), STALE, "{case}: the stamp is untouched");
    }

    // The shell's own wording, pinned rather than compared: `${1:?}`
    // is bash's diagnostic, naming the script's path and line 28.
    let missing = World {
        mode: None,
        ..World::remote(&[INIT], &[INIT])
    };
    let (shell, _) = missing.both();
    assert!(
        shell
            .err()
            .contains(": line 28: 1: usage: scripts/migrate.sh <--remote|--local>\n"),
        "bash's parameter-expansion diagnostic changed shape: {:?}",
        shell.err()
    );

    let wrong = World {
        mode: Some("--nope"),
        ..World::remote(&[INIT], &[INIT])
    };
    let (shell, _) = wrong.both();
    assert_eq!(
        shell.err(),
        "usage: scripts/migrate.sh <--remote|--local>\n",
        "the script's own usage line is the whole of its stderr"
    );
}

/// A database that has never been migrated: no `d1_migrations` table at
/// all, so the applied set is empty without a second query, everything
/// is pending, and the run applies and stamps. The follow-up block is
/// part of stdout, so its wording is pinned by the comparison.
#[test]
fn a_fresh_database_applies_everything_and_stamps() {
    if !ready("a_fresh_database_applies_everything_and_stamps") {
        return;
    }
    let world = World::fresh(&[INIT, MORE]);

    let (shell, port) = world.both();
    diff("a fresh database", &shell, &port, &Diagnostics::Quiet);
    let stamp = digest(&[INIT, MORE]);
    assert_eq!(
        shell.out(),
        format!(
            "==> reading the applied set from the live database\n    0 applied, 2 pending \
             migration file(s)\n==> applying migrations to the live database\n{APPLIED}==> \
             verifying the live database now records every migration file\n==> refreshing the \
             migrations-applied stamp\nremote migrate OK (stamp {stamp})\n\nFollow-ups:\n  - \
             commit the migrations-applied change; the CI deploy stays skipped\n    until it \
             reaches main (docs/runbook.md, \"Integrated topology\")\n"
        )
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty(), "a clean apply is silent");
    assert_eq!(
        shell.stamped(),
        format!("{stamp}\n"),
        "the stamp is the digest of every file, newline terminated"
    );

    // An absent `d1_migrations` table short-circuits: the names query
    // is never asked before the apply, only after it.
    assert_eq!(
        shell.commands("SELECT name FROM d1_migrations"),
        1,
        "the applied set was read twice from a table that existed once: {:?}",
        shell.log
    );
    assert_eq!(shell.log.len(), 4, "{:?}", shell.log);
}

/// Nothing pending is a clean exit that stamps nothing: the stamp is
/// already what the files hash to, and the run ends before the
/// confirmation, the apply and the refresh.
#[test]
fn nothing_pending_ends_the_run_without_applying() {
    if !ready("nothing_pending_ends_the_run_without_applying") {
        return;
    }
    let world = World::remote(&[INIT, MORE], &[INIT, MORE]);
    let current = world.stamp.clone();

    let (shell, port) = world.both();
    diff("nothing pending", &shell, &port, &Diagnostics::Quiet);
    assert_eq!(
        shell.out(),
        "==> reading the applied set from the live database\n    2 applied, 0 pending migration \
         file(s)\nremote migrate OK (nothing pending; the stamp is already current)\n"
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty());
    assert_eq!(
        shell.stamped(),
        current,
        "an untouched stamp is not a rewritten one"
    );
    assert_eq!(
        shell.commands("migrations\tapply"),
        0,
        "nothing was applied"
    );
    assert_eq!(
        shell.log.len(),
        2,
        "the has-table probe and the names query"
    );
}

/// A name D1 recorded whose file is gone: its effects live on in the
/// live schema while `migrations/` no longer describes them, and no
/// stamp may certify that.
///
/// The stamp here is deliberately *also* wrong, which pins the order:
/// a port that checked the stamp first would refuse with the other
/// message, and the two are told apart by these bytes alone.
#[test]
fn a_recorded_migration_with_no_file_is_refused() {
    if !ready("a_recorded_migration_with_no_file_is_refused") {
        return;
    }
    let mut world = World {
        stamp: STALE.to_owned(),
        ..World::remote(&[INIT, MORE], &[INIT, MORE])
    };
    world.respond(
        "names",
        0,
        names(
            filenames(&[INIT, MORE])
                .into_iter()
                .chain(["0009_ghost.sql"]),
        ),
    );

    let (shell, port) = world.both();
    diff(
        "a recorded migration with no file",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.out(),
        "==> reading the applied set from the live database\n    2 applied, 0 pending migration \
         file(s)\n"
    );
    assert_eq!(
        shell.err(),
        "FAIL: D1 records applied migration '0009_ghost.sql' but migrations/0009_ghost.sql is \
         gone\n(renamed or removed). The live schema still carries its effects; restore\nthe \
         file, or reset pre-launch via scripts/wipe.sh.\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(shell.stamped(), STALE, "a refusal stamps nothing");
    assert_eq!(
        shell.commands("migrations\tapply"),
        0,
        "nothing was applied"
    );
}

/// An already-applied file edited in place: D1 tracks by filename and
/// will never replay it, so refreshing the aggregate stamp would
/// certify a live schema that does not match `migrations/`.
#[test]
fn an_applied_file_edited_in_place_is_refused() {
    if !ready("an_applied_file_edited_in_place_is_refused") {
        return;
    }
    let world = World {
        stamp: STALE.to_owned(),
        ..World::remote(&[INIT, MORE, LATER], &[INIT, MORE])
    };

    let (shell, port) = world.both();
    diff(
        "an applied file edited in place",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.out(),
        "==> reading the applied set from the live database\n    2 applied, 1 pending migration \
         file(s)\n"
    );
    assert_eq!(
        shell.err(),
        "FAIL: an already-applied migration file was edited in place (the applied\nfiles no \
         longer hash to the migrations-applied stamp). D1 will NOT\nreplay it, and stamping would \
         unblock deploys against a stale live\nschema. Pre-launch, the edited baseline ships \
         through scripts/wipe.sh;\npost-launch, write a NEW migration file instead of editing an \
         applied\none.\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(shell.stamped(), STALE, "a refusal stamps nothing");
    assert_eq!(
        shell.commands("migrations\tapply"),
        0,
        "nothing was applied"
    );
}

/// A pending file that sorts before an applied one: a fresh database
/// replays `migrations/*.sql` in glob order, so the live database and a
/// rebuilt one would carry different histories under one stamp.
///
/// `0001a_wedge.sql` lands between the two applied files only under
/// `LC_ALL=C`, which both sides run with.
#[test]
fn a_pending_migration_that_sorts_early_is_refused() {
    if !ready("a_pending_migration_that_sorts_early_is_refused") {
        return;
    }
    let world = World::remote(&[INIT, WEDGE, MORE], &[INIT, MORE]);
    let current = world.stamp.clone();

    let (shell, port) = world.both();
    diff(
        "a pending migration that sorts early",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.out(),
        "==> reading the applied set from the live database\n    2 applied, 1 pending migration \
         file(s)\n"
    );
    assert_eq!(
        shell.err(),
        "FAIL: pending migration 0001a_wedge.sql sorts before applied 0002_more.sql; a\nfresh \
         database would replay them in a different order than the live one\nran. Name new \
         migrations after every applied one.\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(shell.stamped(), current, "a refusal stamps nothing");
    assert_eq!(
        shell.commands("migrations\tapply"),
        0,
        "nothing was applied"
    );
}

/// The confirmation is a real gate: an answer that is not `migrate`
/// refuses before anything reaches the live database. The prompt itself
/// is on stdout, unterminated, and is part of the compared bytes.
#[test]
fn an_unconfirmed_run_applies_nothing() {
    if !ready("an_unconfirmed_run_applies_nothing") {
        return;
    }
    // `migrate\r\n` is the sharp one: the default `IFS` strips spaces
    // and tabs, and nothing else, so the `\r` a CRLF line leaves behind
    // is part of the answer and refuses. A port that reached for
    // `trim()` rather than the two characters bash strips would confirm
    // here and apply migrations the shell did not.
    for answer in [
        "nope\n",
        "\n",
        "migrate later\n",
        "MIGRATE\n",
        "migrate\r\n",
    ] {
        let world = World {
            confirmed: false,
            stdin: answer,
            ..World::fresh(&[INIT, MORE])
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
            "==> reading the applied set from the live database\n    0 applied, 2 pending \
             migration file(s)\nAbout to apply 2 migration file(s) to the LIVE database. Type \
             \"migrate\" to confirm: ",
            "{answer:?}"
        );
        assert_eq!(shell.err(), "FAIL: not confirmed\n", "{answer:?}");
        assert_eq!(shell.status, Some(1), "{answer:?}");
        assert_eq!(shell.stamped(), "", "{answer:?}: a refusal stamps nothing");
        assert_eq!(
            shell.commands("migrations\tapply"),
            0,
            "{answer:?}: nothing was applied"
        );
    }
}

/// An answer with no closing newline is end of input, and it refuses
/// whatever it says - `migrate` included. `read -r answer` returns
/// non-zero at EOF, and it is a bare command rather than a condition,
/// so `set -e` ends the run before the comparison it was read for ever
/// happens.
///
/// This is the reachable non-interactive shape: a run with no
/// `CABIN_MIGRATE_YES` and nothing on stdin, which is what a scheduled
/// or piped invocation looks like.
///
/// stderr is the one thing not compared here, and the reason is the
/// finding rather than an excuse: `set -e` kills the shell SILENTLY, so
/// the script's own `FAIL: not confirmed` is never reached, while the
/// port refuses through its error type and says so. Byte parity on this
/// path would mean the port going silent, which is worse for the
/// operator than the divergence. Both halves are asserted below so the
/// difference is pinned rather than skipped.
#[test]
fn an_answer_without_a_newline_is_end_of_input() {
    if !ready("an_answer_without_a_newline_is_end_of_input") {
        return;
    }
    for answer in ["", "migrate", "nope"] {
        let world = World {
            confirmed: false,
            stdin: answer,
            ..World::fresh(&[INIT, MORE])
        };

        let (shell, port) = world.both();
        diff(
            &format!("the unterminated answer {answer:?}"),
            &shell,
            &port,
            &Diagnostics::Lines(&[]),
        );
        assert_eq!(shell.status, Some(1), "{answer:?}");
        assert!(
            shell.out().ends_with("Type \"migrate\" to confirm: "),
            "{answer:?}: the run stopped at the prompt: {:?}",
            shell.out()
        );
        assert_eq!(shell.stamped(), "", "{answer:?}: a refusal stamps nothing");
        assert_eq!(
            shell.commands("migrations\tapply"),
            0,
            "{answer:?}: nothing was applied"
        );
        assert!(
            shell.stderr.is_empty(),
            "{answer:?}: `set -e` kills the shell without a word, so the script's \
             own refusal text is never reached: {:?}",
            shell.err()
        );
        assert!(
            !port.stderr.is_empty(),
            "{answer:?}: the port refused as silently as the shell did, which is \
             the one way this divergence should NOT be resolved"
        );
    }
}

/// The same run, confirmed at the prompt rather than through the
/// environment: the answer `migrate` reaches the same end as
/// [`a_fresh_database_applies_everything_and_stamps`], with the prompt
/// in stdout.
///
/// The padded answers are not decoration. `read -r answer` splits on
/// the default `IFS`, so bash hands the comparison a line already
/// stripped of leading and trailing spaces and tabs - and a port that
/// compared the raw line would refuse all three of these where the
/// shell proceeds. Nothing else is stripped: `migrate later` is in
/// [`an_unconfirmed_run_applies_nothing`], where an interior space
/// makes it a different answer.
#[test]
fn the_confirmation_is_accepted_interactively() {
    if !ready("the_confirmation_is_accepted_interactively") {
        return;
    }
    for answer in ["migrate\n", "   migrate  \n", "\tmigrate\t\n"] {
        let world = World {
            confirmed: false,
            stdin: answer,
            ..World::fresh(&[INIT, MORE])
        };

        let (shell, port) = world.both();
        diff(
            &format!("the answer {answer:?}"),
            &shell,
            &port,
            &Diagnostics::Quiet,
        );
        assert!(
            shell.out().contains(
                "About to apply 2 migration file(s) to the LIVE database. Type \"migrate\" to \
                 confirm: ==> applying migrations to the live database\n"
            ),
            "{answer:?}: the prompt is unterminated, so the next step line continues it: {:?}",
            shell.out()
        );
        assert_eq!(shell.status, Some(0), "{answer:?}");
        assert_eq!(
            shell.stamped(),
            format!("{}\n", digest(&[INIT, MORE])),
            "{answer:?}"
        );
        assert_eq!(
            shell.commands("migrations\tapply\tDB\t--remote"),
            1,
            "{answer:?}"
        );
    }
}

/// An unreadable applied set must never read as an empty one: an empty
/// one would make every file pending and stamp a database whose real
/// bookkeeping nobody could see. Only an *absent* `d1_migrations` table
/// is the never-migrated database.
///
/// The shell's stderr also carries `node`'s `SyntaxError` from parsing
/// the empty capture, which the port - which runs no `node` - has no
/// way to reproduce, so only the script's own line is compared.
#[test]
fn a_failed_bookkeeping_read_refuses_rather_than_reading_empty() {
    if !ready("a_failed_bookkeeping_read_refuses_rather_than_reading_empty") {
        return;
    }
    let mut world = World::remote(&[INIT, MORE], &[INIT]);
    world.respond("has-table", 1, String::new());
    let current = world.stamp.clone();

    let (shell, port) = world.both();
    diff(
        "a failed bookkeeping read",
        &shell,
        &port,
        &Diagnostics::Lines(&["FAIL: could not read the live database's migration bookkeeping"]),
    );
    assert_eq!(
        shell.out(),
        "==> reading the applied set from the live database\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(shell.stamped(), current, "a refusal stamps nothing");
    assert_eq!(shell.log.len(), 1, "the run stopped at the first query");
}

/// The verification after the apply is what separates "wrangler exited
/// 0" from "the live database records these files". A file the
/// post-apply read does not name refuses, and - the point of the
/// scenario - the stamp is NOT written, even though the apply already
/// ran.
#[test]
fn a_file_missing_after_the_apply_refuses_to_stamp() {
    if !ready("a_file_missing_after_the_apply_refuses_to_stamp") {
        return;
    }
    let mut world = World::fresh(&[INIT, MORE]);
    world.respond("names.after", 0, names(filenames(&[INIT])));

    let (shell, port) = world.both();
    diff(
        "a file missing after the apply",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    assert_eq!(
        shell.out(),
        format!(
            "==> reading the applied set from the live database\n    0 applied, 2 pending \
             migration file(s)\n==> applying migrations to the live database\n{APPLIED}==> \
             verifying the live database now records every migration file\n"
        )
    );
    assert_eq!(
        shell.err(),
        "FAIL: 0002_more.sql is not recorded as applied after the apply; do not stamp\n"
    );
    assert_eq!(shell.status, Some(1));
    assert_eq!(
        shell.stamped(),
        "",
        "the apply ran, so the stamp is the only thing standing between a \
         half-applied database and a green deploy gate"
    );
    assert_eq!(shell.commands("migrations\tapply"), 1, "the apply did run");
}

/// The ordinary post-launch shape: some files applied, one new one
/// pending, and it sorts after every applied name. The partition line
/// reports both counts, the apply runs, and the stamp is refreshed to
/// cover every file rather than the newly applied one.
#[test]
fn a_pending_file_beside_applied_ones_applies_and_restamps() {
    if !ready("a_pending_file_beside_applied_ones_applies_and_restamps") {
        return;
    }
    let mut world = World::remote(&[INIT, MORE, LATER], &[INIT, MORE]);
    let previous = world.stamp.clone();
    world.respond("names.after", 0, names(filenames(&[INIT, MORE, LATER])));

    let (shell, port) = world.both();
    diff(
        "a pending file beside applied ones",
        &shell,
        &port,
        &Diagnostics::Quiet,
    );
    let stamp = digest(&[INIT, MORE, LATER]);
    assert!(
        shell
            .out()
            .contains("    2 applied, 1 pending migration file(s)\n"),
        "the partition line changed shape: {:?}",
        shell.out()
    );
    assert!(
        shell
            .out()
            .contains(&format!("remote migrate OK (stamp {stamp})\n"))
    );
    assert_eq!(shell.status, Some(0));
    assert!(shell.stderr.is_empty());
    assert_eq!(shell.stamped(), format!("{stamp}\n"));
    assert_ne!(
        shell.stamped(),
        previous,
        "the refreshed stamp covers every file, not the applied ones"
    );
    // Five, not four: the table already exists, so each read of the
    // applied set costs the has-table probe AND the names query. Only
    // the never-migrated database short-circuits its first read.
    assert_eq!(
        shell.log.len(),
        5,
        "read, apply, then read again: {:?}",
        shell.log
    );
}

/// The harness's own guard, asserted once on its own: every scenario
/// compares this checkout's `registry/migrations-applied` across its
/// two runs, and this is the file it reads. A rename would otherwise
/// turn that comparison into a panic in every test at once.
#[test]
fn the_real_stamp_is_never_touched() {
    let stamp = xtask_registry_admin::registry_dir().join("migrations-applied");
    assert!(
        stamp.is_file(),
        "{} is gone, so the guard every scenario runs cannot read it",
        show(&stamp)
    );
    assert!(!real_stamp().is_empty(), "this checkout's stamp is empty");
}
