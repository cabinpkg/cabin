//! Process handling shared by the Unix backends.
//!
//! The confined program leads a session of its own, so it has no
//! controlling terminal to read from or inject input into (`TIOCSTI`),
//! and its process group is its session's.  On exit the whole group is
//! killed before the leader is reaped, so the group id cannot be reused
//! by an unrelated process in between.  Being outside the terminal's
//! foreground group, the program would miss the terminal's Ctrl-C:
//! [`forward`] kills running groups when Cabin itself is interrupted,
//! terminated, or hung up on.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::SandboxError;

/// Give `command` an empty stdin, the caller's stdout and stderr, the
/// working directory, exactly `env`, and a new session.
pub(crate) fn configure(
    command: &mut Command,
    current_dir: &Path,
    env: Vec<(OsString, OsString)>,
    stdout: File,
    stderr: File,
) {
    command
        .current_dir(current_dir)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(target_os = "macos")]
    let descriptors = descriptor_limit();
    // SAFETY: only async-signal-safe system calls; `close_range(2)` is
    // Linux 5.11, which Landlock ABI 3 implies.
    unsafe {
        command.pre_exec(move || {
            match libc::setsid() {
                -1 => return Err(io::Error::last_os_error()),
                // Under this crate's own Linux filter (a nested run),
                // `setsid` pretends, and the group to kill would not be
                // this program's.
                
                _ => {}
            }
            // Mark every descriptor above stderr close-on-exec.  Rust opens
            // its own that way, but ones Cabin inherited (a shell's
            // `3<file`, a careless parent's socket) would hand the program
            // what it could not open.
            #[cfg(target_os = "linux")]
            if libc::syscall(
                libc::SYS_close_range,
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            #[cfg(target_os = "macos")]
            for fd in 3..descriptors {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }
}

/// One past the highest descriptor number a process can hold.  Inherited
/// ones may sit above the current soft limit (opened before it was
/// lowered), never above the kernel's per-process maximum.
#[cfg(target_os = "macos")]
fn descriptor_limit() -> libc::c_int {
    let mut max: libc::c_int = 0;
    let mut len = std::mem::size_of_val(&max);
    // SAFETY: `max` and `len` are valid for writes of their sizes.
    unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut max).cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        );
        max.max(libc::getdtablesize())
    }
}

/// Spawn `command`, wait for it to exit, kill its process group, then
/// reap it.  `program` names it in errors.
pub(crate) fn run(mut command: Command, program: &Path) -> Result<ExitStatus, SandboxError> {
    let failed = |source| SandboxError::Run {
        program: PathBuf::from(program),
        source,
    };
    let slot = forward::reserve()
        .ok_or_else(|| failed(io::Error::other("too many confined programs are running")))?;
    let mut child = command.spawn().map_err(failed)?;
    let group = libc::pid_t::try_from(child.id()).expect("process ids fit pid_t");
    slot.set(group);
    let exited = wait_without_reaping(group);
    // SAFETY: plain system call.  `group` is the unreaped leader's id, so
    // it still names this program's group.
    unsafe { libc::killpg(group, libc::SIGKILL) };
    drop(slot);
    let status = child.wait();
    exited.map_err(failed)?;
    status.map_err(failed)
}

fn wait_without_reaping(pid: libc::pid_t) -> io::Result<()> {
    let id = libc::id_t::try_from(pid).expect("process ids are positive");
    loop {
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // SAFETY: `info` is valid for writes; `WNOWAIT` leaves the child
        // waitable.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

mod forward {
    use std::sync::Once;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// Programs that may run at once: past that, [`reserve`] fails rather
    /// than start one an interrupt would not reach.
    const SLOTS: usize = 1024;
    const RESERVED: libc::pid_t = -1;
    static GROUPS: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];
    static INSTALL: Once = Once::new();
    const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    pub(super) struct Slot(usize);

    /// Install the handler and claim a slot, before the program exists.
    pub(super) fn reserve() -> Option<Slot> {
        INSTALL.call_once(install);
        GROUPS
            .iter()
            .position(|slot| {
                slot.compare_exchange(0, RESERVED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            })
            .map(Slot)
    }

    impl Slot {
        pub(super) fn set(&self, group: libc::pid_t) {
            GROUPS[self.0].store(group, Ordering::SeqCst);
        }
    }

    impl Drop for Slot {
        fn drop(&mut self) {
            GROUPS[self.0].store(0, Ordering::SeqCst);
        }
    }

    /// Kill every running group, then die of `signal`.
    ///
    /// A program whose spawn has not yet returned is missed: its slot is
    /// still `RESERVED`.  Waiting for it here could deadlock (the
    /// interrupted thread may hold a lock `fork` needs), so a signal in
    /// that window of microseconds leaves the program running, confined;
    /// on Linux the parent-death signal still ends the program itself.
    extern "C" fn on_signal(signal: libc::c_int) {
        for slot in &GROUPS {
            let group = slot.load(Ordering::SeqCst);
            if group > 0 {
                // SAFETY: async-signal-safe system call.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        }
        // Then die of the signal as if this handler were not installed.
        // SAFETY: async-signal-safe; `signal` is blocked until the
        // handler returns, when the default action ends the process.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }

    /// Install [`on_signal`] for the signals that end Cabin by default,
    /// leaving any the user had ignored (`nohup`) or someone else handles.
    fn install() {
        for signal in SIGNALS {
            // SAFETY: zeroed `sigaction` is a valid value; the pointers are
            // valid for the calls.
            unsafe {
                let mut previous: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, std::ptr::null(), &raw mut previous) != 0
                    || previous.sa_sigaction != libc::SIG_DFL
                {
                    continue;
                }
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
                libc::sigemptyset(&raw mut action.sa_mask);
                libc::sigaction(signal, &raw const action, std::ptr::null_mut());
            }
        }
    }
}
