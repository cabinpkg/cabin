//! The internal `cabin stamp` command - a shell-free witness writer.
//!
//! A `cabin check` syntax-only compile (`-fsyntax-only` / `/Zs`) produces
//! no object, so Ninja needs a witness file to track the edge
//! incrementally.  The generated syntax-check rule therefore runs the
//! compiler through `cabin stamp <file> -- <argv…>`, which spawns the
//! compiler directly - no shell, so build paths containing `&` / `|` /
//! `()` / `$` need no metacharacter escaping - and writes the witness
//! only on a zero exit.
//!
//! `cabin stamp` is dispatched in [`crate::run`] *before* clap, so it
//! stays out of the user-facing command surface entirely: it never
//! appears in `--help`, `--list`, shell completions, or man pages, and so
//! needs no special-case filtering in any of them.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::anyhow;
use cabin_core::ColorChoice;
use clap::Parser;

/// The `argv[1]` token that selects the internal witness writer.
const COMMAND: &str = "stamp";

/// If this process was invoked as `cabin stamp …`, run the witness
/// writer and return its exit code; otherwise return `None` so normal
/// CLI parsing proceeds. `argv` is the full process argument vector,
/// including `argv[0]`.
pub(crate) fn dispatch(argv: &[OsString]) -> Option<ExitCode> {
    let operands = match argv.get(1) {
        Some(first) if first == COMMAND => &argv[2..],
        _ => return None,
    };
    // Parse with a synthetic program name so any usage / error text reads
    // as `cabin stamp …` rather than the bare binary path.
    let program_name = OsString::from("cabin stamp");
    let parsed = match StampArgs::try_parse_from(
        std::iter::once(program_name).chain(operands.iter().cloned()),
    ) {
        Ok(parsed) => parsed,
        Err(err) => err.exit(),
    };
    Some(match execute(&parsed) {
        Ok(code) => code,
        // `cabin stamp` is Ninja-invoked plumbing; route genuine failures
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
/// create the witness `FILE`.
#[derive(Parser)]
struct StampArgs {
    /// Witness file to create when the command exits zero.
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

/// Run `args.command` and, on a zero exit, create `args.file`.
///
/// The command's stdout / stderr are inherited so Ninja still surfaces
/// the compiler's diagnostics; the witness is written only on success, so
/// a failed check leaves the edge dirty and Ninja re-runs it next time.  A
/// non-zero command exit is propagated verbatim and stays silent (the
/// compiler already printed its own diagnostics); only Cabin-level
/// failures (un-spawnable program, unwritable witness) surface as a
/// rendered error.
fn execute(args: &StampArgs) -> anyhow::Result<ExitCode> {
    let Some((program, rest)) = args.command.split_first() else {
        // `required = true` already makes clap reject an empty argv; this
        // is defensive only.
        anyhow::bail!("cabin stamp: missing command after `--`");
    };
    match std::process::Command::new(program).args(rest).status() {
        Ok(status) if status.success() => {
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
        Err(err) => Err(anyhow!("cabin stamp: failed to run {program}: {err}")),
    }
}
