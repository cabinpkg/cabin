//! The result file: what a build script reports, one directive a line.
//!
//! The file is UTF-8.  A line ends with `\n` or `\r\n`; an empty one, one
//! starting with `#`, and one repeating an earlier line exactly are
//! skipped.  A directive's fields are separated by one space, and its
//! last field is the rest of the line:
//!
//! - `include-dir <target> <directory>`: a further include directory of
//!   the target, as its manifest's `include-dirs` are.  It is under
//!   `CABIN_OUT_DIR`, or under the directory a `CABIN_SDK_*` variable
//!   names;
//! - `define <target> <NAME[=value]>`: a further entry of the target's
//!   `defines`;
//! - `interface-define <target> <NAME[=value]>`: a further entry of a
//!   library's `interface-defines`, of a macro it does not define yet;
//! - `source <target> <file>`: a source under `CABIN_OUT_DIR` the target
//!   compiles after its own;
//! - `external <target>`: the script supplies this library.  Cabin
//!   builds nothing of the target, and what depends on it gets what the
//!   script's lines say and none of what the manifest declares.  It
//!   comes before every other line naming the target, which takes no
//!   `define` or `source` and at least one `link-file`;
//! - `link-file <target> <file>`: a library file an `external` target is
//!   linked from, under `CABIN_OUT_DIR` or under the directory a
//!   `CABIN_SDK_*` variable names, in the order of the lines.  It is a
//!   `.lib` for an MSVC-style link, and is not built for another
//!   machine;
//! - `rerun-if-changed <path>`: a file or directory the script's
//!   result depends on.  Once a script names one, it runs again only
//!   when one it named changes, not whenever a file of the package
//!   does.  It is one the script may read in the package, or under the
//!   directory a `CABIN_SDK_*` variable names.  A directory stands for
//!   every file and symlink beneath it, not for what a symlink leads to.
//!
//! Every path is absolute, is spelled with no `..` beneath a directory
//! the script was given (`CABIN_OUT_DIR`, `CABIN_MANIFEST_DIR`, a
//! `CABIN_SDK_*` value), and leads to something that exists.  A target
//! is one the package declares and this build enables.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use cabin_build::BuildScriptOutput;
use cabin_core::{escape_control_chars, is_valid_interface_define, macro_name};
use cabin_package::archive::EXCLUDED_DIR_NAMES;
use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::Script;

/// Why a line of the result file is refused.  The text a script wrote
/// is escaped for the terminal.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Invalid {
    #[error("the file is not UTF-8")]
    NotUtf8,
    #[error("`{}` is not a directive", escape_control_chars(.0))]
    UnknownDirective(String),
    #[error("`{0}` takes a target and a value")]
    MissingValue(&'static str),
    #[error("the package has no target `{}`", escape_control_chars(.0))]
    UnknownTarget(String),
    #[error("target `{target}` is not built: it requires the feature `{}`", missing.join("`, `"))]
    DisabledTarget {
        target: String,
        missing: Vec<String>,
    },
    #[error("`{directive}` does not apply to `{target}`, a {kind} target")]
    Kind {
        directive: &'static str,
        target: String,
        kind: &'static str,
    },
    #[error(
        "`{}` is not `NAME` or `NAME=value` with a C identifier for `NAME`",
        escape_control_chars(.0)
    )]
    Define(String),
    #[error("target `{target}` already defines `{name}` for what depends on it")]
    Redefined { target: String, name: String },
    #[error("`external {0}` comes before every other line that names `{0}`")]
    ExternalLate(String),
    #[error(
        "`{directive}` does not apply to `{target}`, which `external` says Cabin does not build"
    )]
    External {
        directive: &'static str,
        target: String,
    },
    #[error("`link-file` applies to a library an `external {0}` line names first")]
    NotExternal(String),
    #[error("`external {0}` needs a `link-file` line for `{0}`")]
    NothingToLink(String),
    #[error("`{}` is built for {built}, and this build is for {machine}", escape_control_chars(.path))]
    Machine {
        path: String,
        built: &'static str,
        machine: String,
    },
    #[error("`{}` {problem}", escape_control_chars(.path))]
    Path { path: String, problem: &'static str },
}

/// What a result file says.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) targets: BTreeMap<String, BuildScriptOutput>,
    /// The `rerun-if-changed` paths, resolved.
    pub(crate) inputs: Vec<PathBuf>,
}

/// A script's paths, each resolved.  The confinement grants where a
/// path leads, and a script reaches one spelled through a symlink only
/// if it may read the symlink, so it is given these.
#[derive(Debug)]
pub(crate) struct Regions {
    pub(crate) root: PathBuf,
    pub(crate) build_dir: PathBuf,
    /// [`Script::dir`].
    pub(crate) dir: PathBuf,
    /// `CABIN_OUT_DIR`, in [`Self::dir`].
    pub(crate) out: PathBuf,
    pub(crate) executable: PathBuf,
    /// [`Script::tools`], in their order.
    pub(crate) tools: Vec<PathBuf>,
    /// The directories `CABIN_SDK_*` variables name, each as spelled,
    /// where a script's paths start, and as resolved, where they lead.
    pub(crate) sdks: Vec<PathBuf>,
    /// The same directories, as spelled only.
    pub(crate) offered: Vec<PathBuf>,
}

impl Regions {
    /// Whether the script reads the entry `name` of the package's root:
    /// not one a package archive leaves out by name, nor what holds the
    /// build directory.
    pub(crate) fn reads(&self, name: &OsStr) -> bool {
        !left_out(name) && !self.build_dir.starts_with(self.root.join(name))
    }
}

/// Whether a package archive leaves out a directory named `name`.  The
/// names are compared as the archive compares them, exactly: a
/// version-control system writes its directory under one spelling.
pub(crate) fn left_out(name: &OsStr) -> bool {
    EXCLUDED_DIR_NAMES
        .iter()
        .any(|excluded| name == OsStr::new(excluded))
}

pub(crate) const NOT_OUTPUT: &str = "is not under `CABIN_OUT_DIR`";
pub(crate) const NOT_OFFERED: &str = "is neither under `CABIN_OUT_DIR` nor under a directory a `CABIN_SDK_*` variable names; the manifest's `include-dirs` list the package's own";
pub(crate) const NOT_LINKABLE: &str =
    "is neither under `CABIN_OUT_DIR` nor under a directory a `CABIN_SDK_*` variable names";
pub(crate) const NOT_LIB: &str = "is not a `.lib` file, which is what an MSVC-style link takes";
pub(crate) const UNWATCHED: &str = "is neither a file of the package the script may read nor under a directory a `CABIN_SDK_*` variable names";

/// Read a result file, refusing it whole at the first line that breaks
/// a rule: nothing of a refused file is applied.
pub(crate) fn parse(
    bytes: &[u8],
    script: &Script<'_>,
    regions: &Regions,
) -> Result<Report, (usize, Invalid)> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        let before = &bytes[..error.valid_up_to()];
        (
            before.split(|byte| *byte == b'\n').count(),
            Invalid::NotUtf8,
        )
    })?;
    let mut report = Report::default();
    let mut seen = HashSet::new();
    // Each `external` line's target and number, in the lines' order.
    let mut external = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') || !seen.insert(line) {
            continue;
        }
        directive(line, script, regions, &mut report).map_err(|invalid| (index + 1, invalid))?;
        if let Some(target) = line.strip_prefix("external ") {
            external.push((target, index + 1));
        }
    }
    for (target, line) in external {
        if report.targets[target].link_files.is_empty() {
            return Err((line, Invalid::NothingToLink(target.to_owned())));
        }
    }
    Ok(report)
}

fn directive(
    line: &str,
    script: &Script<'_>,
    regions: &Regions,
    report: &mut Report,
) -> Result<(), Invalid> {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    if name == "rerun-if-changed" {
        report.inputs.push(input(rest, regions)?);
        return Ok(());
    }
    let Some(directive) = [
        "include-dir",
        "define",
        "interface-define",
        "source",
        "external",
        "link-file",
    ]
    .into_iter()
    .find(|directive| *directive == name) else {
        return Err(Invalid::UnknownDirective(name.to_owned()));
    };
    let (target_name, value) = if directive == "external" {
        (rest, "")
    } else {
        rest.split_once(' ')
            .ok_or(Invalid::MissingValue(directive))?
    };
    let target = script
        .package
        .targets
        .iter()
        .find(|target| target.name.as_str() == target_name)
        .ok_or_else(|| Invalid::UnknownTarget(target_name.to_owned()))?;
    let missing = target.missing_required_features(script.features);
    if !missing.is_empty() {
        return Err(Invalid::DisabledTarget {
            target: target_name.to_owned(),
            missing,
        });
    }
    // As the manifest allows the fields they add to.
    let applies = match directive {
        "interface-define" => target.kind.is_library_like(),
        "define" | "source" => !target.kind.is_header_only(),
        "external" | "link-file" => target.kind.produces_archive(),
        _ => true,
    };
    if !applies {
        return Err(Invalid::Kind {
            directive,
            target: target_name.to_owned(),
            kind: target.kind.as_str(),
        });
    }
    if directive == "external" && report.targets.contains_key(target_name) {
        return Err(Invalid::ExternalLate(target_name.to_owned()));
    }
    let output = report.targets.entry(target_name.to_owned()).or_default();
    match directive {
        "external" => output.external = true,
        "define" | "source" if output.external => {
            return Err(Invalid::External {
                directive,
                target: target_name.to_owned(),
            });
        }
        "link-file" if !output.external => {
            return Err(Invalid::NotExternal(target_name.to_owned()));
        }
        "link-file" => {
            let file = link_file(value, script, regions)?;
            if !output.link_files.contains(&file) {
                output.link_files.push(file);
            }
        }
        "include-dir" => output
            .include_dirs
            .push(include_dir(value, script, regions)?),
        "source" => {
            // One file under two spellings is still one source.
            let source = source(value, regions)?;
            if !output.sources.contains(&source) {
                output.sources.push(source);
            }
        }
        _ if !is_valid_interface_define(value) => return Err(Invalid::Define(value.to_owned())),
        "define" => output.defines.push(value.to_owned()),
        _ => {
            let name = macro_name(value);
            // An external target's manifest defines nothing for what
            // depends on it.
            let declared = target.interface_defines.iter();
            if declared
                .filter(|_| !output.external)
                .chain(&output.interface_defines)
                .any(|define| macro_name(define) == name)
            {
                return Err(Invalid::Redefined {
                    target: target_name.to_owned(),
                    name: name.to_owned(),
                });
            }
            output.interface_defines.push(value.to_owned());
        }
    }
    Ok(())
}

/// Where the path a directive names leads, when it is absolute and
/// spelled, with no `..`, beneath `place` or one of `sdks`; `elsewhere`
/// is the problem of one that is not.  The file system is asked nothing
/// about such a path: it may answer by reaching for another machine,
/// and on Windows `\\host\share` signs in to `host` as the user.
fn real(
    path: &str,
    place: &Path,
    sdks: &[PathBuf],
    elsewhere: &'static str,
) -> Result<PathBuf, Invalid> {
    let problem = |problem| Invalid::Path {
        path: path.to_owned(),
        problem,
    };
    let spelled = Path::new(path);
    if !spelled.is_absolute() {
        return Err(problem("is not an absolute path"));
    }
    if spelled
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return Err(problem("has a `..` in it"));
    }
    let placed = spelled.starts_with(place) || sdks.iter().any(|sdk| spelled.starts_with(sdk));
    if !placed {
        return Err(problem(elsewhere));
    }
    cabin_fs::canonicalize(spelled).map_err(|_| problem("does not lead to anything that exists"))
}

/// `below`, a path found under a resolved directory, under `base`, the
/// same directory as the rest of the build spells it.
fn rebase(base: &Utf8Path, below: &Path, path: &str) -> Result<Utf8PathBuf, Invalid> {
    below.iter().try_fold(base.to_owned(), |rebased, name| {
        let name = name.to_str().ok_or(Invalid::Path {
            path: path.to_owned(),
            problem: "is not UTF-8 once resolved",
        })?;
        Ok(rebased.join(name))
    })
}

/// `real`, where the path a directive names leads, as the build spells
/// it: under `CABIN_OUT_DIR` as the build spells that directory, and
/// under an SDK directory as written.
fn placed(
    path: &str,
    real: &Path,
    elsewhere: &'static str,
    script: &Script<'_>,
    regions: &Regions,
) -> Result<Utf8PathBuf, Invalid> {
    if let Ok(below) = real.strip_prefix(&regions.out) {
        return rebase(&script.out(), below, path);
    }
    if regions.sdks.iter().any(|sdk| real.starts_with(sdk)) {
        Ok(path.into())
    } else {
        Err(Invalid::Path {
            path: path.to_owned(),
            problem: elsewhere,
        })
    }
}

fn include_dir(path: &str, script: &Script<'_>, regions: &Regions) -> Result<Utf8PathBuf, Invalid> {
    let real = real(path, &regions.out, &regions.sdks, NOT_OFFERED)?;
    if !real.is_dir() {
        return Err(Invalid::Path {
            path: path.to_owned(),
            problem: "is not a directory",
        });
    }
    placed(path, &real, NOT_OFFERED, script, regions)
}

fn link_file(path: &str, script: &Script<'_>, regions: &Regions) -> Result<Utf8PathBuf, Invalid> {
    let real = real(path, &regions.out, &regions.sdks, NOT_LINKABLE)?;
    if !real.is_file() {
        return Err(Invalid::Path {
            path: path.to_owned(),
            problem: "is not a file",
        });
    }
    // Placed first: a link beneath `CABIN_OUT_DIR` may lead to a file the
    // script may not read, which is then not opened.
    let placed = placed(path, &real, NOT_LINKABLE, script, regions)?;
    crate::library::check(path, &real, script.msvc, script.machine)?;
    Ok(placed)
}

/// A generated source, relative to `CABIN_OUT_DIR`.
fn source(path: &str, regions: &Regions) -> Result<Utf8PathBuf, Invalid> {
    let problem = |problem| Invalid::Path {
        path: path.to_owned(),
        problem,
    };
    let real = real(path, &regions.out, &[], NOT_OUTPUT)?;
    let below = real
        .strip_prefix(&regions.out)
        .map_err(|_| problem(NOT_OUTPUT))?;
    if !real.is_file() {
        return Err(problem("is not a file"));
    }
    rebase(Utf8Path::new(""), below, path)
}

fn input(path: &str, regions: &Regions) -> Result<PathBuf, Invalid> {
    let real = real(path, &regions.root, &regions.sdks, UNWATCHED)?;
    let in_package = real
        .strip_prefix(&regions.root)
        .is_ok_and(|below| below.iter().next().is_some_and(|top| regions.reads(top)));
    if in_package || regions.sdks.iter().any(|sdk| real.starts_with(sdk)) {
        Ok(real)
    } else {
        Err(Invalid::Path {
            path: path.to_owned(),
            problem: UNWATCHED,
        })
    }
}
