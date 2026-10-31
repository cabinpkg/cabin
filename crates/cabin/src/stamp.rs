//! The internal `cabin stamp`, `cabin recreate`, and `cabin depfile`
//! commands - shell-free Ninja plumbing.
//!
//! A `cabin check` syntax-only compile (`-fsyntax-only` / `/Zs`) produces
//! no object, so Ninja needs a witness file to track the edge
//! incrementally.  The generated syntax-check rule therefore runs the
//! compiler through `cabin stamp <file> -- <argv…>`, which spawns the
//! compiler directly - no shell, so build paths containing `&` / `|` /
//! `()` / `$` need no metacharacter escaping - and writes the witness
//! only on a zero exit.
//!
//! The GNU-dialect archive rule runs `ar crs` through `cabin recreate
//! <archive> -- <argv…>`, which removes the archive before spawning the
//! archiver so it never updates the previous build's archive in place.
//!
//! A compile whose depfile Ninja's parser would misread (see
//! `cabin_ninja::writer`) runs through `cabin depfile <depfile> -- <argv…>`,
//! which prints the depfile's prerequisites in the line format of
//! Ninja's `deps = msvc` after the compiler succeeds.
//!
//! All three are dispatched in [`crate::run`] *before* clap, so they stay out
//! of the user-facing command surface entirely: they never appear in
//! `--help`, `--list`, shell completions, or man pages, and so need no
//! special-case filtering in any of them.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::anyhow;
use cabin_core::ColorChoice;
use clap::Parser;

/// The `argv[1]` tokens that select the internal commands.
const STAMP: &str = "stamp";
const RECREATE: &str = "recreate";
const DEPFILE: &str = "depfile";

/// If this process was invoked as `cabin stamp …`, `cabin recreate …`, or
/// `cabin depfile …`,
/// run that command and return its exit code; otherwise return `None` so
/// normal CLI parsing proceeds. `argv` is the full process argument
/// vector, including `argv[0]`.
pub(crate) fn dispatch(argv: &[OsString]) -> Option<ExitCode> {
    let Some(verb @ (STAMP | RECREATE | DEPFILE)) = argv.get(1).and_then(|first| first.to_str())
    else {
        return None;
    };
    let operands = &argv[2..];
    // Parse with a synthetic program name so any usage / error text reads
    // as `cabin stamp …` (and so on) rather than the bare binary path.
    let program_name = OsString::from(format!("cabin {verb}"));
    let parsed = match StampArgs::try_parse_from(
        std::iter::once(program_name).chain(operands.iter().cloned()),
    ) {
        Ok(parsed) => parsed,
        Err(err) => err.exit(),
    };
    Some(match execute(verb, &parsed) {
        Ok(code) => code,
        // These commands are Ninja-invoked plumbing; route genuine failures
        // through the same diagnostic channel as the rest of the CLI
        // (never a bare `eprintln!`).  Color is off because Ninja captures
        // the output.
        Err(err) => {
            crate::error_rendering::render_error(&err, ColorChoice::Never);
            ExitCode::FAILURE
        }
    })
}

/// `cabin stamp <FILE> -- <COMMAND>…`: run `COMMAND`; on a zero exit,
/// create the witness `FILE`.  `cabin recreate <FILE> -- <COMMAND>…`:
/// remove `FILE`, then run `COMMAND`, which writes it from scratch.
/// `cabin depfile <FILE> -- <COMMAND>…`: run `COMMAND`; on a zero exit,
/// print the prerequisites of the depfile `FILE` it wrote.
#[derive(Parser)]
struct StampArgs {
    /// Witness file to create (`stamp`), output to remove (`recreate`),
    /// or depfile to report (`depfile`).
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// The command to run, taken verbatim from after `--`.
    #[arg(
        last = true,
        allow_hyphen_values = true,
        required = true,
        value_name = "ARGV"
    )]
    command: Vec<String>,
}

/// Run `args.command` and, on a zero exit, create `args.file` for
/// `stamp` or report it for `depfile`.  `recreate` removes `args.file`
/// first.
///
/// The command's stdout / stderr are inherited so Ninja still surfaces
/// the compiler's diagnostics; the witness is written only on success, so
/// a failed check leaves the edge dirty and Ninja re-runs it next time.  A
/// non-zero command exit is propagated verbatim and stays silent (the
/// compiler already printed its own diagnostics); only Cabin-level
/// failures (un-spawnable program, unwritable witness, unremovable
/// output) surface as a rendered error.
fn execute(verb: &str, args: &StampArgs) -> anyhow::Result<ExitCode> {
    let Some((program, rest)) = args.command.split_first() else {
        // `required = true` already makes clap reject an empty argv; this
        // is defensive only.
        anyhow::bail!("cabin {verb}: missing command after `--`");
    };
    if verb == RECREATE
        && let Err(err) = std::fs::remove_file(&args.file)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        anyhow::bail!(
            "cabin recreate: failed to remove {}: {err}",
            args.file.display()
        );
    }
    match std::process::Command::new(program).args(rest).status() {
        Ok(status) if status.success() => {
            match verb {
                STAMP => std::fs::write(&args.file, []).map_err(|err| {
                    anyhow!(
                        "cabin stamp: failed to write {}: {err}",
                        args.file.display()
                    )
                })?,
                DEPFILE => report_depfile(&args.file)?,
                _ => {}
            }
            Ok(ExitCode::SUCCESS)
        }
        Ok(status) => Ok(ExitCode::from(
            u8::try_from(status.code().unwrap_or(1)).unwrap_or(1),
        )),
        Err(err) => Err(anyhow!("cabin {verb}: failed to run {program}: {err}")),
    }
}

/// Print the [`cabin_ninja::msvc_deps_report`] of `depfile`, which
/// Ninja's `deps = msvc` records and removes from the edge's output.  The
/// paths are written as raw bytes because Ninja reads them as bytes.
fn report_depfile(depfile: &Path) -> anyhow::Result<()> {
    let content = std::fs::read(depfile)
        .map_err(|err| anyhow!("cabin depfile: failed to read {}: {err}", depfile.display()))?;
    let report = cabin_ninja::msvc_deps_report(&content);
    std::io::stdout().write_all(&report).map_err(|err| {
        anyhow!(
            "cabin depfile: failed to report {}: {err}",
            depfile.display()
        )
    })
}
