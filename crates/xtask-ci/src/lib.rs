//! Local mirror of the CI gate (`rust.yml`, `ci.yml`, `website.yml`).
//! Expensive checks are scoped to the surfaces changed relative to
//! `origin/main`.
//!
//! The signal-safe process management underneath the gate -
//! [`spawn_tracked`], [`reap`], [`terminate`], [`masked`],
//! [`arm_teardown`] and the teardown-time file restore - is also the shared child-process
//! layer for the other xtask crates that run long-lived children
//! (the registry smoke test's dev servers and mocks).
//!
//! The cargo phases and the website block are independent and share no
//! build artifacts (clippy compiles under its own driver; check, test
//! and doc differ in `RUSTFLAGS`, feature set, or compiler), so they
//! run concurrently, each cargo phase in its own target directory -
//! cargo's target-dir lock is exclusive, so a shared directory would
//! serialize them right back.  The first run in a checkout pays a cold
//! build in each directory; after that every run is incremental, and
//! keeping the gate's `-D warnings` builds out of `target/` also stops
//! them from invalidating ordinary iteration builds.
//!
//! Ceilings:
//!
//! - the teardown is Unix-only.  Every phase - plus, in hook mode,
//!   the serial steps - runs in its own process group and a
//!   `SIGTERM`/`SIGINT`/`SIGHUP` tears the groups down; on a non-Unix
//!   host the children are ordinary processes and a cancelled gate
//!   can orphan them;
//! - a teardown reaches only the processes still in a phase's group:
//!   one that moved to a group of its own (`setsid`, or a tool's
//!   per-child groups) stops only if its parent stops it on SIGTERM,
//!   before the SIGKILL;
//! - the signal masks that keep the handler out of a spawn and its
//!   table record, or a reap and its table removal, block only the
//!   thread that spawns or reaps, so they hold while no other thread
//!   can take a teardown signal then: spawn any other thread under
//!   [`masked`];
//! - a teardown signal arriving while a tracked child is being
//!   spawned stays pending until the spawn returns, because the
//!   spawn-and-record pair runs under a signal mask, so a stalled
//!   `exec` defers the cancellation for as long as it stalls -
//!   deferred, not lost.

pub mod cores;
pub mod hook;
pub mod scope;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

/// The repository this gate checks.
///
/// Resolved at RUNTIME through git: the gate checks the tree it is
/// invoked in, which is what makes it correct inside a `git worktree`.
/// Taking the crate's compile-time manifest directory instead would
/// pin every run to whichever checkout the binary happened to be built
/// in - a copied or worktree-shared binary would then check the wrong
/// tree and report its verdict about someone else's changes.
///
/// # Errors
///
/// If the working directory is not inside a git repository.
pub fn repo_root() -> Result<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("run git rev-parse --show-toplevel")?;
    if !out.status.success() {
        bail!("not inside a git repository");
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if root.is_empty() {
        bail!("git named no repository root");
    }
    Ok(PathBuf::from(root))
}

/// One concurrent phase: a child, its label, and the log its output
/// goes to.  The log is replayed only on failure, followed by a final
/// `==> <name>` marker, so the hook adapter's "last `==>` line names
/// the failed step" contract holds in both modes.
struct Phase {
    name: String,
    child: Child,
    log: std::fs::File,
}

/// The gate's runner: it either backgrounds each check as a phase or
/// runs them in the foreground, depending on how many cores there are
/// to divide.
pub struct Gate {
    root: PathBuf,
    parallel: bool,
    phases: Vec<Phase>,
    /// Where the gate's own narration goes.  In hook mode this is a
    /// buffer, because stdout there carries only the JSON decision -
    /// and because the failed-step marker has to be recoverable from
    /// it.
    out: Box<dyn std::io::Write + Send>,
    /// Where a serial child's stdout and stderr go.  Inherited
    /// normally; captured in hook mode for the same two reasons.
    capture: bool,
}

impl Gate {
    #[must_use]
    pub fn new(
        root: PathBuf,
        parallel: bool,
        out: Box<dyn std::io::Write + Send>,
        capture: bool,
    ) -> Self {
        Self {
            root,
            parallel,
            phases: Vec::new(),
            out,
            capture,
        }
    }

    /// An anonymous scratch file for a child's output, as a (writer,
    /// reader) pair.  `NamedTempFile` creates it exclusively under a
    /// random name with mode 0600; `reopen` gives the reader an
    /// INDEPENDENT file description, because it must not share the
    /// writer's cursor - a grandchild still holding the write end
    /// after the reaped child exits would otherwise have its
    /// position moved by the replay's seek.  Dropping the `TempPath`
    /// unlinks the file right away, so no path is left for a
    /// squatter to attack and an unlinked log cannot outlive the
    /// gate on any exit path.  A FAILED unlink is ignored; the
    /// straggler is an unreadable 0600 file.
    fn log() -> Result<(std::fs::File, std::fs::File)> {
        let file = tempfile::NamedTempFile::new().context("create a scratch log file")?;
        let reader = file.reopen().context("reopen the scratch log file")?;
        let (writer, path) = file.into_parts();
        drop(path);
        Ok((writer, reader))
    }

    /// Everything a finished child wrote to its log, as bytes: one
    /// invalid byte in compiler output must not drop the whole replay.
    fn replay(log: &mut std::fs::File) -> Vec<u8> {
        use std::io::{Read as _, Seek as _};
        let mut output = Vec::new();
        if log.seek(std::io::SeekFrom::Start(0)).is_ok() {
            let _ = log.read_to_end(&mut output);
        }
        output
    }

    /// The gate's narration, which the hook adapter reads back to find
    /// the failed step.
    ///
    /// # Errors
    ///
    /// If the sink cannot be written.
    pub fn say(&mut self, line: &str) -> Result<()> {
        writeln!(self.out, "{line}").context("write the gate narration")?;
        self.out.flush().ok();
        Ok(())
    }

    /// A serial step: announced, run in the foreground, and fatal.
    ///
    /// Tracked for teardown only in hook mode, where the whole gate
    /// must stay killable.  Otherwise a serial step stays in the
    /// foreground group, where moving it would trade the terminal's
    /// signal delivery for a `SIGTTIN`/`SIGTTOU` stop the gate would
    /// then wait on forever.
    ///
    /// # Errors
    ///
    /// If the program cannot be spawned or exits non-zero.
    pub fn step(&mut self, name: &str, command: &mut Command) -> Result<()> {
        self.say(&format!("==> {name}"))?;
        if command.get_current_dir().is_none() {
            command.current_dir(&self.root);
        }
        let status = if self.capture {
            // A serial child inherits stdout by default, which in hook
            // mode would put compiler output ahead of the JSON.
            let (writer, mut log) = Self::log()?;
            let errors = writer.try_clone().context("clone the step log handle")?;
            let mut child = spawn_tracked(
                command
                    .stdout(Stdio::from(writer))
                    .stderr(Stdio::from(errors)),
            )
            .with_context(|| format!("run {name}"))?;
            let status = reap(&mut child).with_context(|| format!("wait for {name}"))?;
            let _ = self.out.write_all(&Self::replay(&mut log));
            status
        } else {
            command.status().with_context(|| format!("run {name}"))?
        };
        if !status.success() {
            bail!("{name} failed: {status}");
        }
        Ok(())
    }

    /// A check, backgrounded as a phase when the machine has cores to
    /// spare and run in the foreground otherwise.
    ///
    /// # Errors
    ///
    /// If the program cannot be spawned, or - in serial mode - exits
    /// non-zero.
    pub fn launch(&mut self, name: &str, command: &mut Command) -> Result<()> {
        if !self.parallel {
            return self.step(name, command);
        }
        let (writer, log) = Self::log()?;
        let errors = writer.try_clone().context("clone the phase log handle")?;
        self.say(&format!("==> {name} (started)"))?;
        let child = spawn_tracked(
            command
                .current_dir(&self.root)
                .stdout(Stdio::from(writer))
                .stderr(Stdio::from(errors)),
        )
        .with_context(|| format!("run {name}"))?;
        self.phases.push(Phase {
            name: name.to_owned(),
            child,
            log,
        });
        Ok(())
    }

    /// Waits for every phase, replaying the output of those that
    /// failed.
    ///
    /// # Errors
    ///
    /// If any phase exited non-zero.
    pub fn finish(&mut self) -> Result<()> {
        let mut failed = false;
        // Removed one at a time, never iterated in place: a phase must
        // leave the list BEFORE it is reaped (a reaped pid must never
        // be visible to `drop`'s group-kill below), while the phases a
        // panic keeps this loop from reaching stay listed for `drop`
        // to tear down.
        while !self.phases.is_empty() {
            let mut phase = self.phases.remove(0);
            let status = reap(&mut phase.child);
            let ok = status.is_ok_and(|status| status.success());
            if ok {
                let _ = writeln!(self.out, "    ok: {}", phase.name);
            } else {
                failed = true;
                let _ = writeln!(self.out, "\n--- failed phase output: {} ---", phase.name);
                let _ = self.out.write_all(&Self::replay(&mut phase.log));
                // After the output, never before: the `(started)`
                // markers were all printed up front, so this is what
                // makes the last `==>` line name the phase that failed
                // rather than the one launched last.
                let _ = writeln!(self.out, "\n==> {}", phase.name);
            }
        }
        if failed {
            bail!("a phase failed");
        }
        Ok(())
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        // The logs need no cleanup here - they were already unlinked
        // (or, on a failed unlink, deliberately abandoned).
        //
        // The whole GROUP, not `Child::kill`: a phase is an `npm` or
        // `cargo` tree, and killing only the direct child on an unwind
        // leaves its grandchildren running with PPID 1.  Every phase
        // still listed here is one `finish` never reached (it removes
        // each entry before reaping it), so none has been reaped, as
        // `terminate` requires.
        terminate(self.phases.iter_mut().map(|phase| &mut phase.child));
    }
}

/// Puts a child in its own process group, so a teardown can kill the
/// phase's whole tree - npm's node children included - rather than
/// just the process this crate spawned.
///
/// The child's signal mask is reset before exec.  The spawn happens
/// under [`teardown::masked`]; `std::process::Command` passes the
/// parent's mask to the child and `exec` preserves it, so without the
/// reset every child inherits blocked TERM/INT/HUP and is permanently
/// immune to the very teardown the mask protects (observed empirically;
/// `ps` reports such a child's mask as clear).
#[cfg(unix)]
fn group(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt as _;
    unsafe {
        // Only async-signal-safe calls are allowed here.
        command.pre_exec(|| {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &raw const empty, std::ptr::null_mut());
            Ok(())
        });
    }
    command.process_group(0)
}

#[cfg(not(unix))]
fn group(command: &mut Command) -> &mut Command {
    command
}

/// Spawns a child in its own process group and records it for the
/// teardown handler, with the teardown signals blocked across both,
/// so the handler cannot run between the spawn and the record, when
/// its kill loop would miss the child.
///
/// Public for the other xtask crates that manage long-running
/// children (the registry smoke test's dev servers and mocks); the
/// caller must have armed the teardown with [`arm_teardown`].
///
/// # Errors
///
/// If the program cannot be spawned.
pub fn spawn_tracked(command: &mut Command) -> std::io::Result<Child> {
    masked(|| {
        let child = group(command).spawn()?;
        record(child.id());
        Ok(child)
    })
}

/// Stops tracked children's whole process groups the way the teardown
/// handler does - SIGTERM, then SIGKILL to what is left once every
/// child has exited or the grace period has passed - and [`reap`]s each
/// child that exited.  One the SIGKILL wait leaves running (stuck in an
/// uninterruptible sleep) stays unreaped and in the teardown table,
/// which then still names a pid this process holds.  Off Unix,
/// `Child::kill` and [`reap`].
///
/// Every child must be one [`spawn_tracked`] started and nothing has
/// reaped yet: an unreaped child keeps its pid, and so the id of the
/// group it leads, from naming any other process.
pub fn terminate<'a>(children: impl IntoIterator<Item = &'a mut Child>) {
    #[cfg(unix)]
    {
        let children: Vec<&mut Child> = children.into_iter().collect();
        let leaders: Vec<u32> = children.iter().map(|child| child.id()).collect();
        teardown::stop(&leaders);
        for child in children {
            if teardown::exited(child.id()) {
                let _ = reap(child);
            }
        }
    }
    #[cfg(not(unix))]
    for child in children {
        let _ = child.kill();
        let _ = reap(child);
    }
}

/// Waits for a tracked child without letting its pid become reusable
/// while the teardown handler could still signal it: first a wait
/// that does NOT reap (`WNOWAIT` - the child stays a zombie, and a
/// zombie's pid cannot be recycled), then the real reap and the
/// table removal together under the signal mask, so a recycled pid
/// is not signaled.  The one exception is the fallback below for a
/// failed `waitid`, which accepts the window between the reap and the
/// table removal rather than waiting masked for the child's whole
/// runtime.
///
/// # Errors
///
/// If the wait itself fails.
#[cfg(unix)]
pub fn reap(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    let exited = loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let found = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                &raw mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if found == 0 {
            break true;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break false;
        }
    };
    if exited {
        masked(|| {
            let status = child.wait();
            forget(child.id());
            status
        })
    } else {
        let status = child.wait();
        forget(child.id());
        status
    }
}

/// # Errors
///
/// If the wait itself fails.
#[cfg(not(unix))]
pub fn reap(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    let status = child.wait();
    forget(child.id());
    status
}

#[cfg(unix)]
mod teardown {
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

    /// The live phase groups, for the signal handler.  A fixed array
    /// of atomics rather than a lock: the handler runs in signal
    /// context, where taking a mutex can deadlock against the very
    /// thread it interrupted.
    static GROUPS: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
    const SLOTS: usize = 8;

    /// Whether a cancelled gate exits 0, as the hook adapter promises
    /// on every path ([`crate::hook`]).
    static HOOKED: AtomicBool = AtomicBool::new(false);

    pub fn hooked() {
        HOOKED.store(true, Ordering::SeqCst);
    }

    pub fn record(pid: u32) {
        for slot in &GROUPS {
            if slot
                .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Drops a reaped group, so a later cancellation cannot signal a
    /// recycled, unrelated process group.
    pub fn forget(pid: u32) {
        for slot in &GROUPS {
            let _ = slot.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// A working-tree file the handler must put back before `_exit`:
    /// the target path, and the backup to rename over it (null = the
    /// file did not exist, so unlink it).  Raw leaked `CString`s in
    /// atomics because the handler may only touch async-signal-safe
    /// state - `rename(2)` and `unlink(2)` are on the list, an
    /// allocator is not.
    static RESTORE_TO: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());
    static RESTORE_FROM: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());

    /// Arms the handler to restore `target` from `backup` (or remove
    /// `target`, when there is no backup) on a teardown signal.  The
    /// smoke test's `.dev.vars` needs this: it is a real working-tree
    /// file the run mutates, and `Drop` never runs under `_exit`.
    ///
    /// The previous registration, if any, is leaked rather than freed:
    /// the handler may hold the old pointer at the instant of the
    /// swap, and a use-after-free in a signal handler is a worse bug
    /// than a few dozen leaked bytes in a short-lived tool.
    pub fn restore(backup: Option<&std::path::Path>, target: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt as _;
        let raw = |path: &std::path::Path| {
            std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_or(std::ptr::null_mut(), std::ffi::CString::into_raw)
        };
        // Target last: the handler keys on RESTORE_TO, so the source
        // must already be visible when the target appears.
        RESTORE_FROM.store(backup.map_or(std::ptr::null_mut(), raw), Ordering::SeqCst);
        RESTORE_TO.store(raw(target), Ordering::SeqCst);
    }

    /// Disarms [`restore`] once the normal cleanup path has restored
    /// the file itself.
    pub fn restore_done() {
        RESTORE_TO.store(std::ptr::null_mut(), Ordering::SeqCst);
        RESTORE_FROM.store(std::ptr::null_mut(), Ordering::SeqCst);
    }

    /// How long [`stop`] waits for the leaders after SIGTERM, and again
    /// after SIGKILL, in seconds.  Leaders that all exit sooner end the
    /// wait.
    pub const GRACE: libc::time_t = 2;

    /// Stops the process groups that `leaders`, unreaped children of
    /// this process, lead: SIGTERM to every group, then, once every
    /// leader has exited or [`GRACE`] has passed, SIGKILL to every
    /// group - which reaches a member that ignores SIGTERM or outlives
    /// its leader - and the same wait again.  The leaders stay
    /// unreaped, so each pid, and the id of the group it leads, names
    /// no other process however long the waits take.
    ///
    /// Async-signal-safe, as the handler needs: no allocation, no lock,
    /// and only `kill`, `poll`, `clock_gettime`, `waitid`, and reads of
    /// `errno`.  The first three are on POSIX's async-signal-safe list.
    /// `waitid` is not, but the Linux and macOS C libraries wrap it no
    /// differently from `waitpid`, which is, and it is the only wait
    /// that sees an exit without reaping.
    pub fn stop(leaders: &[u32]) {
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            for &pid in leaders {
                // The whole group; the direct pid as the fallback.
                if unsafe { libc::kill(-pid.cast_signed(), signal) } != 0 {
                    unsafe { libc::kill(pid.cast_signed(), signal) };
                }
            }
            let (seconds, nanoseconds) = now();
            let deadline = (seconds + GRACE, nanoseconds);
            while leaders.iter().any(|&pid| !exited(pid)) && now() < deadline {
                unsafe { libc::poll(std::ptr::null_mut(), 0, 10) };
            }
        }
    }

    /// Whether `pid`, a child of this process, has exited, without
    /// reaping it.  One that cannot be waited for (`ECHILD`) counts as
    /// exited, an interrupted look as not yet.
    pub fn exited(pid: u32) -> bool {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let found = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if found != 0 {
            return std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted;
        }
        // With WNOHANG, a zero si_pid means still running.
        (unsafe { info.si_pid() }) != 0
    }

    /// `CLOCK_MONOTONIC` as seconds and nanoseconds, which compare in
    /// order.
    fn now() -> (libc::time_t, libc::c_long) {
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now) };
        (now.tv_sec, now.tv_nsec)
    }

    /// A cancelled gate must not orphan its phases: without this, a
    /// process-directed signal leaves cargo and npm running.  The exit
    /// code is the conventional 128 + signal, or 0 under [`HOOKED`].
    extern "C" fn handle(signal: libc::c_int) {
        // `arm` masks the other teardown signals for the handler's
        // whole run, so this never nests.  A reap and its table
        // removal run with these signals blocked, so every pid read
        // here is an unreaped child's (see the crate's ceilings for
        // other threads).
        let mut leaders = [0; SLOTS];
        let mut count = 0;
        for slot in &GROUPS {
            let pid = slot.load(Ordering::SeqCst);
            if pid != 0 {
                leaders[count] = pid;
                count += 1;
            }
        }
        // Nothing is reaped: the exit below leaves the zombies to init.
        stop(&leaders[..count]);
        // The registered working-tree restore, after the groups are
        // stopped (nothing rewrites the file once they are).  Only
        // rename/unlink - both async-signal-safe.
        let to = RESTORE_TO.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !to.is_null() {
            let from = RESTORE_FROM.swap(std::ptr::null_mut(), Ordering::SeqCst);
            if from.is_null() {
                unsafe { libc::unlink(to) };
            } else {
                unsafe { libc::rename(from, to) };
            }
        }
        let code = if HOOKED.load(Ordering::SeqCst) {
            0
        } else {
            128 + signal
        };
        unsafe { libc::_exit(code) };
    }

    pub fn arm() {
        // `sigaction` with all three teardown signals in `sa_mask`,
        // not `signal()`: the handler must never nest.  A second
        // teardown signal would start another `stop` inside the
        // first; blocked, it stays pending and the running handler's
        // `_exit` makes it moot.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle as *const () as libc::sighandler_t;
        unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            unsafe { libc::sigaddset(&raw mut action.sa_mask, signal) };
        }
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
        }
    }

    /// Runs `action` with the teardown signals blocked on this thread,
    /// so a spawn and its `record`, or a reap and its `forget`, are
    /// atomic against the handler.  Blocked, not lost: a signal arriving
    /// meanwhile stays pending and the handler runs on restore.  A
    /// thread spawned inside inherits the block, so it never runs the
    /// handler.
    ///
    /// `pthread_sigmask`, not `sigprocmask`: macOS's `sigprocmask` sets
    /// every thread's mask, so two threads masking at once could restore
    /// each other's blocked set and leave the process deaf to teardown.
    pub fn masked<T>(action: impl FnOnce() -> T) -> T {
        // Restores on unwind too, so a panicking `action` cannot leave
        // this thread deaf to teardown.
        struct Restore(libc::sigset_t);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    libc::pthread_sigmask(
                        libc::SIG_SETMASK,
                        &raw const self.0,
                        std::ptr::null_mut(),
                    )
                };
            }
        }
        let mut block: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::sigemptyset(&raw mut block) };
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            unsafe { libc::sigaddset(&raw mut block, signal) };
        }
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &raw const block, &raw mut previous) };
        let _restore = Restore(previous);
        action()
    }
}

#[cfg(unix)]
use teardown::{forget, record};

#[cfg(unix)]
pub use teardown::{
    arm as arm_teardown, hooked as teardown_exits_zero, masked, restore as restore_on_teardown,
    restore_done as teardown_restore_done,
};

/// No-ops off Unix, where there is no signal path to restore on.
#[cfg(not(unix))]
pub fn restore_on_teardown(_backup: Option<&std::path::Path>, _target: &std::path::Path) {}

#[cfg(not(unix))]
pub fn teardown_restore_done() {}

#[cfg(not(unix))]
fn record(_pid: u32) {}

#[cfg(not(unix))]
fn forget(_pid: u32) {}

#[cfg(not(unix))]
pub fn masked<T>(action: impl FnOnce() -> T) -> T {
    action()
}

/// No-op off Unix.
#[cfg(not(unix))]
pub fn teardown_exits_zero() {}

/// No-op off Unix, where there are no process groups to tear down.
#[cfg(not(unix))]
pub fn arm_teardown() {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::time::{Duration, Instant};

    /// Fails the test after killing the phase group, so nothing is
    /// left running.  Called only right after a read found the write
    /// end open: a live member keeps the group's id from naming
    /// anyone else's.
    fn fail(group: libc::pid_t, message: &str) -> ! {
        unsafe { libc::kill(-group, libc::SIGKILL) };
        panic!("{message}");
    }

    /// What the holder wrote since the last call: empty at
    /// end-of-file, `None` while the write end is open and silent.
    fn available(reader: &mut std::fs::File) -> Option<Vec<u8>> {
        let mut buf = [0u8; 64];
        match reader.read(&mut buf) {
            Ok(n) => Some(buf[..n].to_vec()),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(err) => panic!("reading the fifo failed: {err}"),
        }
    }

    /// A fifo in `dir`.
    fn fifo(dir: &std::path::Path, name: &str) -> PathBuf {
        use std::os::unix::ffi::OsStrExt as _;
        let fifo = dir.join(name);
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("the fifo path");
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        fifo
    }

    /// The fifo's read end, opened before the launch and without
    /// blocking, so neither side ever waits on the other to open it.
    fn read_end(fifo: &std::path::Path) -> std::fs::File {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(fifo)
            .expect("the fifo's read end")
    }

    /// The phase's group, which a member announces on the fifo as `$$`
    /// (the phase's pid) and a newline once it holds the write end.
    fn announced(reader: &mut std::fs::File) -> libc::pid_t {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut announced = Vec::new();
        // Until a member opens the write end, a read sees end-of-file.
        while !announced.ends_with(b"\n") {
            announced.extend(available(reader).unwrap_or_default());
            assert!(
                Instant::now() < deadline,
                "the phase never started: it announced {announced:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        String::from_utf8_lossy(&announced)
            .trim()
            .parse()
            .expect("the announced group")
    }

    /// What the group wrote to the fifo up to end-of-file, which comes
    /// once every process holding the write end has died.  One still
    /// holding it at `deadline` fails the test.
    fn until_closed(reader: &mut std::fs::File, group: libc::pid_t, deadline: Instant) -> Vec<u8> {
        let mut written = Vec::new();
        loop {
            match available(reader) {
                Some(bytes) if bytes.is_empty() => return written,
                Some(bytes) => written.extend(bytes),
                None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                None => fail(
                    group,
                    &format!("group {group} survived the teardown, having written {written:?}"),
                ),
            }
        }
    }

    /// A gate running one phase: `script` under `sh`, with the fifo
    /// `held` as `$1`.  The path travels as a shell ARGUMENT, never
    /// interpolated into the script: a scratch directory with spaces or
    /// metacharacters in it must not change what the redirects open.
    fn launched(dir: &std::path::Path, script: &str, held: &std::path::Path) -> Gate {
        let mut gate = Gate::new(dir.to_path_buf(), true, Box::new(std::io::sink()), false);
        gate.launch(
            "phase",
            Command::new("sh").arg("-c").arg(script).arg("sh").arg(held),
        )
        .expect("launch the phase");
        gate
    }

    /// Drops `gate` on another thread, so a teardown that hangs fails
    /// the test instead of stalling it, and sends how long the drop
    /// took.
    fn dropped(gate: Gate) -> std::sync::mpsc::Receiver<Duration> {
        let (took, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            drop(gate);
            let _ = took.send(start.elapsed());
        });
        receiver
    }

    fn grace() -> Duration {
        Duration::from_secs(teardown::GRACE.try_into().expect("a positive grace period"))
    }

    /// A child killed and reaped on drop, so a failing test leaves none
    /// of its own children running.
    struct Reaped(Child);

    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A phase whose leader and grandchild both ignore SIGTERM: the
    /// leader `exec`s into `sleep`, which keeps the inherited disposition.
    const IGNORES_SIGTERM: &str =
        r#"exec 3>"$1"; trap '' TERM; (echo $$ >&3; exec sleep 60) & exec sleep 60"#;

    #[test]
    fn masked_unblocks_the_teardown_signals_when_its_action_panics() {
        // On a thread of its own, so a broken restore cannot leave the
        // signals blocked for whatever the test's thread runs next.
        std::thread::spawn(|| {
            let _ = std::panic::catch_unwind(|| masked(|| panic!("the masked action")));
            let mut current: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &raw mut current) };
            assert_eq!(
                unsafe { libc::sigismember(&raw const current, libc::SIGTERM) },
                0
            );
        })
        .join()
        .expect("the mask was restored");
    }

    /// Dropping a gate with a live phase terminates the phase's whole
    /// process group - the grandchild too, which killing the leader
    /// alone never reached.  Death is observed as EOF on a FIFO whose
    /// write end the grandchild holds: a terminated process closes its
    /// descriptors even where an unreaping container init leaves it a
    /// zombie, which a `kill(pid, 0)` liveness probe would misread as
    /// alive.
    #[test]
    fn a_dropped_gate_kills_the_whole_phase_group() {
        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let held = fifo(scratch.path(), "held");
        let blocker = fifo(scratch.path(), "blocker");
        let mut reader = read_end(&held);

        // The grandchild, a backgrounded subshell, announces the
        // phase's group once it runs shell code, then blocks opening a
        // fifo nobody writes: no `exec` is left for the teardown signal
        // to land in.
        let mut gate = Gate::new(
            scratch.path().to_path_buf(),
            true,
            Box::new(std::io::sink()),
            false,
        );
        gate.launch(
            "grandchild holder",
            Command::new("sh")
                .arg("-c")
                .arg(r#"{ echo $$; read line < "$2"; } > "$1" & wait"#)
                .arg("sh")
                .arg(&held)
                .arg(&blocker),
        )
        .expect("launch the phase");
        let group = announced(&mut reader);

        let teardown = dropped(gate);
        let deadline = Instant::now() + Duration::from_secs(10);
        // The group's members at the deadline, reported only once another
        // read finds the write end still open, so that read, not the
        // snapshot, is what the kill follows.
        let mut survivors = None;
        loop {
            match available(&mut reader) {
                Some(bytes) if bytes.is_empty() => break,
                Some(bytes) => fail(
                    group,
                    &format!("the grandchild wrote {bytes:?} after its announcement"),
                ),
                None => match survivors.take() {
                    Some(members) => fail(
                        group,
                        &format!(
                            "the grandchild survived the gate's teardown; group {group} still holds {members:#?}"
                        ),
                    ),
                    None if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    None => {
                        let ps = Command::new("ps")
                            .args(["-A", "-o", "pgid=,pid=,stat=,command="])
                            .output()
                            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                            .unwrap_or_default();
                        survivors = Some(
                            ps.lines()
                                .filter(|line| {
                                    line.split_whitespace().next() == Some(&group.to_string())
                                })
                                .map(str::to_owned)
                                .collect::<Vec<_>>(),
                        );
                    }
                },
            }
        }
        // With the grandchild gone the leader's `wait` returns, so a
        // drop that is still running is stuck on its own.
        teardown
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("the gate's drop never returned");
    }

    /// The teardown's first signal is SIGTERM, which a phase can handle,
    /// and a drop whose phases all exit on it returns without sitting
    /// out the grace period.
    #[test]
    fn a_dropped_gate_lets_a_phase_handle_sigterm() {
        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let held = fifo(scratch.path(), "held");
        let mut reader = read_end(&held);
        // `wait`, unlike a foreground `sleep`, never holds the trap off.
        let gate = launched(
            scratch.path(),
            r#"exec 3>"$1"; trap 'echo term >&3; exit 0' TERM; echo $$ >&3; while :; do sleep 1 & wait; done"#,
            &held,
        );
        let group = announced(&mut reader);

        let took = dropped(gate);
        let written = until_closed(&mut reader, group, Instant::now() + Duration::from_secs(10));
        assert_eq!(written, b"term\n");
        let took = took
            .recv_timeout(Duration::from_secs(10))
            .expect("the gate's drop never returned");
        assert!(took < grace(), "the drop took {took:?}");
    }

    /// A phase ignoring SIGTERM is killed once the grace period has
    /// passed, in bounded time, and no other group is signaled: a
    /// bystander in a group of its own outlives the teardown.
    #[test]
    fn a_dropped_gate_kills_a_phase_ignoring_sigterm_after_the_grace_period() {
        use std::os::unix::process::CommandExt as _;
        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let held = fifo(scratch.path(), "held");
        let mut reader = read_end(&held);
        let mut bystander = Reaped(
            Command::new("sleep")
                .arg("60")
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn the bystander"),
        );
        let gate = launched(scratch.path(), IGNORES_SIGTERM, &held);
        let group = announced(&mut reader);

        let took = dropped(gate);
        until_closed(
            &mut reader,
            group,
            Instant::now() + grace() + Duration::from_secs(10),
        );
        let took = took
            .recv_timeout(Duration::from_secs(10))
            .expect("the gate's drop never returned");
        let untouched = bystander
            .0
            .try_wait()
            .expect("poll the bystander")
            .is_none();
        assert!(
            took >= grace(),
            "killed after {took:?}, inside the grace period"
        );
        assert!(untouched, "the teardown signaled an unrelated group");
    }

    /// A grandchild ignoring SIGTERM is killed once its leader has
    /// exited on it, without waiting out the grace period.
    #[test]
    fn a_dropped_gate_kills_a_grandchild_that_outlives_its_leader() {
        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let held = fifo(scratch.path(), "held");
        let mut reader = read_end(&held);
        let gate = launched(
            scratch.path(),
            r#"exec 3>"$1"; (trap '' TERM; echo $$ >&3; exec sleep 60) & exec sleep 60"#,
            &held,
        );
        let group = announced(&mut reader);

        let took = dropped(gate);
        until_closed(&mut reader, group, Instant::now() + Duration::from_secs(10));
        let took = took
            .recv_timeout(Duration::from_secs(10))
            .expect("the gate's drop never returned");
        assert!(took < grace(), "the drop took {took:?}");
    }

    /// Names the scratch directory of the process that
    /// [`a_teardown_signal_stops_the_phases_and_restores_the_file`] signals.
    const SIGNALED: &str = "XTASK_CI_TEARDOWN_SIGNALED";

    /// The process the test above runs and signals: an armed teardown
    /// with a registered file restore and a phase ignoring SIGTERM.  It
    /// marks itself `armed` once the phase is recorded, then waits a
    /// minute for the signal, so it ends even if the test failed.
    #[test]
    #[ignore = "run and signaled by a_teardown_signal_stops_the_phases_and_restores_the_file"]
    fn signaled_gate() {
        let Some(scratch) = std::env::var_os(SIGNALED) else {
            return;
        };
        let scratch = PathBuf::from(scratch);
        arm_teardown();
        restore_on_teardown(Some(&scratch.join("backup")), &scratch.join("file"));
        let _gate = launched(&scratch, IGNORES_SIGTERM, &scratch.join("held"));
        std::fs::write(scratch.join("armed"), "").expect("mark the gate armed");
        std::thread::sleep(Duration::from_secs(60));
    }

    /// A teardown signal stops every phase group, escalating to SIGKILL,
    /// then restores the registered file and exits 128 + the signal.
    #[test]
    fn a_teardown_signal_stops_the_phases_and_restores_the_file() {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let held = fifo(scratch.path(), "held");
        let mut reader = read_end(&held);
        std::fs::write(scratch.path().join("backup"), "original").expect("write the backup");
        std::fs::write(scratch.path().join("file"), "mutated").expect("write the file");
        let mut signaled = Reaped(
            Command::new(std::env::current_exe().expect("the test binary"))
                .args(["--exact", "tests::signaled_gate", "--ignored"])
                .env(SIGNALED, scratch.path())
                .process_group(0)
                .stdout(Stdio::null())
                .spawn()
                .expect("run the signaled gate"),
        );
        let group = announced(&mut reader);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !scratch.path().join("armed").exists() {
            assert!(
                Instant::now() < deadline,
                "the signaled gate never finished launching"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        unsafe { libc::kill(signaled.0.id().cast_signed(), libc::SIGTERM) };
        until_closed(
            &mut reader,
            group,
            Instant::now() + grace() + Duration::from_secs(10),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = signaled.0.try_wait().expect("poll the signaled gate") {
                break status;
            }
            assert!(Instant::now() < deadline, "the signaled gate never exited");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            status.code(),
            Some(128 + libc::SIGTERM),
            "{status:?} {:?}",
            status.signal()
        );
        assert_eq!(
            std::fs::read_to_string(scratch.path().join("file")).expect("read the file"),
            "original"
        );
        assert!(!scratch.path().join("backup").exists());
    }
}
