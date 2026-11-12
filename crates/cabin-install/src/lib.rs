//! The executables a user installed with `cabin install`, and the
//! record of which package owns each.
//!
//! An install root holds the executables in `bin/` and Cabin's own
//! files in `.cabin-install/`: `installs.json`, the record; `lock`,
//! which lets one command at a time change the root; and `tmp/`, the
//! files a change is moving.
//!
//! A change copies its new files into `tmp/`, renames each file it
//! replaces or removes there as well, renames the new files into
//! `bin/`, and writes the record last.  A failure before the record is
//! written renames everything back.  Nothing is journaled: running the
//! command again completes a change that was interrupted.  Files a
//! package's first installation had already put in `bin/` are then in
//! the way like any the record does not name, and need `force`.
//! Nothing but the record is synced, so a power loss can undo a change
//! to `bin/` the record already holds.
//!
//! What to build, and which executables a package offers, are the
//! caller's.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

const RECORD_VERSION: u32 = 1;

/// A package installed into a root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Source,
    /// The package's enabled features.
    pub features: Vec<String>,
    pub profile: String,
    pub toolchain: Toolchain,
    /// Every other package the build used.
    pub dependencies: Vec<Dependency>,
    /// The shared libraries the package's programs were linked with and
    /// load when they run.  Cabin neither installed them nor checked
    /// that a program finds them.
    #[serde(default)]
    pub runtime_libraries: Vec<Utf8PathBuf>,
    /// The files in `bin/` the package owns.
    pub binaries: Vec<Binary>,
}

/// Where a package's sources came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    /// A directory on this machine.
    Path(Utf8PathBuf),
    /// A registry archive.
    Registry {
        checksum: String,
        /// Whether the lockfile chose the version.
        locked: bool,
    },
    /// Not built: what the system provides.
    System,
}

/// The compiler that built a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Toolchain {
    pub compiler: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub source: Source,
}

/// A file in `bin/`, with the SHA-256 of what Cabin put there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Binary {
    pub name: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    packages: Vec<Package>,
}

/// What [`Root::install`] did to `bin/`, by file name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Installed {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    /// Files of the package's earlier installation that the new one
    /// does not have.
    pub removed: Vec<String>,
    /// Such files changed since Cabin installed them: left in place,
    /// no longer owned.
    pub left: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("cannot read the install record {}: {reason}", path.display())]
    Record { path: PathBuf, reason: String },
    /// Files in the way, each with the package that owns it, if one
    /// does.
    #[error("{}", collisions(.bin, .files))]
    Collision {
        bin: PathBuf,
        files: Vec<(String, Option<String>)>,
    },
    #[error(
        "`{binary}` differs only in case from `{other}` of package `{owner}`, and a root holds one of the two"
    )]
    Case {
        binary: String,
        other: String,
        owner: String,
    },
    #[error("package `{0}` is not installed")]
    NotInstalled(String),
    #[error("package `{package}` did not install `{binary}`")]
    NotABinary { package: String, binary: String },
    #[error("{}", modified(.0))]
    Modified(Vec<PathBuf>),
}

fn collisions(bin: &Path, files: &[(String, Option<String>)]) -> String {
    let lines: Vec<String> = files
        .iter()
        .map(|(name, owner)| {
            let path = bin.join(name);
            match owner {
                Some(owner) => format!("{} belongs to package `{owner}`", path.display()),
                None => format!("{} exists and Cabin did not install it", path.display()),
            }
        })
        .collect();
    lines.join("\n")
}

fn modified(files: &[PathBuf]) -> String {
    let lines: Vec<String> = files
        .iter()
        .map(|path| format!("{} changed since Cabin installed it", path.display()))
        .collect();
    lines.join("\n")
}

/// An install root.
#[derive(Debug, Clone)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Where the root's executables are.
    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    fn state(&self) -> PathBuf {
        self.dir.join(".cabin-install")
    }

    fn record(&self) -> PathBuf {
        self.state().join("installs.json")
    }

    /// The installed packages, by name.  A root nothing was installed
    /// into has none.
    ///
    /// # Errors
    /// [`Error::Io`] or [`Error::Record`] when the record cannot be
    /// read.
    pub fn packages(&self) -> Result<Vec<Package>, Error> {
        let path = self.record();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(Error::Io { path, source }),
        };
        let record: Record = serde_json::from_slice(&bytes).map_err(|err| Error::Record {
            path: path.clone(),
            reason: err.to_string(),
        })?;
        if record.version != RECORD_VERSION {
            return Err(Error::Record {
                path,
                reason: format!("it is version {}, not {RECORD_VERSION}", record.version),
            });
        }
        Ok(record.packages)
    }

    fn write(&self, mut packages: Vec<Package>) -> Result<(), Error> {
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        let mut bytes = serde_json::to_vec_pretty(&Record {
            version: RECORD_VERSION,
            packages,
        })
        .expect("the record is plain data");
        bytes.push(b'\n');
        let path = self.record();
        match cabin_fs::write_atomic(&path, &bytes) {
            // The new record can be in place when making it durable
            // fails: the change is then made, and is not to be undone.
            Err(_) if fs::read(&path).is_ok_and(|now| now == bytes) => Ok(()),
            written => written.map_err(|source| Error::Io { path, source }),
        }
    }

    /// Install `files`, each a file name in `bin/` and the built file
    /// to copy there, as `package`'s executables, replacing what the
    /// root has of an earlier installation of the package.
    /// `package.binaries` is ignored: the record gets what was
    /// installed.  `on_wait` runs once before waiting for another
    /// command to finish with the root.
    ///
    /// # Errors
    /// [`Error::Collision`], with nothing changed, when a file name is
    /// taken by another package or by a file Cabin did not install,
    /// unless `force`, which takes the name over.  [`Error::Case`]
    /// when a name differs only in case from one the root has or is
    /// getting.  Any other error leaves `bin/` and the record as they
    /// were.
    pub fn install(
        &self,
        mut package: Package,
        files: &[(String, PathBuf)],
        force: bool,
        on_wait: impl FnOnce(),
    ) -> Result<Installed, Error> {
        let _lock = self.lock(on_wait)?;
        let bin = self.bin();
        let mut packages = self.packages()?;
        let earlier = packages
            .iter()
            .position(|p| p.name == package.name)
            .map(|index| packages.remove(index).binaries)
            .unwrap_or_default();
        // Names that differ only in case are one file where the file
        // system ignores case, so a root holds one of them everywhere.
        let named = packages
            .iter()
            .flat_map(|p| p.binaries.iter().map(move |b| (&p.name, &b.name)))
            .chain(earlier.iter().map(|b| (&package.name, &b.name)))
            .chain(files.iter().map(|(name, _)| (&package.name, name)));
        for (owner, other) in named {
            let clash =
                |(name, _): &&(String, PathBuf)| name != other && name.eq_ignore_ascii_case(other);
            if let Some((name, _)) = files.iter().find(clash) {
                return Err(Error::Case {
                    binary: name.clone(),
                    other: other.clone(),
                    owner: owner.clone(),
                });
            }
        }
        let mut done = Installed::default();
        let mut taken = Vec::new();
        for (name, _) in files {
            let path = bin.join(name);
            // Replacing a directory would delete what it holds.
            if path.symlink_metadata().is_ok_and(|at| at.is_dir()) {
                return io_at(&path, Err(io::ErrorKind::IsADirectory.into()));
            }
            if earlier.iter().any(|b| b.name == *name) {
                done.replaced.push(name.clone());
                continue;
            }
            let owner = packages
                .iter()
                .find(|p| p.binaries.iter().any(|b| b.name == *name));
            if owner.is_some() || exists(&path) {
                taken.push((name.clone(), owner.map(|p| p.name.clone())));
                done.replaced.push(name.clone());
            } else {
                done.added.push(name.clone());
            }
        }
        if !taken.is_empty() && !force {
            return Err(Error::Collision { bin, files: taken });
        }
        for other in &mut packages {
            other
                .binaries
                .retain(|b| files.iter().all(|(name, _)| *name != b.name));
        }
        packages.retain(|p| !p.binaries.is_empty());

        let mut out: Vec<&str> = done
            .replaced
            .iter()
            .map(String::as_str)
            .filter(|name| exists(&bin.join(name)))
            .collect();
        for stale in &earlier {
            if files.iter().any(|(name, _)| *name == stale.name) {
                continue;
            }
            match installed_sha256(&bin.join(&stale.name))? {
                Some(hash) if hash != stale.sha256 => done.left.push(stale.name.clone()),
                Some(_) => {
                    out.push(&stale.name);
                    done.removed.push(stale.name.clone());
                }
                None => {}
            }
        }

        let tmp = self.tmp()?;
        io_at(&bin, fs::create_dir_all(&bin))?;
        let mut staged = Vec::new();
        package.binaries.clear();
        for (index, (name, built)) in files.iter().enumerate() {
            let copy = tmp.join(format!("new-{index}"));
            io_at(built, fs::copy(built, &copy))?;
            let sha256 = io_at(&copy, sha256(&copy))?;
            package.binaries.push(Binary {
                name: name.clone(),
                sha256,
            });
            staged.push((copy, name.as_str()));
        }
        package.binaries.sort_by(|a, b| a.name.cmp(&b.name));
        packages.push(package);
        swap(&bin, &tmp, &out, &staged, || self.write(packages))?;
        self.sweep();
        Ok(done)
    }

    /// Remove `package`'s executables, or only those named in
    /// `binaries`, and return the file names removed from the record.
    /// A file already gone is fine.
    ///
    /// # Errors
    /// [`Error::Modified`], with nothing removed, when a file is not
    /// the one Cabin installed.
    pub fn uninstall(
        &self,
        package: &str,
        binaries: &[String],
        on_wait: impl FnOnce(),
    ) -> Result<Vec<String>, Error> {
        // A root nothing was installed into is not created to say so.
        if !exists(&self.record()) {
            return Err(Error::NotInstalled(package.to_owned()));
        }
        let _lock = self.lock(on_wait)?;
        let bin = self.bin();
        let mut packages = self.packages()?;
        let index = packages
            .iter()
            .position(|p| p.name == package)
            .ok_or_else(|| Error::NotInstalled(package.to_owned()))?;
        let owned = &mut packages[index].binaries;
        if let Some(binary) = binaries
            .iter()
            .find(|name| owned.iter().all(|b| b.name != **name))
        {
            return Err(Error::NotABinary {
                package: package.to_owned(),
                binary: binary.clone(),
            });
        }
        let (gone, kept): (Vec<Binary>, Vec<Binary>) = std::mem::take(owned)
            .into_iter()
            .partition(|b| binaries.is_empty() || binaries.contains(&b.name));
        let mut out = Vec::new();
        let mut changed = Vec::new();
        for binary in &gone {
            let path = bin.join(&binary.name);
            match installed_sha256(&path)? {
                Some(hash) if hash != binary.sha256 => changed.push(path),
                Some(_) => out.push(binary.name.as_str()),
                None => {}
            }
        }
        if !changed.is_empty() {
            return Err(Error::Modified(changed));
        }
        if kept.is_empty() {
            packages.remove(index);
        } else {
            packages[index].binaries = kept;
        }
        let tmp = self.tmp()?;
        swap(&bin, &tmp, &out, &[], || self.write(packages))?;
        self.sweep();
        Ok(gone.into_iter().map(|b| b.name).collect())
    }

    /// Held until the returned file is dropped.
    fn lock(&self, on_wait: impl FnOnce()) -> Result<File, Error> {
        let state = self.state();
        io_at(&state, fs::create_dir_all(&state))?;
        let path = state.join("lock");
        let file = io_at(
            &path,
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path),
        )?;
        let locked = match file.try_lock() {
            Ok(()) => Ok(()),
            Err(TryLockError::WouldBlock) => {
                on_wait();
                file.lock()
            }
            Err(TryLockError::Error(err)) => Err(err),
        };
        io_at(&path, locked)?;
        Ok(file)
    }

    /// A new, empty directory for one change's files.
    fn tmp(&self) -> Result<PathBuf, Error> {
        let tmp = self.state().join("tmp");
        io_at(&tmp, fs::create_dir_all(&tmp))?;
        // A directory an earlier change left cannot be reused: on
        // Windows it may hold a replaced executable that still runs.
        let mut n = 0u32;
        loop {
            let dir = tmp.join(n.to_string());
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(source) => return Err(Error::Io { path: dir, source }),
            }
        }
    }

    /// Delete what finished and interrupted changes left in `tmp/`,
    /// as far as the system allows: Windows keeps a running
    /// executable until its last process exits.
    fn sweep(&self) {
        let _ = fs::remove_dir_all(self.state().join("tmp"));
    }
}

/// Rename each of `out` from `bin` into `tmp` and each of `staged`
/// into `bin`, then run `record`.  When a step fails, the renames
/// made are undone and its error returned.
fn swap(
    bin: &Path,
    tmp: &Path,
    out: &[&str],
    staged: &[(PathBuf, &str)],
    record: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut rename = |from: PathBuf, to: PathBuf| {
        io_at(&to, fs::rename(&from, &to))?;
        renamed.push((from, to));
        Ok(())
    };
    let moved = out
        .iter()
        .enumerate()
        .try_for_each(|(index, name)| rename(bin.join(name), tmp.join(format!("old-{index}"))))
        .and_then(|()| {
            staged
                .iter()
                .try_for_each(|(copy, name)| rename(copy.clone(), bin.join(name)))
        });
    let result = moved.and_then(|()| record());
    if result.is_err() {
        for (from, to) in renamed.iter().rev() {
            // The first failure is the one to report.
            let _ = fs::rename(to, from);
        }
    }
    result
}

/// Whether anything is at `path`, a dangling symbolic link included.
fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn sha256(path: &Path) -> io::Result<String> {
    File::open(path).and_then(cabin_core::hash::hash_reader)
}

/// The SHA-256 of the file at `path`, or `None` when nothing is there.
fn installed_sha256(path: &Path) -> Result<Option<String>, Error> {
    match sha256(path) {
        Ok(hash) => Ok(Some(hash)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn io_at<T>(path: &Path, result: io::Result<T>) -> Result<T, Error> {
    result.map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}
