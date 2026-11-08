//! Linux: Landlock filesystem rules and a seccomp filter, applied in the
//! child between `fork` and `exec` and inherited by everything it starts.
//!
//! Landlock (ABI 3 required) limits reads and executes to the system
//! directories below (but the user's home) plus the granted paths, and
//! writes to the granted directories and `/dev/null`.  It also stops the
//! program from tracing or reading `/proc/<pid>/environ` of processes
//! outside its domain, so Cabin's own environment stays hidden.  On ABI 4
//! or newer it also denies TCP `bind`/`connect`, and on ABI 6 or newer
//! signals to processes outside the domain.
//!
//! The seccomp filter makes these fail with `EPERM`: `socket(2)`, and
//! `socketpair(2)` but for connected `AF_UNIX` pairs (it accepts the TIPC
//! network protocol, and a datagram end can send to any socket path); the
//! `io_uring` calls (file access around Landlock); the
//! kernel keyrings (Kerberos tickets live there), System V IPC, and POSIX
//! message queues (`mq_unlink` bypasses Landlock), which reach other
//! processes' data; and `kill(-1, …)`, which would signal
//! every process of the user.  It makes `setsid(2)` and `setpgid(2)`
//! succeed without effect, so no descendant leaves the process group Cabin
//! kills while callers such as ninja's `posix_spawn` still work.  It kills
//! the process on any other system call ABI (i386 `socketcall`, x32).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

use landlock::{
    ABI, Access as _, AccessFs, AccessNet, CompatLevel, Compatible as _, Ruleset, RulesetAttr as _,
    RulesetCreated, RulesetCreatedAttr as _, RulesetError, Scope, path_beneath_rules,
};

use crate::{Confinement, SandboxError, unix};

/// What starting a program needs: the dynamic loader, libraries,
/// configuration, and the kernel's pseudo-filesystems.  Missing entries
/// are skipped.
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc",
    "/opt",
    "/nix/store",
    "/proc",
    "/sys",
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
];

pub(crate) fn run(confinement: Confinement) -> Result<ExitStatus, SandboxError> {
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
    // Compare resolved paths: a rule follows symlinks, and on a
    // merged-`/usr` system `/lib` is `/usr/lib`.
    let resolved = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
        paths
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .collect()
    };
    let hidden = crate::resolve_hidden(&hidden).map_err(|source| SandboxError::Run {
        program: program.clone(),
        source,
    })?;
    // The system grants leave out the user's home should it lie under one
    // (`/home` linked to `/usr/home`); the caller's grants are kept.
    let home = std::env::home_dir().and_then(|home| home.canonicalize().ok());
    let system = without_hidden(
        resolved(SYSTEM_READ.iter().map(PathBuf::from).collect()),
        &[&hidden[..], home.as_slice()].concat(),
    );
    let read = [system, without_hidden(resolved(read), &hidden)].concat();
    let ruleset = ruleset(&read, &write).map_err(|err| {
        SandboxError::Unavailable(format!(
            "Landlock ABI 3 (Linux 6.2) or newer is required: {err}"
        ))
    })?;
    let filter = seccomp_filter();
    let mut command = Command::new(&program);
    command.args(args);
    unix::configure(&mut command, &current_dir, env, stdout, stderr);
    let parent = std::process::id();
    let mut ruleset = Some(ruleset);
    // SAFETY: the closure runs in the forked child, where only
    // async-signal-safe work is allowed: it makes system calls on data
    // prepared before the fork and never allocates.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut command, move || {
            // Die with the thread that started us, should Cabin be killed
            // outright; if it already died, do not start at all.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if u32::try_from(libc::getppid()) != Ok(parent) {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            // Also sets `PR_SET_NO_NEW_PRIVS`, which seccomp requires.
            ruleset
                .take()
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?
                .restrict_self()
                .map_err(|_| io::Error::from_raw_os_error(libc::EPERM))?;
            let program = libc::sock_fprog {
                len: u16::try_from(filter.len()).unwrap_or(u16::MAX),
                filter: filter.as_ptr().cast_mut(),
            };
            if libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::c_ulong::from(libc::SECCOMP_MODE_FILTER),
                &raw const program,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    unix::run(command, &program)
}

/// `read` with each grant that contains a hidden path replaced by its
/// other entries, recursively: Landlock rules can only grant.  A
/// directory so replaced cannot be listed, and entries created in it
/// later stay unreadable.
fn without_hidden(read: Vec<PathBuf>, hidden: &[PathBuf]) -> Vec<PathBuf> {
    let mut granted = Vec::new();
    let mut pending = read;
    while let Some(path) = pending.pop() {
        if hidden.contains(&path) {
            continue;
        }
        if !hidden.iter().any(|h| h.starts_with(&path)) {
            granted.push(path);
            continue;
        }
        // A symlink's rule would grant its target, which may lie outside
        // the grant or inside a hidden path.  Skipped, a symlink still
        // reaches whatever its target's own rules grant.
        if let Ok(entries) = std::fs::read_dir(&path) {
            pending.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_symlink()))
                    .map(|entry| entry.path()),
            );
        }
    }
    granted
}

fn ruleset(read: &[PathBuf], write: &[PathBuf]) -> Result<RulesetCreated, RulesetError> {
    let all = AccessFs::from_all(ABI::V3);
    let read_only = AccessFs::from_read(ABI::V3);
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(all)?
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessNet::from_all(ABI::V4))?
        .scope(Scope::Signal)?
        .create()?
        .set_compatibility(CompatLevel::HardRequirement)
        .add_rules(path_beneath_rules(read, read_only))?
        .add_rules(path_beneath_rules(write, all))?
        .add_rules(path_beneath_rules(["/dev/null"], AccessFs::WriteFile))
}

/// `AUDIT_ARCH_*` of the native system call ABI (`linux/audit.h`).
#[cfg(target_arch = "x86_64")]
const NATIVE_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const NATIVE_ARCH: u32 = 0xC000_00B7;
/// x32 system calls share x86-64's arch value with this bit set.
#[cfg(target_arch = "x86_64")]
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Built by hand: seccompiler checks the arch value only, so x32 calls
/// would pass its deny rules, and it takes one action per filter.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // BPF fields are fixed-width
fn seccomp_filter() -> Vec<libc::sock_filter> {
    let errno = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let denied = [
        libc::SYS_socket,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_mq_open,
        libc::SYS_mq_unlink,
        libc::SYS_shmget,
        libc::SYS_shmat,
        libc::SYS_shmctl,
        libc::SYS_msgget,
        libc::SYS_msgsnd,
        libc::SYS_msgrcv,
        libc::SYS_msgctl,
        libc::SYS_semget,
        libc::SYS_semop,
        libc::SYS_semtimedop,
        libc::SYS_semctl,
    ];
    let pretended = [libc::SYS_setsid, libc::SYS_setpgid];
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let jeq = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let arch = std::mem::offset_of!(libc::seccomp_data, arch) as u32;
    let nr = std::mem::offset_of!(libc::seccomp_data, nr) as u32;
    // SAFETY: `BPF_STMT` / `BPF_JUMP` only build structs.
    unsafe {
        let mut filter = vec![
            libc::BPF_STMT(load, arch),
            libc::BPF_JUMP(jeq, NATIVE_ARCH, 1, 0),
            libc::BPF_STMT(ret, libc::SECCOMP_RET_KILL_PROCESS),
            libc::BPF_STMT(load, nr),
        ];
        #[cfg(target_arch = "x86_64")]
        filter.extend([
            libc::BPF_JUMP(
                (libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16,
                X32_SYSCALL_BIT,
                0,
                1,
            ),
            libc::BPF_STMT(ret, libc::SECCOMP_RET_KILL_PROCESS),
        ]);
        for call in denied {
            filter.push(libc::BPF_JUMP(jeq, call as u32, 0, 1));
            filter.push(libc::BPF_STMT(ret, errno));
        }
        for call in pretended {
            filter.push(libc::BPF_JUMP(jeq, call as u32, 0, 1));
            filter.push(libc::BPF_STMT(ret, libc::SECCOMP_RET_ERRNO));
        }
        // The low half of `args[0]` (both targets are little-endian).
        let arg0 = std::mem::offset_of!(libc::seccomp_data, args) as u32;
        filter.extend([
            libc::BPF_JUMP(jeq, libc::SYS_kill as u32, 0, 3),
            libc::BPF_STMT(load, arg0),
            libc::BPF_JUMP(jeq, u32::MAX, 0, 1),
            libc::BPF_STMT(ret, errno),
            libc::BPF_STMT(load, nr),
            libc::BPF_JUMP(jeq, libc::SYS_socketpair as u32, 0, 8),
            libc::BPF_STMT(load, arg0),
            libc::BPF_JUMP(jeq, libc::AF_UNIX as u32, 0, 5),
            // The type, without `SOCK_NONBLOCK` / `SOCK_CLOEXEC`.  Listed,
            // not excluded: `AF_UNIX` takes `SOCK_RAW` as `SOCK_DGRAM`.
            libc::BPF_STMT(load, arg0 + 8),
            libc::BPF_STMT((libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16, 0xf),
            libc::BPF_JUMP(jeq, libc::SOCK_STREAM as u32, 1, 0),
            libc::BPF_JUMP(jeq, libc::SOCK_SEQPACKET as u32, 0, 1),
            libc::BPF_STMT(ret, libc::SECCOMP_RET_ALLOW),
            libc::BPF_STMT(ret, errno),
            libc::BPF_STMT(ret, libc::SECCOMP_RET_ALLOW),
        ]);
        filter
    }
}
