//! `cabin port` - inspect the bundled foundation-port set.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::term_verbosity::Reporter;

#[derive(Debug, Args)]
pub(crate) struct PortArgs {
    #[command(subcommand)]
    pub command: PortCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PortCommand {
    /// List the bundled foundation ports.
    List,
}

pub(crate) fn port(args: &PortArgs, _reporter: Reporter) -> Result<()> {
    match args.command {
        PortCommand::List => list_builtin_ports(),
    }
}

fn list_builtin_ports() -> Result<()> {
    let mut entries: Vec<_> = cabin_port::builtin::iter().collect();
    entries.sort_by_key(|p| p.name);
    for entry in entries {
        let descriptor =
            cabin_port::parse_port_str(entry.port_toml, std::path::Path::new("<builtin>"))?;
        println!("{} {}", descriptor.name.as_str(), descriptor.version);
    }
    Ok(())
}
