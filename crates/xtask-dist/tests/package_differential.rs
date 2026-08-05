//! The whole-step differential for release packaging: the shell it
//! replaces and the port, run over one corpus of throwaway repository
//! roots and compared on the `PACKAGE_PATH` each announces, the archive
//! that path names, the staging directory left behind, and the exit
//! status.
//!
//! `tests/fixtures/package.sh.orig` is the original, byte for byte: the
//! `run:` block of the "Package binary" step of `.github/workflows/dist.yml`
//! as it stood on `main` at `bcae35859`, dedented 10 spaces, `sha256`
//! `2a3ccfac359d3f28a601c4788aa940015d89235e193894e5211a5c445df1d790`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # The one edit the runner itself makes
//!
//! The block is not what a runner executes. It carries a
//! `${{ matrix.target }}` template placeholder, and GitHub expands
//! templates into the step's script *before* handing it to `bash` - by
//! the time a shell sees the block, `target="${{ matrix.target }}"` is
//! already `target="x86_64-unknown-linux-gnu"`. So the fixture is
//! vendored with the placeholder intact and [`World::new`] performs
//! exactly that substitution into a derived per-scenario script. The
//! derived script is what runs; the fixture stays pristine, and the
//! harness asserts both halves of the expansion - that the placeholder
//! was there to replace, and that no `${{` survived it - rather than
//! assuming a `replace` matched.
//!
//! The derived script is written beside the two repository roots rather
//! than inside either, so neither side's `tar` can see it.
//!
//! # The invocation
//!
//! The step declares `shell: bash`, for which GitHub runs
//! `bash --noprofile --norc -eo pipefail {0}`. That is reproduced
//! verbatim. Most of it is redundant against the block's own opening
//! `set -euo pipefail` - but not all: `-u` is the block's alone, so a
//! reader who assumes the runner supplied it would misread which
//! unset variable is fatal.
//!
//! # The seam: `GITHUB_ENV` on one side, stdout on the other
//!
//! The shell's last act is `echo "PACKAGE_PATH=${package_path}" >>
//! "$GITHUB_ENV"`. The port instead prints the path on stdout and the
//! workflow does that plumbing inline, so the two sides cannot be
//! compared on stdout at all: the shell's is empty by design and the
//! port's carries the payload.
//!
//! [`Outcome::reported`] is the bridge. For the shell it is the whole
//! `GITHUB_ENV` file; for the port it is the `PACKAGE_PATH=` line the
//! workflow will write from its stdout. Asserting those two strings
//! equal *is* the contract the workflow swap preserves, and it is the
//! first thing every scenario checks.
//!
//! It also pins the ordering on the failing paths. The shell writes
//! `GITHUB_ENV` only at the very end, after the archive exists, so any
//! earlier death leaves the file empty - there is no half-announced
//! package. The port's equivalent is printing nothing. Both are
//! asserted, in that shape, by [`Contract::Refused`].
//!
//! # What is compared, and where the comparison stops
//!
//! **Not the archive's bytes.** `tar` embeds mtimes, uids and modes,
//! and the two sides package separately staged copies made moments
//! apart; identical bytes were never on offer. The contract is the
//! entry list and what the entries contain, so [`archive`] lists the
//! archive with `tar -tf`, extracts it, and compares a map of entry
//! name to file bytes. Directory entries keep their trailing `/` and
//! map to no bytes, which is deliberate: an archive that dropped the
//! directory entry - what a port appending files one by one would
//! produce - differs in the key set and fails here even though every
//! file inside it matches.
//!
//! **Not the failure wording.** One side is diagnosed by `cp`, the
//! other by the port's own error type. [`Contract::Refused`] narrows to
//! "both died, both said something, neither announced a path".
//!
//! **What a failure leaves staged, though, is.** It is where the
//! block's least obvious detail shows: `cp README.md LICENSE
//! "$package/"` is one `cp` with two
//! sources, so a missing `README.md` still leaves `LICENSE` staged -
//! `cp` continues through its remaining operands - where a port copying
//! one file at a time would stop after the first failure. The binary is
//! its own `cp`, so its absence stages nothing at all.
//! [`a_missing_input_dies_before_announcing_anything`] pins the sorted
//! listing of both sides against each other and against what the shell
//! was observed to leave.
//!
//! **Not the Windows leg.** The block's `zip` branch shells out to
//! `powershell`; this suite is `#![cfg(unix)]` and covers the `tar`
//! branch only. Version derivation, staging and the failure paths are
//! the same code on both legs, so what goes uncovered is the archive
//! step alone.
//!
//! # The port may only use its arguments
//!
//! The shell reads `GITHUB_REF_NAME`, `GITHUB_REF_TYPE`, `GITHUB_SHA`
//! and `RUNNER_OS` from the environment. The port takes the same values
//! as flags, so [`World::both`] removes all of them from the port's
//! environment rather than passing them along. A port that quietly read
//! the environment instead of its arguments would agree with the shell
//! here if it were handed both; stripped, it can only agree by using
//! what it was given.
//!
//! # Two answers taken from the shell rather than from reasoning
//!
//! Both were run before either side was written, because both are
//! places a port would plausibly "fix" the original:
//!
//! - **A `$GITHUB_SHA` shorter than twelve characters.** `${VAR::12}`
//!   is a slice, not a requirement: bash yields whatever is there.
//!   `abc123` gives `dev-abc123`, and an empty SHA gives a bare `dev-`.
//!   Neither is an error.
//! - **A ref name containing `/`,** which a tag may. The version goes
//!   into the package *name*, so `release/x` makes the name
//!   `cabin-release/x-<triple>`: `mkdir -p` creates a nested
//!   `cabin-release/` directory, `tar` packages the nested path - every
//!   entry carries the `cabin-release/` prefix - and `package_path`
//!   names an archive written *inside* that directory. Odd, and
//!   faithfully reproduced;
//!   [`a_slashed_tag_name_nests_the_whole_package`] is what holds a
//!   port to it.
//!
//! Every test skips rather than fails when a tool it needs is missing,
//! and the harness's own failures panic.
//!
//! # Negative proofs
//!
//! Both were run by hand, then reverted, with the fixture's `sha256`
//! re-checked afterwards. Both perturb one side only, which is the
//! shape of a real divergence:
//!
//! - **The reported-path equivalence discriminates.** Hard-coding the
//!   port's `--ref-type` to `tag` in [`World::both`], which is a port
//!   that misread one flag, failed exactly the three tests whose
//!   scenarios use a ref that is not a tag -
//!   [`a_ref_that_is_not_a_tag_falls_back_to_the_sha`],
//!   [`a_short_sha_is_taken_for_whatever_it_has`] and
//!   [`a_slashed_branch_name_is_never_consulted`] - every one of them
//!   on `the two sides announced different packages` (`shell:
//!   PACKAGE_PATH=cabin-dev-0123456789ab-... / port:
//!   PACKAGE_PATH=cabin-main-...`). The three scenarios that do pass a
//!   tag stayed green, so the catch is the divergence itself and not
//!   collateral.
//! - **The archive comparison is load-bearing on its own.** Dropping
//!   `README.md` from the fixture's `cp README.md LICENSE "$package/"`,
//!   which is a shell that stages one file fewer while naming the
//!   identical archive, failed all five packaging scenarios on `the two
//!   archives hold different entries` - *after* the reported path
//!   compared equal, which is the point: a staging divergence is caught
//!   by itself rather than only when it happens to change the path.
//!   [`a_missing_input_dies_before_announcing_anything`] failed too,
//!   and instructively: with `README.md` no longer copied, removing it
//!   stops being fatal, so the shell packaged where the port still
//!   refused and the reported-path assertion caught that instead
//!   (`shell: PACKAGE_PATH=cabin-0.14.0-... / port: ""`).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_fs::TempDir;

/// The triple every scenario packages for. It is a matrix entry rather
/// than the host's own: the step only ever interpolates it into paths
/// and names, and pinning it keeps the expected archive names the same
/// on every machine.
const TRIPLE: &str = "x86_64-unknown-linux-gnu";

/// What the runner replaces before `bash` ever sees the block.
const PLACEHOLDER: &str = "${{ matrix.target }}";

/// Non-Windows is all the `tar` branch asks. Reported as the host, so a
/// reader of a failure sees the value the step actually compared.
const RUNNER_OS: &str = if cfg!(target_os = "macos") {
    "macOS"
} else {
    "Linux"
};

/// The three files a package is made of. The binary is deliberately not
/// valid UTF-8: the real `cabin` is irrelevant here, but bytes that
/// survive a lossy round-trip would not prove the archive carried them.
const BINARY: &[u8] = b"\x7fELF\x00\xff\xfe not a real binary\n";
const README: &[u8] = b"# Cabin\n\nfixture readme\n";
const LICENSE: &[u8] = b"fixture license\n";

/// A forty-character `$GITHUB_SHA`, of which a dev version keeps the
/// first twelve.
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA12: &str = "0123456789ab";

/// How far a scenario's two sides can be held to each other.
enum Contract {
    /// Both packaged. The reported path, the archive's entries and
    /// contents, the staging directory and the exit status are all
    /// compared.
    Packaged,
    /// Both refused. `cp` diagnosed one and the port's error type the
    /// other, so only the shape is compared: died, said something,
    /// announced nothing, wrote no archive.
    Refused,
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    /// The root the side ran in, and under which it staged.
    root: PathBuf,
    /// The `PACKAGE_PATH=<path>\n` line this side is responsible for,
    /// empty when it announced nothing: the shell's `GITHUB_ENV` file
    /// as written, and for the port the line the workflow writes from
    /// its stdout.
    reported: String,
}

impl Outcome {
    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// The path out of [`Outcome::reported`], which both sides must
    /// agree on before this is ever consulted.
    fn package_path(&self) -> &str {
        self.reported
            .strip_prefix("PACKAGE_PATH=")
            .and_then(|line| line.strip_suffix('\n'))
            .unwrap_or_else(|| panic!("no package path was announced: {:?}", self.reported))
    }

    /// The archive the side named, as entry name to file bytes.
    fn archive(&self) -> BTreeMap<String, Vec<u8>> {
        let path = self.root.join(self.package_path());
        assert!(
            path.is_file(),
            "the announced archive is not there: {}",
            show(&path)
        );
        archive(&path, &self.root.join("unpacked"))
    }

    /// The directory the side staged into, which the step leaves on
    /// disk. It is the archive's path without the `.tar.xz`, so a
    /// slashed name nests it exactly as the archive is nested.
    fn staging(&self) -> PathBuf {
        let package = self
            .package_path()
            .strip_suffix(".tar.xz")
            .expect("the unix leg names a .tar.xz");
        self.root.join(package)
    }
}

/// One scenario: two byte-identical repository roots, one per side, and
/// the derived script the shell side runs in its own.
struct World {
    dir: TempDir,
    script: PathBuf,
    /// Removed from both roots after they are populated, which is how
    /// the failing scenarios are built.
    missing: Option<String>,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new().expect("a scratch directory");

        let fixture = fs::read_to_string(fixture()).expect("the vendored original");
        assert!(
            fixture.contains(PLACEHOLDER),
            "the fixture no longer carries {PLACEHOLDER}, so the runner's expansion is being \
             reproduced against nothing"
        );
        let derived = fixture.replace(PLACEHOLDER, TRIPLE);
        assert!(
            !derived.contains("${{"),
            "a template placeholder survived the expansion: {derived}"
        );
        let script = dir.path().join("package.sh");
        fs::write(&script, derived).expect("the derived script");

        Self {
            dir,
            script,
            missing: None,
        }
    }

    /// Runs both sides over identical roots and returns what each did.
    fn both(&self, ref_name: &str, ref_type: Option<&str>, sha: &str) -> (Outcome, Outcome) {
        let shell_env = self.dir.path().join("shell.env");
        fs::write(&shell_env, b"").expect("the shell's GITHUB_ENV file");

        let mut bash = Command::new("bash");
        bash.args(["--noprofile", "--norc", "-eo", "pipefail"])
            .arg(&self.script)
            .env("GITHUB_REF_NAME", ref_name)
            .env("GITHUB_SHA", sha)
            .env("RUNNER_OS", RUNNER_OS)
            .env("GITHUB_ENV", &shell_env);
        // `${GITHUB_REF_TYPE:-}` tolerates a set and an unset variable
        // alike; the port is handed `""` for the unset one.
        if let Some(kind) = ref_type {
            bash.env("GITHUB_REF_TYPE", kind);
        } else {
            bash.env_remove("GITHUB_REF_TYPE");
        }
        let shell = self.side("shell", bash, |_| {
            fs::read_to_string(&shell_env).expect("the shell's GITHUB_ENV file")
        });

        let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-dist"));
        ported.args([
            "package",
            "--target",
            TRIPLE,
            "--ref-name",
            ref_name,
            "--ref-type",
            ref_type.unwrap_or_default(),
            "--sha",
            sha,
        ]);
        for leaked in [
            "GITHUB_REF_NAME",
            "GITHUB_REF_TYPE",
            "GITHUB_SHA",
            "GITHUB_ENV",
            "RUNNER_OS",
        ] {
            ported.env_remove(leaked);
        }
        let port = self.side("port", ported, |stdout| {
            if stdout.is_empty() {
                String::new()
            } else {
                format!("PACKAGE_PATH={}", String::from_utf8_lossy(stdout))
            }
        });

        (shell, port)
    }

    /// Populates one side's root, runs it there, and reads back what it
    /// announced.
    fn side(
        &self,
        name: &str,
        mut command: Command,
        announced: impl Fn(&[u8]) -> String,
    ) -> Outcome {
        let root = self.dir.path().join(name);
        let built = root.join("target").join(TRIPLE).join("release");
        fs::create_dir_all(&built).expect("the build output directory");
        fs::write(built.join("cabin"), BINARY).expect("the built binary");
        fs::write(root.join("README.md"), README).expect("the readme");
        fs::write(root.join("LICENSE"), LICENSE).expect("the license");

        if let Some(gone) = &self.missing {
            let path = root.join(gone);
            if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            }
            .unwrap_or_else(|why| panic!("removing {} for the scenario: {why}", show(&path)));
        }

        let produced = command
            .current_dir(&root)
            .output()
            .expect("running one side of the scenario");

        Outcome {
            reported: announced(&produced.stdout),
            stdout: produced.stdout,
            stderr: produced.stderr,
            status: produced.status.code(),
            root,
        }
    }
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/package.sh.orig")
}

/// Lists `path` with `tar -tf`, unpacks it under `into`, and pairs each
/// entry with its bytes. Both sides are read by the same `tar`, so a
/// difference in the result is a difference in the archives.
fn archive(path: &Path, into: &Path) -> BTreeMap<String, Vec<u8>> {
    let listing = tar(&["-tf", &show(path)]);
    fs::create_dir_all(into).expect("somewhere to unpack");
    tar(&["-xf", &show(path), "-C", &show(into)]);

    listing
        .lines()
        .map(|entry| {
            let bytes = if entry.ends_with('/') {
                Vec::new()
            } else {
                let unpacked = into.join(entry);
                fs::read(&unpacked)
                    .unwrap_or_else(|why| panic!("reading {}: {why}", show(&unpacked)))
            };
            (entry.to_owned(), bytes)
        })
        .collect()
}

fn tar(args: &[&str]) -> String {
    let done = Command::new("tar")
        .args(args)
        .output()
        .expect("running the harness's own tar");
    assert!(
        done.status.success(),
        "the harness's `tar {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&done.stderr)
    );
    String::from_utf8_lossy(&done.stdout).into_owned()
}

/// The names `directory` holds, sorted.
fn staged(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap_or_else(|why| panic!("reading {}: {why}", show(directory)))
        .map(|entry| {
            entry
                .expect("a staged entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn ready(test: &str) -> bool {
    // `xz` is listed for GNU tar, which shells out to it for `-J`;
    // bsdtar links liblzma instead and would pass without it.
    for tool in ["bash", "tar", "xz"] {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, contract: &Contract) {
    assert!(
        shell.reported == port.reported,
        "{case}: the two sides announced different packages\nshell: {:?}\nport:  {:?}",
        shell.reported,
        port.reported
    );

    match *contract {
        Contract::Packaged => {
            assert_eq!(shell.status, port.status, "{case}: exit status");
            assert_eq!(shell.status, Some(0), "{case}: a packaged run succeeds");
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert!(
                    outcome.stderr.is_empty(),
                    "{case}: the {side} diagnosed a run that worked: {}",
                    outcome.err()
                );
            }
            assert!(
                shell.stdout.is_empty(),
                "{case}: the shell announces through GITHUB_ENV, not stdout: {}",
                shell.stdout.escape_ascii()
            );

            let (staged, ported) = (shell.archive(), port.archive());
            assert!(
                staged.keys().eq(ported.keys()),
                "{case}: the two archives hold different entries\nshell: {:#?}\nport:  {:#?}",
                staged.keys().collect::<Vec<_>>(),
                ported.keys().collect::<Vec<_>>()
            );
            for (entry, bytes) in &staged {
                assert!(
                    ported[entry] == *bytes,
                    "{case}: the two archives differ at {entry}"
                );
            }

            for (side, outcome) in [("shell", shell), ("port", port)] {
                let staging = outcome.staging();
                assert!(
                    staging.is_dir(),
                    "{case}: the {side} did not leave {} staged",
                    show(&staging)
                );
            }
        }
        Contract::Refused => {
            assert!(
                shell.reported.is_empty(),
                "{case}: a step that died announced a package: {:?}",
                shell.reported
            );
            for (side, outcome) in [("shell", shell), ("port", port)] {
                assert_ne!(outcome.status, Some(0), "{case}: the {side} must die");
                assert!(
                    !outcome.stderr.is_empty(),
                    "{case}: the {side} died without a diagnostic"
                );
            }
            assert!(
                port.stdout.is_empty(),
                "{case}: the port printed a path it could not have packaged: {}",
                port.stdout.escape_ascii()
            );
            assert!(
                shell.stdout.is_empty(),
                "{case}: the shell wrote to stdout: {}",
                shell.stdout.escape_ascii()
            );
        }
    }
}

/// What a package of `version` holds, as [`archive`] reports it.
fn expected(version: &str) -> BTreeMap<String, Vec<u8>> {
    let package = format!("cabin-{version}-{TRIPLE}");
    [
        (format!("{package}/"), Vec::new()),
        (format!("{package}/cabin"), BINARY.to_vec()),
        (format!("{package}/README.md"), README.to_vec()),
        (format!("{package}/LICENSE"), LICENSE.to_vec()),
    ]
    .into_iter()
    .collect()
}

/// A tag is the one ref kind whose name becomes the version, and the
/// whole packaging contract in one scenario: the name, the three
/// entries, their bytes, and the staging directory left behind.
#[test]
fn a_tag_names_the_package_after_itself() {
    if !ready("a_tag_names_the_package_after_itself") {
        return;
    }
    let world = World::new();
    let (shell, port) = world.both("0.14.0", Some("tag"), SHA);

    diff("a tag ref", &shell, &port, &Contract::Packaged);
    assert_eq!(
        shell.reported,
        format!("PACKAGE_PATH=cabin-0.14.0-{TRIPLE}.tar.xz\n")
    );
    assert_eq!(
        shell.archive(),
        expected("0.14.0"),
        "the archive holds the binary, the readme and the license, and nothing else"
    );
}

/// Anything that is not exactly `tag` - a branch, a ref type the runner
/// never set, and the two near misses a case-insensitive comparison
/// would let through - derives the version from the SHA instead, and
/// the ref name is not consulted at all.
#[test]
fn a_ref_that_is_not_a_tag_falls_back_to_the_sha() {
    if !ready("a_ref_that_is_not_a_tag_falls_back_to_the_sha") {
        return;
    }
    for (case, ref_type) in [
        ("a branch", Some("branch")),
        ("no ref type at all", None),
        ("a capitalized Tag", Some("Tag")),
        ("the plural tags", Some("tags")),
    ] {
        let world = World::new();
        let (shell, port) = world.both("main", ref_type, SHA);

        diff(case, &shell, &port, &Contract::Packaged);
        assert_eq!(
            shell.reported,
            format!("PACKAGE_PATH=cabin-dev-{SHA12}-{TRIPLE}.tar.xz\n"),
            "{case}"
        );
        assert_eq!(shell.archive(), expected(&format!("dev-{SHA12}")), "{case}");
    }
}

/// `${GITHUB_SHA::12}` is a slice and not a requirement. A SHA shorter
/// than twelve characters is taken whole, and an empty one leaves a
/// bare `dev-`; neither is an error on either side.
#[test]
fn a_short_sha_is_taken_for_whatever_it_has() {
    if !ready("a_short_sha_is_taken_for_whatever_it_has") {
        return;
    }
    for (case, sha, version) in [
        ("six characters", "abc123", "dev-abc123"),
        ("exactly twelve", SHA12, "dev-0123456789ab"),
        ("nothing at all", "", "dev-"),
    ] {
        let world = World::new();
        let (shell, port) = world.both("main", Some("branch"), sha);

        diff(case, &shell, &port, &Contract::Packaged);
        assert_eq!(
            shell.reported,
            format!("PACKAGE_PATH=cabin-{version}-{TRIPLE}.tar.xz\n"),
            "{case}"
        );
        assert_eq!(shell.archive(), expected(version), "{case}");
    }
}

/// A tag may contain `/`, and the version goes into a *path*. The
/// package name becomes `cabin-release/x-<triple>`, so `mkdir -p`
/// nests, every archive entry carries the `cabin-release/` prefix, and
/// the archive itself is written inside that directory. Pinned from
/// what the shell does, not from what it ought to do.
#[test]
fn a_slashed_tag_name_nests_the_whole_package() {
    if !ready("a_slashed_tag_name_nests_the_whole_package") {
        return;
    }
    let world = World::new();
    let (shell, port) = world.both("release/x", Some("tag"), SHA);

    diff("a slashed tag", &shell, &port, &Contract::Packaged);
    assert_eq!(
        shell.reported,
        format!("PACKAGE_PATH=cabin-release/x-{TRIPLE}.tar.xz\n"),
        "the announced path is itself nested"
    );
    assert_eq!(
        shell.archive(),
        expected("release/x"),
        "the nesting reaches every entry"
    );
    assert!(
        shell.root.join("cabin-release").is_dir(),
        "mkdir -p created the directory the slash implied"
    );
}

/// The same slashed name under a branch ref, where it is never read:
/// the version comes from the SHA, so nothing nests. This is what
/// separates "the name has a slash" from "a tag's name has a slash".
#[test]
fn a_slashed_branch_name_is_never_consulted() {
    if !ready("a_slashed_branch_name_is_never_consulted") {
        return;
    }
    let world = World::new();
    let (shell, port) = world.both("release/x", Some("branch"), SHA);

    diff("a slashed branch", &shell, &port, &Contract::Packaged);
    assert_eq!(
        shell.reported,
        format!("PACKAGE_PATH=cabin-dev-{SHA12}-{TRIPLE}.tar.xz\n")
    );
    assert!(
        !shell.root.join("cabin-release").exists(),
        "a branch's name reached the package name"
    );
}

/// Every input the step copies is required. Each absence kills the step
/// before the archive step, so no archive exists and - the ordering
/// that matters - no `PACKAGE_PATH` was announced. The staging
/// directory survives on both sides, because `mkdir -p` runs first, and
/// what is left in it is `cp`'s own doing: the readme and the license
/// share one `cp`, so either one missing still stages the other, while
/// the binary has a `cp` to itself and its absence stages nothing.
#[test]
fn a_missing_input_dies_before_announcing_anything() {
    if !ready("a_missing_input_dies_before_announcing_anything") {
        return;
    }
    for (case, gone, left) in [
        (
            "no readme",
            "README.md".to_owned(),
            &["LICENSE", "cabin"][..],
        ),
        ("no license", "LICENSE".to_owned(), &["README.md", "cabin"]),
        (
            "no built binary",
            format!("target/{TRIPLE}/release/cabin"),
            &[],
        ),
        ("no target directory", "target".to_owned(), &[]),
    ] {
        let mut world = World::new();
        world.missing = Some(gone);
        let (shell, port) = world.both("0.14.0", Some("tag"), SHA);

        diff(case, &shell, &port, &Contract::Refused);

        let package = format!("cabin-0.14.0-{TRIPLE}");
        for (side, outcome) in [("shell", &shell), ("port", &port)] {
            assert!(
                !outcome.root.join(format!("{package}.tar.xz")).exists(),
                "{case}: the {side} left an archive behind"
            );
            let staging = outcome.root.join(&package);
            assert!(
                staging.is_dir(),
                "{case}: the {side} did not stage before it failed"
            );
            assert_eq!(
                staged(&staging),
                left,
                "{case}: what the {side} left staged"
            );
        }
    }
}
