//! The internal `cabin stamp` and `cabin recreate` commands - shell-free
//! Ninja plumbing.
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
//! Both are dispatched in [`crate::run`] *before* clap, so they stay out
//! of the user-facing command surface entirely: they never appear in
//! `--help`, `--list`, shell completions, or man pages, and so need no
//! special-case filtering in any of them.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::anyhow;
use cabin_core::ColorChoice;
use clap::Parser;

/// The `argv[1]` tokens that select the internal commands.
const STAMP: &str = "stamp";
const RECREATE: &str = "recreate";

/// If this process was invoked as `cabin stamp …` or `cabin recreate …`,
/// run that command and return its exit code; otherwise return `None` so
/// normal CLI parsing proceeds. `argv` is the full process argument
/// vector, including `argv[0]`.
pub(crate) fn dispatch(argv: &[OsString]) -> Option<ExitCode> {
    let Some(verb @ (STAMP | RECREATE)) = argv.get(1).and_then(|first| first.to_str()) else {
        return None;
    };
    let operands = &argv[2..];
    // Parse with a synthetic program name so any usage / error text reads
    // as `cabin stamp …` / `cabin recreate …` rather than the bare binary
    // path.
    let program_name = OsString::from(format!("cabin {verb}"));
    let parsed = match StampArgs::try_parse_from(
        std::iter::once(program_name).chain(operands.iter().cloned()),
    ) {
        Ok(parsed) => parsed,
        Err(err) => err.exit(),
    };
    Some(match execute(verb, &parsed) {
        Ok(code) => code,
        // Both commands are Ninja-invoked plumbing; route genuine failures
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
#[derive(Parser)]
struct StampArgs {
    /// Witness file to create (`stamp`) or output to remove (`recreate`).
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

/// Run `args.command` and, for `stamp`, create `args.file` on a zero
/// exit.  `recreate` removes `args.file` first.
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
            if verb == RECREATE {
                return Ok(ExitCode::SUCCESS);
            }
            std::fs::write(&args.file, []).map_err(|err| {
                anyhow!(
                    "cabin stamp: failed to write {}: {err}",
                    args.file.display()
                )
            })?;
            Ok(ExitCode::SUCCESS)
        }
        Ok(status) => Ok(ExitCode::from(
            u8::try_from(status.code().unwrap_or(1)).unwrap_or(1),
        )),
        Err(err) => Err(anyhow!("cabin {verb}: failed to run {program}: {err}")),
    }
}
