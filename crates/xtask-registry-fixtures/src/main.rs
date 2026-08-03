//! Command-line shim for the fixture generator.  Argument parsing is
//! hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the generator itself lives in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo gen-fixtures <out-dir>

Build the in-tree `cabin` binary and package the publish-conformance
fixtures into <out-dir>, three archive + canonical-metadata pairs:

  smoke-nodep-0.1.0.zip         no dependencies
  smoke-withdep-0.2.0.zip       a dependency + standards + links blocks
  smoke-withupstream-0.3.0.zip  a [package.upstream] block

options:
  -h, --help  show this help
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut out = None;
    // `args_os`, not `args`: the shell this replaces took any byte
    // sequence as the output path, and `args` panics on one.
    for argument in std::env::args_os().skip(1) {
        match argument.to_str() {
            Some("-h" | "--help") => {
                print!("{USAGE}");
                return Ok(());
            }
            _ if out.is_none() => out = Some(PathBuf::from(argument)),
            _ => bail!(
                "unexpected argument: {}\n\n{USAGE}",
                argument.to_string_lossy()
            ),
        }
    }
    // Empty as well as absent: `${1:?...}` refused both, and an empty
    // path would package into the working directory.
    let Some(out) = out.filter(|path| !path.as_os_str().is_empty()) else {
        bail!("no output directory named\n\n{USAGE}");
    };
    xtask_registry_fixtures::generate(&out)
}
