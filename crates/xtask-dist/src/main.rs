//! Command-line shim for the dist packaging steps; the steps
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use xtask_dist::checksums;
use xtask_dist::package;

/// The command line the `dist-*` aliases reach.
const USAGE: &str = "\
usage: xtask-dist <COMMAND>

Release packaging steps for .github/workflows/dist.yml, run from a
workflow step through their Cargo aliases.

commands:
  package --target <TRIPLE> --ref-name <NAME> --sha <SHA>
          [--ref-type <TYPE>]
                   stage target/<TRIPLE>/release/cabin, README.md and
                   LICENSE into cabin-<VERSION>-<TRIPLE>/, archive that
                   directory, and print the archive's path to stdout
                   (`cargo dist-package`)
  checksums        write <archive>.sha256 and sha256.sum for every
                   release archive in the working directory and print
                   the summary (`cargo dist-checksums`)

options:
  --target <TRIPLE>  the target triple the release binary was built for
  --ref-name <NAME>  $GITHUB_REF_NAME, the version for a tag build
  --ref-type <TYPE>  $GITHUB_REF_TYPE; anything but `tag` versions the
                     package `dev-<SHA[..12]>` (default: empty)
  --sha <SHA>        $GITHUB_SHA
  -h, --help         show this help
";

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Package(PackageArgs),
    Checksums,
}

/// The flags the workflow step passes, which this shim translates into
/// the library's own [`package::Arguments`]: every value the original
/// spliced from the run's environment arrives here instead, and each
/// is required and may be empty because `${VAR}` under `-u` killed the
/// step on an unset name where a set-but-empty one was a value.
// `args_override_self`: a repeated flag keeps its last value, as the
// parser this replaces did and as a repeated shell assignment would
// have, so a wrapper may supply a default and override it.
#[derive(clap::Parser)]
#[command(args_override_self = true)]
struct PackageArgs {
    #[arg(long)]
    target: String,
    #[arg(long)]
    ref_name: String,
    #[arg(long, default_value = "")]
    ref_type: String,
    #[arg(long)]
    sha: String,
}

impl From<&PackageArgs> for package::Arguments {
    fn from(args: &PackageArgs) -> Self {
        Self {
            target: args.target.clone(),
            ref_name: args.ref_name.clone(),
            ref_type: args.ref_type.clone(),
            sha: args.sha.clone(),
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return refuse(&error),
    };
    match run(&cli.command) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `-h`/`--help` answers with the binary's own usage on stdout, which
/// is what the aliases document, for the subcommands too. Every other
/// parse failure is the exit 1 the step took under `set -e`, where
/// clap's own default is 2.
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

/// Every failure is the exit 1 the step took under `set -e`;
/// `checksums` owns its status outright, because its refusal is the
/// shell's bare sentence rather than this shim's `error:` rendering.
fn run(command: &Command) -> Result<ExitCode> {
    match command {
        Command::Package(args) => package::run(&args.into()).map(|()| ExitCode::SUCCESS),
        Command::Checksums => Ok(checksums::run()),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use clap::error::ErrorKind;

    use super::*;

    fn arguments(target: &str, ref_name: &str, ref_type: &str, sha: &str) -> package::Arguments {
        package::Arguments {
            target: target.to_owned(),
            ref_name: ref_name.to_owned(),
            ref_type: ref_type.to_owned(),
            sha: sha.to_owned(),
        }
    }

    /// The flags as the subcommand receives them, its own name and all.
    fn parse(arguments: &[String]) -> clap::error::Result<package::Arguments> {
        PackageArgs::try_parse_from(
            std::iter::once("package").chain(arguments.iter().map(String::as_str)),
        )
        .map(|args| package::Arguments::from(&args))
    }

    #[test]
    fn the_flags_carry_the_context_the_shell_read_from_the_environment() {
        let given = [
            "--target",
            "t",
            "--ref-name",
            "n",
            "--ref-type",
            "tag",
            "--sha",
            "s",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&given).expect("the arguments"),
            arguments("t", "n", "tag", "s")
        );
    }

    #[test]
    fn an_absent_ref_type_reads_as_empty() {
        let given = [
            "--target",
            "t",
            "--ref-name",
            "n",
            "--sha",
            "0123456789abcdef",
        ]
        .map(str::to_owned);
        // The empty default is what makes the library read this as a
        // non-tag build; `package::tests` covers the version it derives.
        assert_eq!(parse(&given).expect("the arguments").ref_type, "");
    }

    #[test]
    fn an_empty_value_is_a_value() {
        let given = [
            "--target",
            "",
            "--ref-name",
            "",
            "--ref-type",
            "",
            "--sha",
            "",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&given).expect("the arguments"),
            arguments("", "", "", "")
        );
    }

    #[test]
    fn a_missing_required_flag_is_where_the_shell_died_on_an_unset_variable() {
        for flag in ["--target", "--ref-name", "--sha"] {
            let given: Vec<String> = ["--target", "t", "--ref-name", "n", "--sha", "s"]
                .chunks(2)
                .filter(|pair| pair[0] != flag)
                .flat_map(|pair| pair.iter().map(|argument| (*argument).to_owned()))
                .collect();
            let error = parse(&given).expect_err(flag);
            assert!(error.to_string().contains(flag), "{error}");
        }
    }

    /// A repeated flag keeps its LAST value, as the parser this
    /// replaces documented and as a repeated shell assignment would
    /// have: a wrapper may supply a default and then override it.
    #[test]
    fn a_repeated_flag_keeps_its_last_value() {
        let given = [
            "--target",
            "first",
            "--target",
            "second",
            "--ref-name",
            "n",
            "--sha",
            "s",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse(&given).expect("the arguments").target,
            "second",
            "clap's default would refuse the repeat instead"
        );
    }

    #[test]
    fn an_unknown_flag_and_a_valueless_one_are_usage_errors() {
        let unknown = ["--build-dir", "x"].map(str::to_owned);
        assert_eq!(
            parse(&unknown).expect_err("the unknown flag").kind(),
            ErrorKind::UnknownArgument
        );
        let valueless = ["--sha".to_owned()];
        assert_eq!(
            parse(&valueless).expect_err("the valueless flag").kind(),
            ErrorKind::InvalidValue
        );
    }
}
