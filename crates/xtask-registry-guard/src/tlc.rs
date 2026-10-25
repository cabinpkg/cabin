//! Every trap in a TLC witness configuration must be violated
//! (`registry/proofs/README.md`, "Non-vacuity"): each negates a
//! behavior the model must reach, so a trap TLC passes means the model
//! lost that behavior.  TLC stops at the first violation, so each trap
//! runs alone, from a copy of the configuration that keeps only it.
//! TLC reads the configuration, and writes its states, beside the
//! specification, and unpacks its standard modules into
//! `java.io.tmpdir`: all of them land in one scratch directory.
//!
//! Ceiling: the traps are read word by word from the first line naming
//! an `INVARIANT(S)` or `PROPERTY(IES)` header, `\*` comments dropped,
//! and anything but a header or a name there is refused; TLC's verdict
//! is matched in its output (`Invariant <trap> is violated`, or
//! `Action property ...`).

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const SECTIONS: &[&str] = &["INVARIANT", "INVARIANTS", "PROPERTY", "PROPERTIES"];

/// Runs each trap of `config` against `spec` with the TLC in `jar`,
/// and returns one violation for every trap TLC did not report
/// violated.
///
/// # Errors
///
/// An unreadable file, a configuration without traps, or a TLC that
/// cannot be started.
pub fn check(jar: &Path, spec: &Path, config: &Path) -> Result<Vec<String>> {
    let jar = fs::canonicalize(jar).with_context(|| format!("resolving {}", jar.display()))?;
    let text =
        fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    let (head, traps) = split(&text).with_context(|| format!("parsing {}", config.display()))?;
    let spec_name = spec
        .file_name()
        .context("a specification path without a file name")?;
    let mut violations = Vec::new();
    for (section, trap) in traps {
        let scratch = tempfile::tempdir().context("creating a scratch directory")?;
        fs::copy(spec, scratch.path().join(spec_name))
            .with_context(|| format!("copying {}", spec.display()))?;
        fs::write(
            scratch.path().join("trap.cfg"),
            format!("{head}{section}\n    {trap}\n"),
        )
        .context("writing the trap's configuration")?;
        let mut tmpdir = OsString::from("-Djava.io.tmpdir=");
        tmpdir.push(scratch.path());
        let output = Command::new("java")
            .arg("-XX:+UseParallelGC")
            .arg(tmpdir)
            .arg("-jar")
            .arg(&jar)
            .args(["-workers", "auto", "-config", "trap.cfg"])
            .arg(spec_name)
            .current_dir(scratch.path())
            .output()
            .context("running TLC")?;
        if !String::from_utf8_lossy(&output.stdout).contains(&format!(" {trap} is violated")) {
            violations.push(format!(
                "{}: TLC did not report {trap} violated ({})",
                config.display(),
                output.status
            ));
        }
    }
    Ok(violations)
}

/// The configuration before the line holding its first trap section,
/// and every trap with the section header it sits under.
fn split(text: &str) -> Result<(&str, Vec<(&str, &str)>)> {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if words(line).any(|word| SECTIONS.contains(&word)) {
            start = Some(offset);
            break;
        }
        offset += line.len();
    }
    let start = start.context("no INVARIANT or PROPERTY section")?;
    let mut traps = Vec::new();
    let mut section = "";
    for word in text[start..].lines().flat_map(words) {
        if SECTIONS.contains(&word) {
            section = word;
        } else if !section.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            traps.push((section, word));
        } else {
            bail!("unexpected `{word}` among the traps");
        }
    }
    if traps.is_empty() {
        bail!("no traps");
    }
    Ok((&text[..start], traps))
}

/// A line's words before any `\*` comment.
fn words(line: &str) -> std::str::SplitWhitespace<'_> {
    line.split("\\*")
        .next()
        .unwrap_or_default()
        .split_whitespace()
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn each_trap_keeps_its_section_and_the_shared_head() {
        let config = "SPECIFICATION Spec\nCONSTANTS\n    N = 2\nINVARIANTS\n    NoA\n    NoB\n\\* a note\nPROPERTIES\n    NoC\n";
        let (head, traps) = split(config).unwrap();
        assert_eq!(head, "SPECIFICATION Spec\nCONSTANTS\n    N = 2\n");
        assert_eq!(
            traps,
            [
                ("INVARIANTS", "NoA"),
                ("INVARIANTS", "NoB"),
                ("PROPERTIES", "NoC")
            ]
        );
    }

    #[test]
    fn a_trap_on_its_header_line_or_after_a_comment_is_split_out() {
        let config = "SPECIFICATION Spec\nINVARIANT NoA\nINVARIANT \\* a note\n    NoB\n";
        let (head, traps) = split(config).unwrap();
        assert_eq!(head, "SPECIFICATION Spec\n");
        assert_eq!(traps, [("INVARIANT", "NoA"), ("INVARIANT", "NoB")]);
    }

    #[test]
    fn a_configuration_without_traps_is_refused() {
        assert!(split("SPECIFICATION Spec\nINVARIANTS\n").is_err());
        assert!(split("SPECIFICATION Spec\n").is_err());
        assert!(split("INVARIANTS\n    (* NoA *)\n").is_err());
    }
}
