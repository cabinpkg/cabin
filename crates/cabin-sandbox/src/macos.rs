//! macOS: run the program through `/usr/bin/sandbox-exec` with a
//! Seatbelt profile, which the program and everything it starts inherit.
//!
//! The profile allows by default, then denies the network, reads under
//! the places user data lives (`/Users`, `/Volumes`, the per-user and
//! shared temporary directories, and `$HOME`), every write, mounting and
//! unmounting (a user may mount `devfs` over a directory they own and
//! unmount what they mounted), signals to and task ports of processes
//! outside the sandbox (a program signed to debug would read their
//! memory), and what reaches outside it: shared memory and semaphores,
//! launchd jobs, Apple events, `open`, and every Mach service but
//! logging, notifications, and user lookup (daemons such as the
//! keychain's or the DNS resolver would otherwise act on the program's
//! behalf).  It then re-allows the granted paths, and the metadata alone
//! of each directory leading to one and of the files the standard output
//! and error go to (a program resolves the paths it is given, and examines
//! those streams before it writes to them), and finally denies the hidden
//! paths inside the grants, and removing, renaming, or making anything in
//! place of a writable directory itself.  Of two rules for one operation
//! the later wins, and a rule naming an operation outranks one that covers
//! it by wildcard wherever it stands, so the last deny names the metadata
//! too.  Paths are passed as `-D` parameters, never spliced into the
//! profile text.

use std::collections::BTreeSet;
use std::ffi::{CStr, OsStr, OsString};
use std::fmt::Write as _;
use std::fs::File;
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::OnceLock;

use crate::{Confinement, SandboxError, unix};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Paths are canonical: Seatbelt matches the resolved path, so
/// `/var/folders` must be spelled `/private/var/folders`.
const DENIED_READS: &[&str] = &[
    "/Users",
    "/Volumes",
    "/private/var/folders",
    "/private/tmp",
    "/private/var/tmp",
];

pub(crate) fn run(confinement: Confinement) -> Result<ExitStatus, SandboxError> {
    ensure_available()?;
    let Confinement {
        program,
        args,
        current_dir,
        env,
        read,
        write,
        hidden,
        stdout,
        stderr,
    } = confinement;
    let home = std::env::home_dir().and_then(|home| home.canonicalize().ok());
    let read = canonical(&read);
    let write = canonical(&write);
    let hidden = crate::resolve_hidden(&hidden).map_err(|source| SandboxError::Run {
        program: program.clone(),
        source,
    })?;
    let granted = read.iter().chain(&write);
    let leading = granted.flat_map(|path| path.ancestors().skip(1));
    let streams = [&stdout, &stderr].into_iter().filter_map(path_of);
    let seen: BTreeSet<PathBuf> = leading.map(Path::to_path_buf).chain(streams).collect();
    let mut command = Command::new(SANDBOX_EXEC);
    command.arg("-p").arg(profile(
        home.is_some(),
        read.len(),
        write.len(),
        seen.len(),
        hidden.len(),
    ));
    let params = home
        .iter()
        .map(|home| ("HOME".to_owned(), home))
        .chain(read.iter().enumerate().map(|(i, p)| (format!("R{i}"), p)))
        .chain(write.iter().enumerate().map(|(i, p)| (format!("W{i}"), p)))
        .chain(seen.iter().enumerate().map(|(i, p)| (format!("M{i}"), p)))
        .chain(hidden.iter().enumerate().map(|(i, p)| (format!("H{i}"), p)));
    for (key, path) in params {
        let mut definition = OsString::from(key);
        definition.push("=");
        definition.push(path);
        command.arg("-D").arg(definition);
    }
    command.arg("--").arg(&program).args(args);
    unix::configure(&mut command, &current_dir, env, stdout, stderr);
    unix::run(command, &program)
}

/// Where an open file lies, as Seatbelt spells it; `None` for one
/// without a path, such as a pipe.
fn path_of(file: &File) -> Option<PathBuf> {
    let mut path = [0; libc::PATH_MAX as usize];
    // SAFETY: `F_GETPATH` writes a NUL-terminated path of at most
    // `PATH_MAX` bytes.
    let found = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) } != -1;
    let path = CStr::from_bytes_until_nul(&path).ok().filter(|_| found)?;
    Some(OsStr::from_bytes(path.to_bytes()).into())
}

/// Missing paths grant nothing, as on Linux.
fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect()
}

/// The profile for `reads` granted read paths (`R0`..), `writes`
/// granted write paths (`W0`..), `seen` paths of which only the metadata
/// is read (`M0`..), and `hidden` paths (`H0`..) inside the grants,
/// denying reads under `HOME` when set.
fn profile(home: bool, reads: usize, writes: usize, seen: usize, hidden: usize) -> String {
    let params = |filter: &str, prefix: &str, count: usize| -> String {
        (0..count).fold(String::new(), |mut out, i| {
            let _ = write!(out, " ({filter} (param \"{prefix}{i}\"))");
            out
        })
    };
    let mut denied = DENIED_READS.iter().fold(String::new(), |mut out, path| {
        let _ = write!(out, " (subpath \"{path}\")");
        out
    });
    if home {
        denied.push_str(" (subpath (param \"HOME\"))");
    }
    let mut profile = format!(
        r#"(version 1)
(allow default)
(deny network*)
(deny job-creation)
(deny appleevent-send)
(deny lsopen)
(deny ipc-posix-shm*)
(deny ipc-posix-sem*)
(deny ipc-sysv*)
(deny signal)
(allow signal (target same-sandbox))
(deny mach-priv-task-port)
(allow mach-priv-task-port (target same-sandbox))
(deny mach-lookup)
(allow mach-lookup (global-name "com.apple.system.logger")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.system.opendirectoryd.membership"))
(deny file-read*{denied})
(deny file-write*)
(deny file-mount)
(deny file-unmount)
(allow file-write* (literal "/dev/null"))
"#
    );
    let (read, write) = (
        params("subpath", "R", reads),
        params("subpath", "W", writes),
    );
    if !read.is_empty() {
        let _ = writeln!(profile, "(allow file-read*{read})");
    }
    if !write.is_empty() {
        let _ = writeln!(profile, "(allow file-read* file-write*{write})");
        // A symlink the program put in a directory's place would lead the
        // next run's grant elsewhere.
        let roots = params("literal", "W", writes);
        let _ = writeln!(profile, "(deny file-write-create file-write-unlink{roots})");
    }
    let seen = params("literal", "M", seen);
    if !seen.is_empty() {
        let _ = writeln!(profile, "(allow file-read-metadata{seen})");
    }
    let hidden = params("subpath", "H", hidden);
    if !hidden.is_empty() {
        let _ = writeln!(profile, "(deny file-read* file-read-metadata{hidden})");
    }
    profile
}

/// `sandbox-exec` cannot apply a profile inside a process that is
/// already sandboxed (it exits 71 before starting the program), which
/// would read as the program's own failure.  Probe once.
fn ensure_available() -> Result<(), SandboxError> {
    static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let output = Command::new(SANDBOX_EXEC)
                .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
                .env_clear()
                .output()
                .map_err(|err| format!("cannot run {SANDBOX_EXEC}: {err}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{SANDBOX_EXEC} cannot apply a profile here ({}); is Cabin itself running inside a sandbox? {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim(),
                ))
            }
        })
        .clone()
        .map_err(SandboxError::Unavailable)
}
