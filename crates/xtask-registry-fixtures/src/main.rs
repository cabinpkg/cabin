//! Command-line shim for the fixture generator; the generator itself
//! lives in the library.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Parser;
use clap::error::ErrorKind;

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

// The output directory is not a required argument: `${1:?...}` refused
// an empty path as well as an absent one, with one message, and clap's
// required-argument wording covers only the absent half.
//
// `OsString`, not `PathBuf`: both take the byte sequences the shell
// this replaces accepted as a path, but clap's `PathBuf` parser refuses
// an empty value itself, before the refusal below can name it.
#[derive(Parser)]
struct Cli {
    out: Option<OsString>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return refuse(&error),
    };
    match run(cli.out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `-h`/`--help` answers with this usage on stdout; every other parse
/// failure is the exit 1 the shell took, where clap's own default is 2.
fn refuse(error: &clap::Error) -> ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ErrorKind::DisplayVersion => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        _ => {
            let _ = error.print();
            ExitCode::FAILURE
        }
    }
}

fn run(out: Option<OsString>) -> Result<()> {
    // Empty as well as absent: `${1:?...}` refused both, and an empty
    // path would package into the working directory.
    let Some(out) = out.filter(|path| !path.is_empty()) else {
        bail!("no output directory named\n\n{USAGE}");
    };
    xtask_registry_fixtures::generate(Path::new(&out))
}
