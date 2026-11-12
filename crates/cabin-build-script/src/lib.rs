//! Run a package's build script and read what it reports.
//!
//! A package's build script is the `build.c` or `build.cpp` in its root
//! ([`find`]).  `cabin-build` plans the script's executable and applies
//! what the script reports; this crate lies between the two: [`run`]
//! starts the built executable and reads its result file ([`result`]),
//! and [`load`] answers from the last run while nothing that run
//! depended on has changed.
//!
//! A script is code the package supplies, so it runs only under
//! `cabin-sandbox`, and [`run`] fails where that cannot confine it.
//! The script gets:
//!
//! - to read: its own executable, the build-dependency executables it
//!   may run, and each file and directory in the package's root, but for
//!   symlinks, what a package archive leaves out by name (`.git`,
//!   `build`, ...), and what holds the build directory.  The root's own
//!   listing is not among them.  And each directory a `CABIN_SDK_*`
//!   variable names, unless it holds the package's root or the build
//!   directory, or its path leads through one of them;
//! - to write: four directories of its own in [`Script::dir`], made
//!   anew for each run: `out` (`CABIN_OUT_DIR`), `result` (holding
//!   `CABIN_RESULT_FILE`), `home`, and `tmp`;
//! - the environment [`cabin_env::build_script_env`] builds, and the
//!   package's root as its working directory.  On Windows, `TEMP` and
//!   `TMP` name a directory Windows makes for the run instead of `tmp`.
//!
//! Beside those four, [`Script::dir`] holds what the script cannot
//! reach: `stdout` and `stderr`, what the last run printed, and `stamp`,
//! the last successful run's result under the key of what that run
//! depended on.
//!
//! Limits:
//!
//! - `cabin-sandbox` leaves some places readable whatever is granted
//!   (on macOS, all but where user data lives).  In a package there,
//!   the names an archive leaves out stay unreadable, but what holds
//!   the build directory does not.
//! - A program that opens its standard output or error again by name
//!   (`/dev/stderr`) is refused: both lead outside what it reaches.
//! - Only the run is confined.  What a script generates is compiled as
//!   the package's own sources are, with the user's access.
//!
//! Crate boundaries: this crate neither plans nor builds, does not
//! choose the order scripts run in, and prints nothing.  A caller holds
//! the build directory's lock around [`run`] and [`load`].

mod library;
pub use library::loads_at_run_time;
pub mod result;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, Metadata};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{SystemTime, UNIX_EPOCH};

use cabin_build::BuildScriptOutput;
use cabin_build::layout::BUILD_SCRIPT_OUT_DIR;
use cabin_core::hash::{hash_reader, hex_digest};
use cabin_core::{
    Package, TargetPlatform, escape_control_chars, escape_control_chars_keeping_lines,
};
use cabin_env::build_script::names_sdk;
use cabin_env::{BuildScriptEnvError, BuildScriptEnvInputs, build_script_env};
use cabin_sandbox::{Confinement, SandboxError};
use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use result::{Invalid, Regions, left_out};

#[cfg(test)]
mod tests;

/// The directories in [`Script::dir`] a script writes: its output, the
/// one holding its result file, its home, and its temporary files.
const OWNED: [&str; 4] = [BUILD_SCRIPT_OUT_DIR, "result", "home", "tmp"];
const RESULT_FILE: &str = "result";
const STAMP: &str = "stamp";

/// What a build script reported for each target of its package, by the
/// target's name.
pub type Outputs = BTreeMap<String, BuildScriptOutput>;

/// One build script to run: a package's, for one of the contexts the
/// package is built in.
#[derive(Debug, Clone, Copy)]
pub struct Script<'a> {
    pub package: &'a Package,
    /// The features of the package this build enables.
    pub features: &'a BTreeSet<String>,
    /// The package's root, the directory of its manifest.
    pub root: &'a Utf8Path,
    /// Neither the package's root nor a directory above it: the build
    /// directory is Cabin's, and no package lies in it.
    pub build_dir: &'a Utf8Path,
    /// The package's [`cabin_build::layout::BUILD_SCRIPT_DIR`] in the
    /// context the script prepares, as an absolute path.
    pub dir: &'a Utf8Path,
    /// The script, built.
    pub executable: &'a Utf8Path,
    pub profile: &'a str,
    /// The platform the script prepares the package for.
    pub platform: &'a TargetPlatform,
    /// Whether the package's compiler takes MSVC-style commands.
    pub msvc: bool,
    /// The [`TargetPlatform::arch`] the package's compilers build for,
    /// which is not `platform`'s when a build without `--target` runs a
    /// compiler for another machine.
    pub machine: &'a str,
    /// The build-dependency executables the script may run, by target
    /// name.
    pub tools: &'a [(String, Utf8PathBuf)],
    /// Cabin's own environment, of which the script keeps what
    /// [`cabin_env::build_script::PASSED_THROUGH`] names.
    pub inherited: &'a [(OsString, OsString)],
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("`{0}` holds both `build.c` and `build.cpp`; a package has one build script")]
    Ambiguous(Utf8PathBuf),
    #[error(
        "`{0}`, the root of a package with a build script, lies in the build directory, which a build script does not read; choose another build directory"
    )]
    PackageInBuildDir(Utf8PathBuf),
    #[error(transparent)]
    Env(#[from] BuildScriptEnvError),
    /// [`SandboxError::Unavailable`] when the script could not be
    /// confined, and so was not run.
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error("failed to {action} {}: {source}", shown(path))]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// `stderr` is the end of what the script printed there, escaped
    /// for the terminal.
    #[error(
        "the build script failed ({status}); its standard error is in `{}`{}",
        log.display(),
        if stderr.is_empty() { String::new() } else { format!(" and ends:\n{stderr}") }
    )]
    Failed {
        status: ExitStatus,
        stderr: String,
        log: PathBuf,
    },
    #[error(
        "the build script left `{}`, which is not a file or directory of its own: it is a link, a file under a second name, or a special file",
        shown(.0)
    )]
    Link(PathBuf),
    #[error("line {line} of the build script's result file: {invalid}")]
    Result { line: usize, invalid: Invalid },
}

/// A path a script may have named, escaped for the terminal.
fn shown(path: &Path) -> String {
    escape_control_chars(&path.display().to_string())
}

/// The build script of the package in `root`: `build.c` or `build.cpp`,
/// as the root's listing spells it, so one file is a build script on
/// every file system or on none.  A directory of the name is not
/// one, as it is not to `cabin package`.
///
/// # Errors
/// [`Error::Ambiguous`] when the root holds both, and [`Error::Io`]
/// when it cannot be listed.
pub fn find(root: &Utf8Path) -> Result<Option<&'static str>, Error> {
    let mut found = None;
    for entry in fs::read_dir(root).map_err(failed("read", root.as_std_path()))? {
        let entry = entry.map_err(failed("read", root.as_std_path()))?;
        let name = entry.file_name();
        if let Some(script) = ["build.c", "build.cpp"]
            .into_iter()
            .find(|script| name == *script)
            && !entry.file_type().is_ok_and(|kind| kind.is_dir())
            && found.replace(script).is_some()
        {
            return Err(Error::Ambiguous(root.to_owned()));
        }
    }
    Ok(found)
}

/// What `script` reported when it last ran, while that still holds:
/// the run succeeded, and since then neither Cabin, the script's
/// executable, its environment, an executable it may run, a file it
/// read, nor its output has changed.  The files it read are those its
/// `rerun-if-changed` lines named, compared by content, or with none,
/// all it may read of the package, compared by size and modification
/// time as its output is, and where each symlink there leads.  The
/// result is checked again as after the run.
pub fn load(script: &Script<'_>) -> Option<Outputs> {
    let stamp = fs::read(script.dir.join(STAMP)).ok()?;
    let end = stamp.iter().position(|byte| *byte == b'\n')?;
    let (env, regions) = script.prepare().ok()?;
    let report = result::parse(&stamp[end + 1..], script, &regions).ok()?;
    let key = key(&env, &regions, &report.inputs).ok()?;
    (key.as_bytes() == &stamp[..end]).then_some(report.targets)
}

/// Run `script` confined and return what it reported.  [`load`] answers
/// from this run afterwards.
///
/// # Errors
/// [`Error::Sandbox`] when the script cannot be confined or started,
/// [`Error::Failed`] when it does not exit successfully,
/// [`Error::Link`] and [`Error::Result`] when what it left is refused,
/// [`Error::Env`] when two names would share one of its variables,
/// [`Error::PackageInBuildDir`], and [`Error::Io`].
pub fn run(script: &Script<'_>) -> Result<Outputs, Error> {
    let dir = script.dir.as_std_path();
    fs::create_dir_all(dir).map_err(failed("create", dir))?;
    let (env, regions) = script.prepare()?;
    let dir = &regions.dir;
    // A run that fails leaves no result, whatever it leaves in `out`.
    let stamp = dir.join(STAMP);
    removed(&stamp, fs::remove_file(&stamp))?;
    let before = snapshot(&regions.out);
    let owned = OWNED.map(|name| dir.join(name));
    for path in &owned {
        // Removed, not emptied: on Windows a program can leave a
        // junction where a directory it wrote stood empty, and a run
        // granted that path is granted where the junction leads.
        removed(path, fs::remove_dir_all(path))?;
        fs::create_dir(path).map_err(failed("create", path))?;
    }
    let [stdout, stderr] = ["stdout", "stderr"].map(|name| dir.join(name));
    let create = |path: &Path| File::create(path).map_err(failed("write", path));
    let status = cabin_sandbox::run(Confinement {
        program: regions.executable.clone(),
        args: Vec::new(),
        current_dir: regions.root.clone(),
        env: env
            .iter()
            .map(|(name, value)| (name.into(), value.clone()))
            .collect(),
        read: regions.readable()?,
        write: owned.to_vec(),
        hidden: regions.hidden()?,
        stdout: create(&stdout)?,
        stderr: create(&stderr)?,
    })?;
    if !status.success() {
        return Err(Error::Failed {
            status,
            stderr: tail(&stderr),
            log: stderr,
        });
    }
    let [out, result_dir, ..] = &owned;
    scan(out)?;
    scan(result_dir)?;
    let result_file = result_dir.join(RESULT_FILE);
    let text = match fs::read(&result_file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(failed("read", &result_file)(error));
        }
        text => text.unwrap_or_default(),
    };
    let report = result::parse(&text, script, &regions)
        .map_err(|(line, invalid)| Error::Result { line, invalid })?;
    restore(out, &before);
    let key = key(&env, &regions, &report.inputs)?;
    cabin_fs::write_atomic(&stamp, [key.as_bytes(), b"\n", &text].concat())
        .map_err(failed("write", &stamp))?;
    Ok(report.targets)
}

impl Script<'_> {
    pub(crate) fn out(&self) -> Utf8PathBuf {
        self.dir.join(BUILD_SCRIPT_OUT_DIR)
    }

    /// The script's environment and its paths resolved, once
    /// [`Self::dir`] exists.
    pub(crate) fn prepare(&self) -> Result<(BTreeMap<String, OsString>, Regions), Error> {
        let real = |path: &Utf8Path| {
            cabin_fs::canonicalize(path).map_err(failed("read", path.as_std_path()))
        };
        let (root, build_dir, dir) = (real(self.root)?, real(self.build_dir)?, real(self.dir)?);
        if root.starts_with(&build_dir) {
            return Err(Error::PackageInBuildDir(self.root.to_owned()));
        }
        let tools = self
            .tools
            .iter()
            .map(|(_, tool)| real(tool))
            .collect::<Result<Vec<_>, _>>()?;
        let features: Vec<_> = self
            .package
            .features
            .features
            .keys()
            .map(|feature| (feature.as_str(), self.features.contains(feature)))
            .collect();
        let named: Vec<_> = self
            .tools
            .iter()
            .zip(&tools)
            .map(|((name, _), tool)| (name.as_str(), tool.as_path()))
            .collect();
        let [out, result, home, tmp] = OWNED.map(|name| dir.join(name));
        let inputs = BuildScriptEnvInputs {
            out_dir: &out,
            result_file: &result.join(RESULT_FILE),
            home: &home,
            tmp: &tmp,
            manifest_dir: &root,
            package_name: self.package.name.as_str(),
            package_version: &self.package.version.to_string(),
            profile: self.profile,
            target_os: &self.platform.os,
            target_arch: &self.platform.arch,
            target_family: &self.platform.family,
            target_env: &self.platform.env,
            msvc: self.msvc,
            features: &features,
            tools: &named,
        };
        let env = build_script_env(&inputs, self.inherited.iter().cloned())?;
        let offered: Vec<PathBuf> = env
            .iter()
            .filter(|(name, value)| names_sdk(name) && Path::new(value).is_absolute())
            .map(|(_, value)| value.into())
            .collect();
        let sdks = offered
            .iter()
            .flat_map(|sdk| [Some(sdk.clone()), cabin_fs::canonicalize(sdk).ok()])
            .flatten()
            .collect();
        let regions = Regions {
            executable: real(self.executable)?,
            root,
            build_dir,
            dir,
            out,
            tools,
            sdks,
            offered,
        };
        Ok((env, regions))
    }
}

impl Regions {
    /// The package's root, listed in name order: each entry's name and
    /// whether it is a plain file or directory.
    fn listing(&self) -> Result<Vec<(OsString, bool)>, Error> {
        let root = &self.root;
        let mut listing = Vec::new();
        for entry in fs::read_dir(root).map_err(failed("read", root))? {
            let entry = entry.map_err(failed("read", root))?;
            let kind = entry.file_type().map_err(failed("read", &entry.path()))?;
            listing.push((entry.file_name(), kind.is_dir() || kind.is_file()));
        }
        listing.sort();
        Ok(listing)
    }

    /// The entries of the package's root a run depends on, each with
    /// whether the script is granted it.  A symlink is not, as the grant
    /// would lead wherever it does, but a script can read through one
    /// that leads to what it is granted.
    fn entries(&self) -> Result<Vec<(PathBuf, bool)>, Error> {
        let listing = self.listing()?.into_iter();
        let read = listing.filter(|(name, _)| self.reads(name));
        Ok(read
            .map(|(name, plain)| (self.root.join(name), plain))
            .collect())
    }

    /// The paths the script may read.  A `CABIN_SDK_*` directory is
    /// one, but for these:
    ///
    /// - one whose path, as the user spelled it, leads through the
    ///   build directory, where a script could have put a link in its
    ///   way, or through the package's root, of which the script reads
    ///   only what [`Self::entries`] grants;
    /// - one that holds either, which a script reads neither of whole;
    /// - one where an entry [`Self::hidden`] hides leads: a hidden path
    ///   may hold no granted one.
    pub(crate) fn readable(&self) -> Result<Vec<PathBuf>, Error> {
        let programs = std::iter::once(&self.executable).chain(&self.tools);
        let entries = self.entries()?.into_iter();
        let granted = entries.filter_map(|(path, granted)| granted.then_some(path));
        let resolved = |path: &Path| cabin_fs::canonicalize(path).ok();
        let kept = [&self.root, &self.build_dir];
        let hidden: Vec<_> = self
            .hidden()?
            .iter()
            .filter_map(|path| resolved(path))
            .collect();
        let mut sdks: Vec<_> = self
            .offered
            .iter()
            .filter_map(|sdk| {
                let real = resolved(sdk).filter(|real| real.is_dir())?;
                let mut through = sdk.ancestors().filter_map(resolved);
                let apart = through.all(|part| kept.iter().all(|kept| !part.starts_with(kept)))
                    && kept.iter().all(|kept| !kept.starts_with(&real))
                    && hidden.iter().all(|hidden| !real.starts_with(hidden));
                apart.then_some(real)
            })
            .collect();
        sdks.sort();
        sdks.dedup();
        Ok(programs.cloned().chain(granted).chain(sdks).collect())
    }

    /// The entries of the root a package archive leaves out by name,
    /// which stay unread where the confinement itself leaves the package
    /// readable.  Taken from the listing, so that on a file system that
    /// ignores case `build` does not hide a `Build` the script is
    /// granted.  Without one that holds the build directory, or leads to
    /// what does: a hidden path may hold no granted one.
    pub(crate) fn hidden(&self) -> Result<Vec<PathBuf>, Error> {
        let listed = self.listing()?.into_iter().map(|(name, _)| name);
        Ok(listed
            .filter(|name| left_out(name))
            .map(|name| self.root.join(name))
            .filter(|path| {
                let leads = cabin_fs::canonicalize(path);
                !self.build_dir.starts_with(leads.as_ref().unwrap_or(path))
            })
            .collect())
    }
}

/// The key of everything a run's result depends on, [`load`]'s list.
fn key(
    env: &BTreeMap<String, OsString>,
    regions: &Regions,
    inputs: &[PathBuf],
) -> Result<String, Error> {
    let mut key = Sha256::new();
    add(&mut key, env!("CARGO_PKG_VERSION"));
    for (name, value) in env {
        add(&mut key, name);
        add(&mut key, value.as_encoded_bytes());
    }
    for program in std::iter::once(&regions.executable).chain(&regions.tools) {
        add(&mut key, hash(program)?);
    }
    let entries: Vec<_>;
    let named = !inputs.is_empty();
    let read = if named {
        inputs
    } else {
        let all = regions.entries()?.into_iter();
        entries = all.map(|(path, _)| path).collect();
        &entries
    };
    let watched = read.iter().map(|path| (path, named));
    for (path, by_content) in watched.chain([(&regions.out, false)]) {
        let mut entry = |path: &Path, metadata: &Metadata| {
            add(&mut key, path.as_os_str().as_encoded_bytes());
            // Each kind under its name, which says how many fields
            // follow, so no two trees share one list of fields.
            if metadata.is_file() && by_content {
                add(&mut key, "content");
                add(&mut key, hash(path)?);
            } else if metadata.is_file() {
                let modified = metadata.modified().ok();
                let since = modified.and_then(|time| time.duration_since(UNIX_EPOCH).ok());
                add(&mut key, "stat");
                add(&mut key, metadata.len().to_string());
                add(
                    &mut key,
                    since.map_or(0, |since| since.as_nanos()).to_string(),
                );
            } else if let Ok(leads) = fs::read_link(path) {
                add(&mut key, "link");
                add(&mut key, leads.as_os_str().as_encoded_bytes());
            } else if metadata.is_dir() {
                add(&mut key, "directory");
            } else {
                add(&mut key, "other");
            }
            Ok(true)
        };
        let metadata = fs::symlink_metadata(path).map_err(failed("read", path))?;
        entry(path, &metadata)?;
        if metadata.is_dir() {
            walk(path, &mut entry)?;
        }
    }
    Ok(hex_digest(&key.finalize()))
}

/// Add one field to a key, closed by the byte no path or variable holds.
fn add(key: &mut Sha256, field: impl AsRef<[u8]>) {
    key.update(field);
    key.update([0]);
}

fn failed<'a>(action: &'static str, path: &'a Path) -> impl Fn(io::Error) -> Error + 'a {
    move |source| Error::Io {
        action,
        path: path.to_owned(),
        source,
    }
}

/// A removal of `path` succeeded when nothing was there.
fn removed(path: &Path, removal: io::Result<()>) -> Result<(), Error> {
    match removal {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(failed("remove", path)(error)),
        _ => Ok(()),
    }
}

fn hash(path: &Path) -> Result<String, Error> {
    File::open(path)
        .and_then(hash_reader)
        .map_err(failed("read", path))
}

/// Visit everything beneath `dir` in name order, each with the metadata
/// of the entry itself, not of what a link leads to.  `visit` returns
/// whether to go on into a directory; no link is followed.
fn walk(
    dir: &Path,
    visit: &mut dyn FnMut(&Path, &Metadata) -> Result<bool, Error>,
) -> Result<(), Error> {
    let mut entries = fs::read_dir(dir)
        .and_then(Iterator::collect::<io::Result<Vec<_>>>)
        .map_err(failed("read", dir))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(failed("read", &path))?;
        if visit(&path, &metadata)? && metadata.is_dir() {
            walk(&path, visit)?;
        }
    }
    Ok(())
}

/// The files of `out` before a run empties it, for [`restore`].  One
/// that cannot be read is left out: all this saves is rebuilding.
fn snapshot(out: &Path) -> BTreeMap<PathBuf, (String, SystemTime)> {
    let mut files = BTreeMap::new();
    if fs::symlink_metadata(out).is_ok_and(|metadata| metadata.is_dir()) {
        let _ = walk(out, &mut |path, metadata| {
            if metadata.is_file()
                && let (Ok(hash), Ok(modified)) = (hash(path), metadata.modified())
            {
                files.insert(path.to_owned(), (hash, modified));
            }
            Ok(true)
        });
    }
    files
}

/// Give each file the run wrote back as it was the modification time it
/// had, so nothing that reads the file is rebuilt.
fn restore(out: &Path, before: &BTreeMap<PathBuf, (String, SystemTime)>) {
    let _ = walk(out, &mut |path, metadata| {
        if metadata.is_file()
            && let Some((old, modified)) = before.get(path)
            && hash(path).is_ok_and(|new| new == *old)
        {
            let _ = File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(*modified));
        }
        Ok(true)
    });
}

/// Refuse a directory the script wrote when it is, or holds, anything
/// but files and directories of the script's own.  Cabin reads what is
/// here, and compiles it, with the user's access, not the script's.
///
/// This holds for as long as nothing of the script still runs, which
/// `cabin-sandbox` sees to but for a process that left the script's
/// process group on macOS.
fn scan(dir: &Path) -> Result<(), Error> {
    let mut own = |path: &Path, metadata: &Metadata| {
        #[cfg(unix)]
        let linked = {
            use std::os::unix::fs::MetadataExt as _;
            metadata.is_file() && metadata.nlink() > 1
        };
        // A reparse point of any kind.  A second name is not looked
        // for: Windows lets a confined program link only to a file it
        // may write.
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt as _;
            metadata.file_attributes() & 0x400 != 0
        };
        if (metadata.is_dir() || metadata.is_file()) && !linked {
            Ok(true)
        } else {
            Err(Error::Link(path.to_owned()))
        }
    };
    own(
        dir,
        &fs::symlink_metadata(dir).map_err(failed("read", dir))?,
    )?;
    walk(dir, &mut own)
}

/// The end of what the script wrote to `stderr`, escaped for the
/// terminal.
fn tail(stderr: &Path) -> String {
    let mut end = Vec::new();
    let _ = File::open(stderr).and_then(|mut file| {
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
        file.read_to_end(&mut end)
    });
    escape_control_chars_keeping_lines(String::from_utf8_lossy(&end).trim_end())
}
