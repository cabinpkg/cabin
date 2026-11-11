//! Run one program under operating-system confinement.
//!
//! Cabin runs code a package supplies (its build script) through this
//! crate.  [`run`] starts [`Confinement::program`] so that it, and every
//! process it starts:
//!
//! - reads nothing under the user's home directory, or any other place
//!   user data lives, but the caller's [`Confinement::read`] paths (on
//!   Linux, it reads only those and what starting a program needs);
//! - writes only under [`Confinement::write`] (and to `/dev/null`);
//! - opens no network connection;
//! - sees exactly [`Confinement::env`]: the environment is cleared first,
//!   and descriptors other than the standard streams are not inherited.
//!
//! Before [`run`] returns, every process the program started is sent
//! `SIGKILL`.
//!
//! Mechanisms: Linux Landlock (ABI 3, Linux 6.2, or newer) plus a
//! seccomp filter, on 64-bit x86 and Arm; macOS `sandbox-exec`.  Other
//! platforms are not supported yet.  When the mechanism is missing,
//! [`run`] fails with [`SandboxError::Unavailable`]: it never starts the
//! program unconfined.
//!
//! Gaps the mechanisms leave, which a caller running untrusted code
//! must weigh:
//!
//! - Linux: Landlock does not govern metadata, so the program can see
//!   whether any path exists, read its size and times, and change the
//!   mode, times, and extended attributes of any file its user owns.  It
//!   can read the command line of every process.  Before Linux 6.12
//!   (Landlock ABI 6) it can signal any single process of its user.
//! - macOS: reads are denied only where user data lives: `/Users`,
//!   `/Volumes`, the temporary directories, and the home directory.  A
//!   file elsewhere, such as on a volume mounted at another path, stays
//!   readable.  The program can read the command line and environment any
//!   process of its user was started with (`sysctl` `KERN_PROCARGS2`),
//!   which Seatbelt cannot deny.  A process that leaves the program's
//!   process group (`setsid(2)`, `setpgid(2)`) escapes the final kill, as
//!   does everything when Cabin itself is killed outright; it stays
//!   confined.  `DYLD_*` variables never reach the program, because the
//!   system strips them when starting `sandbox-exec`.
//! - Both: run by root, the program keeps root's privileges, with which it
//!   can reach past the confinement (create a device node in a write
//!   grant, for one).  Cabin is not meant to run as root.
//!
//! Choosing what to grant (and which environment variables are safe to
//! pass) is the caller's policy; this crate only enforces it.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
))]
mod unix;

/// One confined program run.  Every path is absolute.
#[derive(Debug)]
pub struct Confinement {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Must lie under one of [`Self::read`] or [`Self::write`] for the
    /// program to list it.
    pub current_dir: PathBuf,
    /// The program's whole environment.
    pub env: Vec<(OsString, OsString)>,
    /// Files and directories (recursively) the program may read and
    /// execute.  [`Self::program`] must be under one of them.  Missing
    /// paths are skipped, here and in [`Self::write`].
    pub read: Vec<PathBuf>,
    /// Directories (recursively) the program may read, write, create in,
    /// and delete from.  The directory itself it can neither remove nor
    /// move away, nor make anything in its place.  No path granted to
    /// this or a later run may lie inside one, where the program could
    /// put a symlink in its way.
    pub write: Vec<PathBuf>,
    /// Paths beneath a [`Self::read`] grant that stay unreadable, such as
    /// a package's `.git`.  None may contain a granted path or lie inside
    /// a [`Self::write`] grant (the program could rename an ancestor).  One
    /// with a `..` after a missing directory fails the run.  On Linux the
    /// directory holding one cannot be listed.
    pub hidden: Vec<PathBuf>,
    pub stdout: File,
    pub stderr: File,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The platform cannot confine the program, so it was not started.
    #[error("cannot confine the program on this system: {0}")]
    Unavailable(String),
    #[error("failed to run {}: {source}", program.display())]
    Run {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// `hidden` as the grants see it: each path with its ancestors resolved,
/// plus where it leads (a symlinked `.git`'s target).  Neither need exist
/// yet: each stays unreadable should it appear during the run.  One whose
/// destination cannot be told (see [`resolve`]) fails the run.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
))]
fn resolve_hidden(hidden: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    for path in hidden {
        let spelled = path
            .parent()
            .and_then(resolve)
            .zip(path.file_name())
            .map(|(parent, name)| parent.join(name));
        let (Some(spelled), Some(leads)) = (spelled, resolve(path)) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot tell where hidden path {} leads", path.display()),
            ));
        };
        resolved.extend([spelled, leads]);
    }
    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

/// Where `path` leads: its longest existing prefix resolved, and dangling
/// symlinks followed to where their targets would appear.  `None` when
/// that cannot be told: a `..` after a missing directory (it leads
/// wherever what appears there leads), a symlink that cannot be read,
/// more than 40 symlinks (Linux's `MAXSYMLINKS`; a loop), or a path
/// longer than Linux's `PATH_MAX`, which bounds the work.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
))]
fn resolve(path: &std::path::Path) -> Option<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..=40 {
        if path.as_os_str().len() > 4096 {
            return None;
        }
        let (prefix, resolved) = path
            .ancestors()
            .find_map(|prefix| Some((prefix, prefix.canonicalize().ok()?)))?;
        let rest = path.strip_prefix(prefix).ok()?;
        let Some(next) = rest.iter().next() else {
            return Some(resolved);
        };
        match std::fs::read_link(resolved.join(next)) {
            Ok(target) => path = resolved.join(target).join(rest.strip_prefix(next).ok()?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let plain = rest
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)));
                return plain.then(|| resolved.join(rest));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Run `confinement.program` confined, wait for it, kill whatever it
/// left running, and return its exit status.  stdin is empty.
///
/// # Errors
/// [`SandboxError::Unavailable`] when the platform cannot enforce the
/// confinement; [`SandboxError::Run`] when the program could not be
/// started or waited for.
pub fn run(confinement: Confinement) -> Result<ExitStatus, SandboxError> {
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    return linux::run(confinement);
    #[cfg(target_os = "macos")]
    return macos::run(confinement);
    #[cfg(not(any(
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        target_os = "macos"
    )))]
    {
        drop(confinement);
        Err(SandboxError::Unavailable(
            "Cabin has no sandbox for this platform yet".to_owned(),
        ))
    }
}
