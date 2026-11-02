//! The website demo (`cargo demo-check`, `cargo demo-record`): the
//! `examples/workspace-basic` session that `demo.tape` records into
//! `website/public/demo.gif`.
//!
//! Both subcommands run the session with a given `cabin` binary, from a
//! fresh copy of the example's tracked files in a fresh directory that
//! is also `HOME`, under a cleared environment: no build output, Cabin
//! cache, config, or credential from the caller reaches it. `demo.zsh`
//! logs each command's exit status, and a run passes only when that log
//! lists exactly [`COMMANDS`], every one successful, and the session
//! shows the binary's version line and [`EXPECTED`]. Neither subcommand
//! cuts network access; CI runs `check` without it.
//!
//! `check` pipes [`COMMANDS`] into an interactive zsh. `record` runs VHS
//! on `demo.tape` and checks the log and VHS's text capture of the
//! screen before it copies the GIF to `--out`, so a failed recording
//! never replaces the previous GIF.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, ensure};
use clap::Parser;

/// The commands `demo.tape` types, in order.
const COMMANDS: &[&str] = &[
    "cabin version",
    "cd workspace-basic",
    "bat --paging=never packages/cli/cabin.toml",
    "bat --paging=never packages/cli/src/main.cc",
    "cabin run -p cli",
    "cabin tree -p cli",
];

/// What the session must show: the program's result, then the tree's
/// edge to the local path dependency, which must still be on the final
/// screen of a recording.
const EXPECTED: &[&str] = &["doubled(21) = 42", "└── util v0.1.0 [normal] (workspace)"];

const EXAMPLE: &str = "examples/workspace-basic";
const TAPE: &str = "demo.tape";
const RC: &str = "demo.zsh";
const LOG: &str = "demo.log";
/// The outputs `demo.tape` names, written to the session directory.
const GIF: &str = "demo.gif";
const SCREEN: &str = "demo.txt";

#[derive(Parser)]
enum Cli {
    /// Run the demo session without recording it (`cargo demo-check`).
    Check {
        /// The `cabin` binary the session runs.
        #[arg(long)]
        cabin: PathBuf,
    },
    /// Record `demo.tape` and copy the checked GIF to `--out`
    /// (`cargo demo-record`).
    Record {
        /// The `cabin` binary the session runs.
        #[arg(long)]
        cabin: PathBuf,
        /// Where the GIF goes once the recording passes.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse() {
        Cli::Check { cabin } => check(&cabin),
        Cli::Record { cabin, out } => record(&cabin, &out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn check(cabin: &Path) -> Result<()> {
    let version = version(cabin)?;
    let session = Session::new(cabin)?;
    let mut script = String::from("source \"$DEMO_RC\"\n");
    for command in COMMANDS {
        script.push_str(command);
        script.push('\n');
    }
    let mut zsh = session
        .command("zsh")
        .args(["--no-rcs", "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("running zsh")?;
    zsh.stdin
        .take()
        .context("zsh has no stdin")?
        .write_all(script.as_bytes())
        .context("writing the session to zsh")?;
    let output = zsh.wait_with_output().context("running zsh")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    validate(&session.read(LOG)?, &stdout, &stdout, &version)
}

fn record(cabin: &Path, out: &Path) -> Result<()> {
    let tape = fs::canonicalize(TAPE).with_context(|| format!("resolving {TAPE}"))?;
    let version = version(cabin)?;
    let session = Session::new(cabin)?;
    let status = session
        .command("vhs")
        .arg(tape)
        .status()
        .context("running vhs")?;
    ensure!(status.success(), "vhs failed: {status}");
    promote(&session, &version, out)
}

/// Write the recording's GIF to `out` once its log and screen capture
/// pass [`validate`] and the GIF is whole.
fn promote(session: &Session, version: &str, out: &Path) -> Result<()> {
    let screen = session.read(SCREEN)?;
    validate(&session.read(LOG)?, &screen, &final_frame(&screen), version)?;
    let path = session.home().join(GIF);
    let gif = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    // VHS exits 0 even when FFmpeg fails to encode the GIF.
    ensure!(
        gif.starts_with(b"GIF89a") && gif.ends_with(b";"),
        "VHS left an incomplete GIF"
    );
    fs::write(out, gif).with_context(|| format!("writing {}", out.display()))
}

/// Fail unless `log` lists exactly [`COMMANDS`], each exiting 0, and
/// `output` shows `version` and [`EXPECTED`], the last of them also in
/// `last_screen`.
fn validate(log: &str, output: &str, last_screen: &str, version: &str) -> Result<()> {
    let ran: Vec<&str> = log.lines().collect();
    for (index, command) in COMMANDS.iter().enumerate() {
        let line = ran
            .get(index)
            .with_context(|| format!("the session ended before `{command}` completed"))?;
        ensure!(
            *line == format!("0 {command}"),
            "expected `{command}` to exit 0; the demo log reads `{line}`"
        );
    }
    ensure!(
        ran.len() == COMMANDS.len(),
        "the session ran more than the expected commands: {:?}",
        &ran[COMMANDS.len()..]
    );
    for expected in std::iter::once(version).chain(EXPECTED.iter().copied()) {
        ensure!(
            output.contains(expected),
            "the session never showed `{expected}`"
        );
    }
    if let Some(last) = EXPECTED.last() {
        ensure!(
            last_screen.contains(last),
            "the final screen does not show `{last}`"
        );
    }
    Ok(())
}

/// The last non-blank frame of VHS's text output, whose frames are
/// separated by lines of `─`.
fn final_frame(screen: &str) -> String {
    let mut frames = vec![String::new()];
    for line in screen.lines() {
        if !line.is_empty() && line.chars().all(|c| c == '─') {
            frames.push(String::new());
        } else if let Some(frame) = frames.last_mut() {
            frame.push_str(line);
            frame.push('\n');
        }
    }
    frames
        .into_iter()
        .rev()
        .find(|frame| !frame.trim().is_empty())
        .unwrap_or_default()
}

/// The `cabin version` line the session must show.
fn version(cabin: &Path) -> Result<String> {
    let output = Command::new(cabin)
        .arg("version")
        .output()
        .with_context(|| format!("running {} version", cabin.display()))?;
    ensure!(
        output.status.success(),
        "{} version failed",
        cabin.display()
    );
    let version = String::from_utf8(output.stdout)?.trim().to_owned();
    ensure!(
        version.starts_with("cabin "),
        "unexpected version output {version:?}"
    );
    Ok(version)
}

/// A fresh directory holding the example copy and the `cabin` under
/// test, which is also the session's `HOME` and working directory.
struct Session {
    _dir: tempfile::TempDir,
    /// The directory, canonical so that the prompt abbreviates it as `~`.
    home: PathBuf,
    path: OsString,
    rc: PathBuf,
}

impl Session {
    fn new(cabin: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("cabin-demo")
            .tempdir()
            .context("creating the session directory")?;
        let home = fs::canonicalize(dir.path())?;
        // Tracked files only: an ignored `build/` from an earlier local
        // run would let the session skip the compile it demonstrates.
        let listed = Command::new("git")
            .args(["ls-files", "-z", "--", EXAMPLE])
            .output()
            .context("running git ls-files")?;
        ensure!(listed.status.success(), "git ls-files failed");
        let files = String::from_utf8(listed.stdout)?;
        ensure!(!files.is_empty(), "{EXAMPLE} has no tracked files");
        for file in files.split('\0').filter(|file| !file.is_empty()) {
            let to = home
                .join("workspace-basic")
                .join(Path::new(file).strip_prefix(EXAMPLE)?);
            fs::create_dir_all(to.parent().context("a copied file has no parent")?)?;
            fs::copy(file, &to).with_context(|| format!("copying {file}"))?;
        }

        let bin = home.join(".bin");
        fs::create_dir(&bin)?;
        // Named `cabin` whatever its source is named: the session's
        // commands must not fall through to another `cabin` on `PATH`.
        fs::copy(cabin, bin.join("cabin"))
            .with_context(|| format!("copying {}", cabin.display()))?;
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let path =
            std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&inherited)))?;
        let rc = fs::canonicalize(RC).with_context(|| format!("resolving {RC}"))?;
        Ok(Self {
            _dir: dir,
            home,
            path,
            rc,
        })
    }

    fn home(&self) -> &Path {
        &self.home
    }

    /// `program` run in the session: the caller's `PATH` behind the
    /// copied `cabin`, and its `LANG` so the prompt's glyph renders;
    /// every other variable is cleared.
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.home())
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.home())
            .env("DEMO_RC", &self.rc)
            .env("DEMO_LOG", self.home().join(LOG));
        if let Some(lang) = std::env::var_os("LANG") {
            command.env("LANG", lang);
        }
        command
    }

    fn read(&self, name: &str) -> Result<String> {
        let path = self.home().join(name);
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "cabin 0.18.0";

    fn passing_log() -> String {
        let mut log = String::new();
        for command in COMMANDS {
            log.push_str("0 ");
            log.push_str(command);
            log.push('\n');
        }
        log
    }

    fn passing_output() -> String {
        format!("{VERSION}\n{}\n", EXPECTED.join("\n"))
    }

    fn check(log: &str, output: &str) -> Result<()> {
        validate(log, output, output, VERSION)
    }

    #[test]
    fn a_complete_successful_session_passes() {
        check(&passing_log(), &passing_output()).unwrap();
    }

    #[test]
    fn a_failed_command_fails_despite_the_expected_output() {
        let log = passing_log().replace("0 cabin run", "1 cabin run");
        let err = check(&log, &passing_output()).unwrap_err();
        assert!(err.to_string().contains("cabin run -p cli"), "{err}");
    }

    #[test]
    fn a_session_cut_before_the_last_command_completes_fails() {
        let log = passing_log().replace("0 cabin tree -p cli\n", "");
        let err = check(&log, &passing_output()).unwrap_err();
        assert!(err.to_string().contains("ended before"), "{err}");
    }

    #[test]
    fn missing_expected_output_fails() {
        for missing in std::iter::once(VERSION).chain(EXPECTED.iter().copied()) {
            let output = passing_output().replace(missing, "");
            assert!(check(&passing_log(), &output).is_err(), "{missing}");
        }
    }

    #[test]
    fn a_recording_whose_final_screen_lacks_the_tree_fails() {
        let earlier = passing_output();
        let rule = "─".repeat(20);
        let screen = format!("{earlier}{rule}\n❯ cabin tree -p cli\n{rule}\n\n");
        assert!(final_frame(&screen).starts_with("❯ cabin tree"));
        assert!(validate(&passing_log(), &screen, &final_frame(&screen), VERSION).is_err());
    }

    #[test]
    fn a_failed_recording_keeps_the_previous_gif() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            home: dir.path().to_owned(),
            _dir: dir,
            path: OsString::new(),
            rc: PathBuf::new(),
        };
        let home = session.home();
        fs::write(home.join(GIF), "GIF89a-new;").unwrap();
        fs::write(home.join(SCREEN), passing_output()).unwrap();
        let out = home.join("published.gif");
        fs::write(&out, "previous").unwrap();

        let failed = passing_log().replace("0 cabin run", "127 cabin run");
        fs::write(home.join(LOG), failed).unwrap();
        assert!(promote(&session, VERSION, &out).is_err());
        assert_eq!(fs::read_to_string(&out).unwrap(), "previous");

        fs::write(home.join(LOG), passing_log()).unwrap();
        fs::write(home.join(GIF), "GIF89a-truncated").unwrap();
        assert!(promote(&session, VERSION, &out).is_err());
        assert_eq!(fs::read_to_string(&out).unwrap(), "previous");

        fs::write(home.join(GIF), "GIF89a-new;").unwrap();
        promote(&session, VERSION, &out).unwrap();
        assert_eq!(fs::read_to_string(&out).unwrap(), "GIF89a-new;");
    }
}
