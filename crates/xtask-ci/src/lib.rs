//! Local mirror of the CI gate (`rust.yml`, `ci.yml`, `website.yml`).
//! Expensive checks are scoped to the surfaces changed relative to
//! `origin/main`, per AGENTS.md ("run only the checks that match the
//! touched surface").
//!
//! The signal-safe process management underneath the gate -
//! [`spawn_tracked`], [`reap`], [`kill_group`], [`arm_teardown`] and
//! the teardown-time file restore - is also the shared child-process
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
//! Ceilings, where this stops short of the shell it replaces:
//!
//! - the teardown is Unix-only.  Every phase - plus, in hook mode,
//!   the serial steps, as in the shell's hook, where the whole inner
//!   gate was one process group - runs in its own process group and a
//!   `SIGTERM`/`SIGINT`/`SIGHUP` tears the groups down; on a non-Unix
//!   host the children are ordinary processes and a cancelled gate
//!   can orphan them;
//! - a teardown signal arriving while a tracked child is being
//!   spawned stays pending until the spawn returns, because the
//!   spawn-and-record pair runs under a signal mask.  Bash recorded
//!   its jobs at fork time and could cancel a child stuck before
//!   `exec`; here a stalled `exec` defers the cancellation for as
//!   long as it stalls - deferred, not lost;
//! - `nproc` honored `OMP_NUM_THREADS`, which the standard library's
//!   parallelism probe does not, so [`cores::effective`] reads those
//!   variables itself rather than quietly using more of a host than
//!   the operator asked for.

pub mod cores;
pub mod hook;
pub mod scope;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

/// The repository this gate checks.
///
/// Resolved at RUNTIME through git, as the shell resolved it from its
/// own location: the gate checks the tree it is invoked in, which is
/// what makes it correct inside a `git worktree`.  Taking the crate's
/// compile-time manifest directory instead would pin every run to
/// whichever checkout the binary happened to be built in - a copied or
/// worktree-shared binary would then check the wrong tree and report
/// its verdict about someone else's changes.
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
/// goes to.  The log is replayed only on failure, under a final
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
    /// random name with mode 0600 (exactly `mktemp`); `reopen` gives
    /// the reader an INDEPENDENT file description, because it must
    /// not share the writer's cursor - a grandchild still holding
    /// the write end after the reaped child exits would otherwise
    /// have its position moved by the replay's seek, as `cat`'s
    /// separate open never did.  Dropping the `TempPath` unlinks the
    /// file right away, so no path is left for a squatter to attack
    /// and an unlinked log cannot outlive the gate on any exit
    /// path - the shell needed `rm` traps for that.  A FAILED
    /// unlink is ignored, as the shell's `rm -f ... || true`
    /// ignored it; the straggler is an unreadable 0600 file.
    fn log() -> Result<(std::fs::File, std::fs::File)> {
        let file = tempfile::NamedTempFile::new().context("create a scratch log file")?;
        let reader = file.reopen().context("reopen the scratch log file")?;
        let (writer, path) = file.into_parts();
        drop(path);
        Ok((writer, reader))
    }

    /// Everything a finished child wrote to its log, as bytes: one
    /// invalid byte in compiler output must not drop the whole replay
    /// (the shell used `cat`).
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
    /// Tracked for teardown only in hook mode, which is the shell's
    /// exact split: its hook ran the whole inner gate as one killable
    /// process group, but the script itself enabled job control only
    /// for the parallel phases - a serial step stayed in the
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
    /// spare and run in the foreground otherwise.  Written once either
    /// way, as the shell's `launch` was.
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
        // leaves its grandchildren (a `wrangler dev` pair included)
        // running with PPID 1.  Every phase still listed here is one
        // `finish` never reached (it removes each entry before reaping
        // it), so none has been reaped: its zombie still pins the pid
        // and the group kill cannot hit a recycled one.  Every group
        // is signaled BEFORE the first blocking reap, as the teardown
        // handler orders it, so one stuck phase cannot delay the
        // signal to the next; the reaps then clear the teardown table
        // entries.  A child that ignores TERM hangs the reap - the
        // exposure the teardown handler documents and accepts.
        for phase in &mut self.phases {
            kill_group(&mut phase.child);
        }
        for mut phase in self.phases.drain(..) {
            let _ = reap(&mut phase.child);
        }
    }
}

/// Puts a child in its own process group, so a teardown can kill the
/// phase's whole tree - npm's node children included - rather than
/// just the process this crate spawned.
///
/// The child's signal mask is reset before exec.  The spawn happens
/// under [`teardown::masked`], and `exec` preserves the blocked mask:
/// without the reset every child inherits blocked TERM/INT/HUP and is
/// permanently immune to the very teardown the mask protects - found
/// empirically, and invisible to `ps`, which reports the mask as
/// clear.  Bash gave its children a clean mask the same way.
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
/// teardown handler, with the teardown signals blocked across both:
/// bash registered the job at fork time, so its trap never saw a
/// child the kill loop could miss, and neither may ours.
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

/// Signals a tracked child's whole process group - the direct pid as
/// the fallback, as the teardown handler falls back - for a caller
/// that stops and restarts a phase mid-run.  The caller still
/// [`reap`]s the child afterwards; the group table entry is dropped
/// there, not here.
pub fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        if unsafe { libc::kill(-pid.cast_signed(), libc::SIGTERM) } != 0 {
            unsafe { libc::kill(pid.cast_signed(), libc::SIGTERM) };
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// Waits for a tracked child without letting its pid become reusable
/// while the teardown handler could still signal it: first a wait
/// that does NOT reap (`WNOWAIT` - the child stays a zombie, and a
/// zombie's pid cannot be recycled), then the real reap and the
/// table removal together under the signal mask.  This is the
/// guarantee bash's job table gave the shell: reaped jobs leave it,
/// so a recycled pid is not signaled.  The one exception is the
/// fallback below for a failed `waitid`, which accepts the old
/// reap-to-forget window rather than waiting masked for the child's
/// whole runtime.
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
        // Only reachable if `waitid` failed outright; waiting under
        // the mask there would defer cancellation for the child's
        // whole runtime, so take the plain wait and its small window
        // instead.
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
    static GROUPS: [AtomicU32; 8] = [
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ];

    /// Whether a cancelled gate should still exit 0.  The hook adapter
    /// promises that on every path, including this one - a non-zero
    /// exit there reads as "the hook crashed" rather than as a
    /// decision.
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

    /// A cancelled gate must not orphan its phases: without this, a
    /// process-directed signal leaves cargo and npm running.  The exit
    /// code is the conventional 128 + signal, as the shell's traps
    /// used.
    extern "C" fn handle(signal: libc::c_int) {
        // `arm` masks the other teardown signals for the handler's
        // whole run, so this never nests.  The ordering below - kill
        // everything first, reap and clear after - is still the safe
        // shape on its own: a slot leaves the table only once its
        // group is dead AND reaped, never while the pid could still
        // name a live child (bash's kill loop left the job table
        // intact the same way).
        for slot in &GROUPS {
            let pid = slot.load(Ordering::SeqCst);
            if pid != 0 {
                // Negative pid: the whole process group; the direct
                // pid as the fallback, as the shell's
                // `kill -- "-$pid" || kill "$pid"` fell back.
                if unsafe { libc::kill(-pid.cast_signed(), libc::SIGTERM) } != 0 {
                    unsafe { libc::kill(pid.cast_signed(), libc::SIGTERM) };
                }
            }
        }
        // The shell's traps `wait`ed, so a cancelled gate never exits
        // while its children still run (`waitpid` is on the
        // async-signal-safe list).  A child that ignores SIGTERM
        // hangs this, which is the exposure the shell's `wait`
        // accepted too.
        for slot in &GROUPS {
            let pid = slot.load(Ordering::SeqCst);
            if pid != 0 {
                let mut status: libc::c_int = 0;
                unsafe { libc::waitpid(pid.cast_signed(), &raw mut status, 0) };
                // Reaped, so the pid is recyclable from here: drop it
                // from the table at once, so no later pass over the
                // slots can signal whatever the kernel hands it to.
                let _ = slot.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
            }
        }
        // The registered working-tree restore, after the children are
        // gone (nothing rewrites the file once they are).  Only
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
        // 128 + signal is the convention the shell's traps used, but
        // the hook adapter's contract outranks it.
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
        // teardown signal interrupting the first handler between a
        // kill and its wait - or between a reap and its slot clear -
        // would act on a table mid-update; blocked, it stays pending
        // and the running handler's `_exit` makes it moot.
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

    /// Runs `action` with the teardown signals blocked, so a spawn
    /// and its `record`, or a reap and its `forget`, are atomic
    /// against the handler.  Blocked, not lost: a signal arriving
    /// meanwhile stays pending and the handler runs on restore.
    pub fn masked<T>(action: impl FnOnce() -> T) -> T {
        let mut block: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::sigemptyset(&raw mut block) };
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            unsafe { libc::sigaddset(&raw mut block, signal) };
        }
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::sigprocmask(libc::SIG_BLOCK, &raw const block, &raw mut previous) };
        let value = action();
        unsafe { libc::sigprocmask(libc::SIG_SETMASK, &raw const previous, std::ptr::null_mut()) };
        value
    }
}

#[cfg(unix)]
use teardown::{forget, masked, record};

#[cfg(unix)]
pub use teardown::{
    arm as arm_teardown, hooked as teardown_exits_zero, restore as restore_on_teardown,
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
fn masked<T>(action: impl FnOnce() -> T) -> T {
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

    /// Dropping a gate with a live phase terminates the phase's whole
    /// process group - the grandchild too, which killing the leader
    /// alone never reached.  Death is observed as EOF on a FIFO whose
    /// write end the grandchild holds: a terminated process closes its
    /// descriptors even where an unreaping container init leaves it a
    /// zombie, which a `kill(pid, 0)` liveness probe would misread as
    /// alive.
    #[test]
    fn a_dropped_gate_kills_the_whole_phase_group() {
        use std::os::unix::ffi::OsStrExt as _;

        let scratch = assert_fs::TempDir::new().expect("a scratch directory");
        let fifo = scratch.path().join("held");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("the fifo path");
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

        let mut gate = Gate::new(
            scratch.path().to_path_buf(),
            true,
            Box::new(std::io::sink()),
            false,
        );
        // The path travels as a shell ARGUMENT, never interpolated
        // into the script: a scratch directory with spaces or
        // metacharacters in it must not change what the redirect
        // opens.
        gate.launch(
            "grandchild holder",
            Command::new("sh")
                .arg("-c")
                .arg(r#"sleep 300 > "$1" & wait"#)
                .arg("sh")
                .arg(&fifo),
        )
        .expect("launch the phase");
        // Blocks until the grandchild has the write end open: the
        // phase-has-started signal, so the drop below races nothing.
        let mut held = std::fs::File::open(&fifo).expect("the fifo's read end");
        drop(gate);

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut byte = [0u8; 1];
            let _ = sender.send(std::io::Read::read(&mut held, &mut byte));
        });
        let read = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the grandchild survived the gate's teardown");
        assert!(
            matches!(read, Ok(0)),
            "the fifo answered {read:?} instead of end-of-file"
        );
    }
}
