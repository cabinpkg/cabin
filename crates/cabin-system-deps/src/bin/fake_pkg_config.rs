//! Test-only stand-in for `pkg-config`.
//!
//! Cabin's integration tests need a deterministic `pkg-config`
//! executable without depending on the host having a particular
//! package installed (or even `pkg-config` itself).  This binary
//! mimics the small subset of pkg-config's command-line surface
//! Cabin's probe layer invokes:
//!
//! - `--version` is always accepted and exits 0.
//! - For every other invocation we expect at least one of
//!   `--exists`, `--modversion`, `--cflags`, `--libs`.
//! - Positional tokens after the option flags are interpreted as
//!   alternating `name [op version]?` triples.
//! - A *fixture directory* is read from the
//!   `CABIN_FAKE_PKG_CONFIG_FIXTURES` env var.  Each declared
//!   module name maps to a `<name>.json` file inside that
//!   directory; the file describes the module's installed
//!   version and the cflags / libs strings the fake should
//!   print.  Missing files produce a "package not found" exit.
//! - Optional file `<name>.invocations.log` accumulates one line
//!   per invocation so tests can assert exactly which stages
//!   were exercised.

#![allow(
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const FIXTURE_ENV: &str = "CABIN_FAKE_PKG_CONFIG_FIXTURES";
const LOG_ENV: &str = "CABIN_FAKE_PKG_CONFIG_LOG";

#[derive(Default, Debug)]
struct Fixture {
    version: Option<String>,
    cflags: String,
    libs: String,
    cflags_exit: Option<i32>,
    libs_exit: Option<i32>,
    cflags_stderr: String,
    libs_stderr: String,
}

fn main() -> ExitCode {
    // Cabin scrubs the registry credential before spawning
    // pkg-config; failing loudly here turns every integration test
    // into an enforcement point for that contract.
    if env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).is_some() {
        eprintln!("fake pkg-config: CABIN_REGISTRY_TOKEN leaked into the tool environment");
        return ExitCode::from(4);
    }
    let args = env::args_os().skip(1);
    let mut want_version_flag = false;
    let mut want_exists = false;
    let mut want_modversion = false;
    let mut want_cflags = false;
    let mut want_libs = false;
    let mut want_print_errors = false;
    let mut positional: Vec<OsString> = Vec::new();

    for arg in args {
        match arg.to_string_lossy().as_ref() {
            "--version" => want_version_flag = true,
            "--exists" => want_exists = true,
            "--modversion" => want_modversion = true,
            "--cflags" => want_cflags = true,
            "--libs" => want_libs = true,
            "--print-errors" => want_print_errors = true,
            other if other.starts_with("--") => {
                // Unknown flags are silently accepted to keep
                // tests forward-compatible if Cabin adds new
                // bookkeeping flags later.
            }
            _ => positional.push(arg),
        }
    }

    if want_version_flag && !want_exists && !want_modversion && !want_cflags && !want_libs {
        println!("cabin-system-deps-fake-pkg-config 0.0.0");
        return ExitCode::SUCCESS;
    }

    let fixtures_dir = if let Some(path) = env::var_os(FIXTURE_ENV) {
        PathBuf::from(path)
    } else {
        eprintln!(
            "cabin-system-deps-fake-pkg-config: {FIXTURE_ENV} must be set to a directory of fixture files",
        );
        return ExitCode::from(2);
    };

    let queries = match parse_positionals(&positional) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("cabin-system-deps-fake-pkg-config: {e}");
            return ExitCode::from(3);
        }
    };

    log_invocation(&positional);

    // Load fixtures for every queried module up-front so a
    // missing fixture exits before any output is emitted.
    let mut fixtures: HashMap<String, Fixture> = HashMap::new();
    for q in &queries {
        if fixtures.contains_key(&q.name) {
            continue;
        }
        let fixture = match load_fixture(&fixtures_dir, &q.name) {
            Ok(f) => f,
            Err(LoadError::Missing) => {
                if want_print_errors {
                    eprintln!(
                        "Package {} was not found in the pkg-config search path.",
                        q.name
                    );
                }
                return ExitCode::from(1);
            }
            Err(LoadError::Malformed(reason)) => {
                eprintln!(
                    "cabin-system-deps-fake-pkg-config: fixture for {} is malformed: {reason}",
                    q.name
                );
                return ExitCode::from(2);
            }
        };
        fixtures.insert(q.name.clone(), fixture);
    }

    for q in &queries {
        if let Some(constraint) = &q.constraint {
            let fixture = &fixtures[&q.name];
            if let Some(installed) = fixture.version.as_deref() {
                if !constraint_matches(installed, constraint) {
                    if want_print_errors {
                        eprintln!(
                            "Requested '{} {} {}' but version of {} is {}",
                            q.name, constraint.op, constraint.version, q.name, installed
                        );
                    }
                    return ExitCode::from(1);
                }
            } else {
                if want_print_errors {
                    eprintln!("Package {} has no version information.", q.name);
                }
                return ExitCode::from(1);
            }
        }
    }

    if want_exists {
        return ExitCode::SUCCESS;
    }
    if want_modversion {
        for q in &queries {
            match &fixtures[&q.name].version {
                Some(v) => println!("{v}"),
                None => return ExitCode::from(1),
            }
        }
        return ExitCode::SUCCESS;
    }

    let mut cflags_pieces: Vec<String> = Vec::new();
    let mut libs_pieces: Vec<String> = Vec::new();
    for q in &queries {
        let fixture = &fixtures[&q.name];
        if want_cflags {
            if let Some(code) = fixture.cflags_exit
                && code != 0
            {
                if !fixture.cflags_stderr.is_empty() {
                    eprintln!("{}", fixture.cflags_stderr);
                }
                return ExitCode::from(code as u8);
            }
            if !fixture.cflags.is_empty() {
                cflags_pieces.push(fixture.cflags.clone());
            }
        }
        if want_libs {
            if let Some(code) = fixture.libs_exit
                && code != 0
            {
                if !fixture.libs_stderr.is_empty() {
                    eprintln!("{}", fixture.libs_stderr);
                }
                return ExitCode::from(code as u8);
            }
            if !fixture.libs.is_empty() {
                libs_pieces.push(fixture.libs.clone());
            }
        }
    }
    if want_cflags && want_libs {
        // pkg-config concatenates cflags then libs with a space.
        let mut combined = cflags_pieces.clone();
        combined.extend(libs_pieces.iter().cloned());
        println!("{}", combined.join(" "));
        return ExitCode::SUCCESS;
    }
    if want_cflags {
        println!("{}", cflags_pieces.join(" "));
        return ExitCode::SUCCESS;
    }
    if want_libs {
        println!("{}", libs_pieces.join(" "));
        return ExitCode::SUCCESS;
    }
    ExitCode::SUCCESS
}

#[derive(Debug)]
struct Query {
    name: String,
    constraint: Option<Constraint>,
}

#[derive(Debug)]
struct Constraint {
    op: String,
    version: String,
}

fn parse_positionals(positionals: &[OsString]) -> Result<Vec<Query>, String> {
    let mut out: Vec<Query> = Vec::new();
    let mut iter = positionals.iter().peekable();
    while let Some(tok) = iter.next() {
        let name = tok.to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        let op_owned: Option<String> = iter
            .peek()
            .and_then(|p| p.to_str().map(str::to_owned))
            .filter(|p| matches!(p.as_str(), "=" | "!=" | "<" | ">" | "<=" | ">="));
        let constraint = if let Some(op_str) = op_owned {
            iter.next();
            let version = iter
                .next()
                .ok_or_else(|| format!("missing version after operator {op_str} for {name}"))?
                .to_string_lossy()
                .into_owned();
            Some(Constraint {
                op: op_str,
                version,
            })
        } else {
            None
        };
        out.push(Query { name, constraint });
    }
    Ok(out)
}

#[derive(Debug)]
enum LoadError {
    Missing,
    Malformed(String),
}

fn load_fixture(dir: &std::path::Path, name: &str) -> Result<Fixture, LoadError> {
    let path = dir.join(format!("{name}.json"));
    let body = match fs::read_to_string(&path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(err) => return Err(LoadError::Malformed(format!("{}: {err}", path.display()))),
    };
    parse_fixture(&body).map_err(LoadError::Malformed)
}

/// Minimal hand-rolled JSON-ish reader so the fake binary stays
/// dependency-free.  Recognized keys:
/// version: string
/// cflags: string
/// libs: string
/// cflags_exit: integer (default 0)
/// libs_exit: integer (default 0)
/// cflags_stderr: string
/// libs_stderr: string
fn parse_fixture(body: &str) -> Result<Fixture, String> {
    let mut fixture = Fixture::default();
    for raw_line in body.lines() {
        let line = raw_line
            .trim()
            .trim_end_matches(',')
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = split_kv(line) else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        let value = value.trim();
        match key {
            "version" => fixture.version = Some(parse_str(value)?),
            "cflags" => fixture.cflags = parse_str(value)?,
            "libs" => fixture.libs = parse_str(value)?,
            "cflags_exit" => fixture.cflags_exit = Some(parse_int(value)?),
            "libs_exit" => fixture.libs_exit = Some(parse_int(value)?),
            "cflags_stderr" => fixture.cflags_stderr = parse_str(value)?,
            "libs_stderr" => fixture.libs_stderr = parse_str(value)?,
            _ => {}
        }
    }
    Ok(fixture)
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let idx = line.find(':')?;
    Some((&line[..idx], &line[idx + 1..]))
}

fn parse_str(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches(',').trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        Ok(inner.replace("\\\\", "\\").replace("\\\"", "\""))
    } else {
        Err(format!("expected quoted string, got {trimmed:?}"))
    }
}

fn parse_int(raw: &str) -> Result<i32, String> {
    let trimmed = raw.trim().trim_end_matches(',').trim();
    trimmed
        .parse::<i32>()
        .map_err(|e| format!("invalid integer {trimmed:?}: {e}"))
}

fn constraint_matches(installed: &str, constraint: &Constraint) -> bool {
    let lhs = parse_version(installed);
    let rhs = parse_version(&constraint.version);
    match constraint.op.as_str() {
        "=" => lhs == rhs,
        "!=" => lhs != rhs,
        "<" => lhs < rhs,
        "<=" => lhs <= rhs,
        ">" => lhs > rhs,
        ">=" => lhs >= rhs,
        _ => false,
    }
}

/// Parse a dotted numeric version into a `Vec<u64>`.  Non-numeric
/// suffixes terminate parsing so vendor-tagged strings like
/// `1.0.1f` compare as `[1, 0, 1]`.  Missing segments default to
/// zero during comparison.
fn parse_version(raw: &str) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let trimmed = raw.trim();
    for seg in trimmed.split(['.', '-']) {
        let prefix: String = seg.chars().take_while(char::is_ascii_digit).collect();
        if prefix.is_empty() {
            break;
        }
        match prefix.parse::<u64>() {
            Ok(v) => out.push(v),
            Err(_) => break,
        }
    }
    out
}

fn log_invocation(args: &[OsString]) {
    let Some(target) = env::var_os(LOG_ENV) else {
        return;
    };
    let rendered: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let joined = rendered.join(" ");
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PathBuf::from(target))
    {
        let _ = writeln!(file, "{joined}");
    }
}
