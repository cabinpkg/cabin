//! Windows: run the program in an `AppContainer` with no capabilities,
//! inside a job object.
//!
//! An `AppContainer` process opens an object only when both its user and
//! its container may.  Every container may read the system directories
//! and Program Files; none may reach the user's profile, registry hive,
//! or credentials, another process, Cabin's console, or a named pipe (an
//! SSH agent's) that does not admit containers.  With no capabilities it
//! has no network, loopback included.  A less-privileged container (one
//! outside `ALL APPLICATION PACKAGES`) is not used: the Microsoft
//! compiler does not run in one.
//!
//! Windows starts a program only in a container registered for the
//! user, so each run registers one under a fresh random name and removes
//! it afterwards, along with the directory Windows gives it and points
//! the program's `LOCALAPPDATA`, `TEMP`, and `TMP` at.  The container's
//! SID is granted the read and write paths, through an access-control
//! entry their contents inherit, for that run only, so no other run's
//! program reaches them.  Should Cabin be killed first, the container and
//! the entries stay behind, for no process to run in again.
//!
//! The job holds the program from its start, and kills it and everything
//! it started once the program exits, or as soon as Cabin's handle to the
//! job closes, which Cabin's exit does however it exits.  The program
//! inherits only the three standard handles.
//!
//! A killed process keeps the files it had open until Windows has closed
//! its handles, which is when its own handle is signaled, and the job
//! stops counting it before that.  So the job reports each process it
//! takes in to a completion port, Cabin opens them as the program runs,
//! and a run ends by waiting on each one not yet gone.  A report can be
//! lost, and a process can deny Cabin its handle: a run that cannot
//! account for every process the job counted fails.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::iter;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::os::windows::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_ACL, ERROR_INVALID_PARAMETER,
    INVALID_HANDLE_VALUE, LocalFree, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::Security::Authorization::{
    ACCESS_MODE, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, REVOKE_ACCESS,
    SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile,
};
use windows_sys::Win32::Security::{
    ACE_FLAGS, DACL_SECURITY_INFORMATION, FreeSid, NO_INHERITANCE, PSID, SECURITY_CAPABILITIES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS,
    JobObjectAssociateCompletionPortInformation, JobObjectBasicAccountingInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::SystemServices::JOB_OBJECT_MSG_NEW_PROCESS;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::{Confinement, SandboxError};

const LOCAL: &str = "LOCALAPPDATA";
const READ: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
/// Without `DELETE`: the program removes and renames what a granted
/// directory holds, not the directory itself.
const WRITE: u32 = READ | FILE_GENERIC_WRITE | FILE_DELETE_CHILD;
/// Milliseconds between two looks at what a running program's job has
/// reported.  No guarantee rests on the value: a process gone before
/// its report is read needs no waiting for.
const REPORT_INTERVAL: u32 = 50;

thread_local! {
    /// Probe only: skip the final waits.
    pub(crate) static PROBE_NO_WAIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Probe only: whether `process` is signaled, and whether it is in `job`.
fn probe_state(process: &OwnedHandle, job: &OwnedHandle) -> (bool, i32) {
    let mut member = -1;
    // SAFETY: handles we own and an out-pointer.
    unsafe {
        let over = WaitForSingleObject(process.as_raw_handle(), 0) == WAIT_OBJECT_0;
        IsProcessInJob(
            process.as_raw_handle(),
            job.as_raw_handle(),
            &raw mut member,
        );
        (over, member)
    }
}

/// Probe only: run as a user without the debug privilege would.
fn probe_disable_debug_privilege() {
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, SE_DEBUG_NAME, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES,
    };
    use windows_sys::Win32::System::Threading::OpenProcessToken;
    let mut token = null_mut();
    let mut privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        ..TOKEN_PRIVILEGES::default()
    };
    // SAFETY: plain system calls on a token closed before returning.
    unsafe {
        let opened = OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &raw mut token);
        let named = LookupPrivilegeValueW(
            null(),
            SE_DEBUG_NAME,
            &raw mut privileges.Privileges[0].Luid,
        );
        let adjusted =
            AdjustTokenPrivileges(token, 0, &raw const privileges, 0, null_mut(), null_mut());
        if opened == 0 || named == 0 || adjusted == 0 {
            eprintln!("probe-privilege failed {opened} {named} {adjusted}");
        }
        windows_sys::Win32::Foundation::CloseHandle(token);
    }
}

pub(crate) fn run(confinement: Confinement) -> Result<ExitStatus, SandboxError> {
    probe_disable_debug_privilege();
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
    let failed = |source| SandboxError::Run {
        program: program.clone(),
        source,
    };
    let strings = [program.as_os_str(), current_dir.as_os_str()]
        .into_iter()
        .chain(args.iter().map(OsString::as_os_str))
        .chain(env.iter().flat_map(|(key, value)| [&**key, &**value]));
    if strings.flat_map(OsStr::encode_wide).any(|unit| unit == 0) {
        return Err(failed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a NUL in the program, an argument, the working directory, or the environment",
        )));
    }
    let hidden = crate::resolve_hidden(&hidden).map_err(failed)?;
    // Windows starts nothing in a container without this variable, from
    // which it derives the three it sets for the program.
    let Some(local) = std::env::var_os(LOCAL) else {
        let reason = format!("{LOCAL} is not set");
        return Err(SandboxError::Unavailable(reason));
    };
    let lock = Path::new(&local).join("cabin-sandbox.lock");
    let env: Vec<_> = env
        .into_iter()
        .filter(|(key, _)| !key.eq_ignore_ascii_case(LOCAL))
        .chain([(LOCAL.into(), local)])
        .collect();
    let container = Container::new().map_err(|err| {
        SandboxError::Unavailable(format!("cannot register an AppContainer: {err}"))
    })?;
    let mut grants = Grants {
        sid: container.sid,
        lock,
        paths: Vec::new(),
    };
    let read = crate::without_hidden(canonical(&read), &hidden);
    for (paths, access) in [(read, READ), (canonical(&write), WRITE)] {
        for path in paths {
            grants.add(path, access).map_err(failed)?;
        }
    }
    let launched = Launch {
        sid: container.sid,
        program: &program,
        args: &args,
        current_dir: &current_dir,
        env: &env,
        stdout: &stdout,
        stderr: &stderr,
    };
    launched.run().map_err(failed)
}

/// Missing paths grant nothing, as on Linux.
fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect()
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(iter::once(0)).collect()
}

/// An `AppContainer` registered under a fresh random name, and removed
/// on drop.
struct Container {
    name: Vec<u16>,
    sid: PSID,
}

impl Container {
    fn new() -> io::Result<Self> {
        let mut bytes = [0_u8; 16];
        // SAFETY: the buffer is valid for its length.
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                bytes.as_mut_ptr(),
                16,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status < 0 {
            return Err(io::Error::other(format!(
                "BCryptGenRandom failed: {status:#x}"
            )));
        }
        let name: String = iter::once("cabin.".to_owned())
            .chain(bytes.iter().map(|byte| format!("{byte:02x}")))
            .collect();
        let name = wide(OsStr::new(&name));
        let mut sid = null_mut();
        // SAFETY: NUL-terminated strings and an out-pointer; the SID is
        // freed on drop.
        let result = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                null(),
                0,
                &raw mut sid,
            )
        };
        if result < 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        Ok(Self { name, sid })
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        // SAFETY: the SID `CreateAppContainerProfile` allocated, and the
        // NUL-terminated name it registered.
        unsafe {
            FreeSid(self.sid);
            DeleteAppContainerProfile(self.name.as_ptr());
        }
    }
}

/// The access-control entries granted to a container's SID, removed on
/// drop.
struct Grants {
    sid: PSID,
    lock: PathBuf,
    paths: Vec<PathBuf>,
}

impl Grants {
    /// Grant `access` to `path` and, for a directory, to what it holds.
    fn add(&mut self, path: PathBuf, access: u32) -> io::Result<()> {
        let inheritance = if path.is_dir() {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        };
        let granted = set_entry(
            &self.lock,
            &path,
            self.sid,
            GRANT_ACCESS,
            access,
            inheritance,
        )
        .map_err(|err| {
            let path = path.display();
            io::Error::new(err.kind(), format!("cannot grant access to {path}: {err}"))
        });
        // Revoked even if it failed: Windows may have reached part of what
        // the directory holds.
        self.paths.push(path);
        granted
    }
}

impl Drop for Grants {
    fn drop(&mut self) {
        for path in &self.paths {
            // An entry left behind names a container no process runs in.
            let _ = set_entry(&self.lock, path, self.sid, REVOKE_ACCESS, 0, NO_INHERITANCE);
        }
    }
}

/// Apply `mode` for `sid` to `path`'s access-control list, which Windows
/// then passes on to what the directory holds.  A list beneath `path`
/// comes out as its directory passes down, whatever it was before: one
/// narrower than that, or absent, is not kept.
fn set_entry(
    lock: &Path,
    path: &Path,
    sid: PSID,
    mode: ACCESS_MODE,
    access: u32,
    inheritance: ACE_FLAGS,
) -> io::Result<()> {
    // The list is read, then written whole: a run doing the same in
    // between would lose its entry, or drop this one.
    let held = File::create(lock).and_then(|file| file.lock().map(|()| file));
    let _held = held.map_err(|err| {
        let lock = lock.display();
        io::Error::new(err.kind(), format!("cannot lock {lock}: {err}"))
    })?;
    let name = wide(path.as_os_str());
    let mut acl = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: a NUL-terminated name and out-pointers; `acl` points into
    // `descriptor`, freed below once `SetEntriesInAclW` has copied it.
    let error = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &raw mut acl,
            null_mut(),
            &raw mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error.cast_signed()));
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            ptstrName: sid.cast(),
            ..TRUSTEE_W::default()
        },
    };
    let mut updated = null_mut();
    // No list at all admits everyone; one made for the entry would admit
    // nobody else.
    let error = if acl.is_null() {
        ERROR_INVALID_ACL
    } else {
        // SAFETY: one valid entry, the list read above, and an out-pointer.
        unsafe { SetEntriesInAclW(1, &raw const entry, acl, &raw mut updated) }
    };
    // SAFETY: allocated by `GetNamedSecurityInfoW`.
    unsafe { LocalFree(descriptor) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error.cast_signed()));
    }
    // Windows now rebuilds every list beneath `path` from what its
    // directory passes down, and one that was absent stays rebuilt once
    // the entry is removed.  Refusing those as the absent list above is
    // refused would take reading every list beneath each grant first.
    // SAFETY: a NUL-terminated name and the list built above, freed after.
    let error = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            updated,
            null(),
        )
    };
    // SAFETY: allocated by `SetEntriesInAclW`.
    unsafe { LocalFree(updated.cast()) };
    if error == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error.cast_signed()))
    }
}

struct Launch<'a> {
    sid: PSID,
    program: &'a Path,
    args: &'a [OsString],
    current_dir: &'a Path,
    env: &'a [(OsString, OsString)],
    stdout: &'a File,
    stderr: &'a File,
}

impl Launch<'_> {
    /// Start the program in a job, wait for it, and end the job.
    fn run(&self) -> io::Result<ExitStatus> {
        let stdin = File::open("NUL")?;
        let inherited = [
            inheritable(&stdin)?,
            inheritable(self.stdout)?,
            inheritable(self.stderr)?,
        ];
        let handles = inherited
            .each_ref()
            .map(std::os::windows::io::AsRawHandle::as_raw_handle);
        let capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: self.sid,
            ..SECURITY_CAPABILITIES::default()
        };
        let mut job = Job::new()?;
        let jobs = [job.handle.as_raw_handle()];
        // Declared after the values it points to, so dropped before them.
        let attributes = Attributes::new(4)?;
        // Probe only: strict handle checks forced off (2) or on (1).
        let strict: u64 = match std::env::var("CABIN_SANDBOX_PROBE_STRICT").as_deref() {
            Ok("off") => 2 << 24,
            Ok("on") => 1 << 24,
            _ => 0,
        };
        if strict != 0 {
            attributes.set(
                windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY,
                (&raw const strict).cast(),
                size_of_val(&strict),
            )?;
        }
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            (&raw const capabilities).cast(),
            size_of_val(&capabilities),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            handles.as_ptr().cast(),
            size_of_val(&handles),
        )?;
        // In the job from the start: were Cabin to put it there afterwards
        // and die first, the program would be left outside any job.
        attributes.set(
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            jobs.as_ptr().cast(),
            size_of_val(&jobs),
        )?;
        let mut info = STARTUPINFOEXW::default();
        info.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>()).unwrap();
        info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        info.StartupInfo.hStdInput = handles[0];
        info.StartupInfo.hStdOutput = handles[1];
        info.StartupInfo.hStdError = handles[2];
        info.lpAttributeList = attributes.list;
        let mut command_line = command_line(self.program, self.args);
        let environment = environment(self.env);
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: NUL-terminated strings, an environment block, and the
        // startup information, whose attribute list and the values it
        // points to outlive the call.
        let created = unsafe {
            CreateProcessW(
                wide(self.program.as_os_str()).as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                wide(self.current_dir.as_os_str()).as_ptr(),
                &raw const info.StartupInfo,
                &raw mut process,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        drop(inherited);
        // SAFETY: `CreateProcessW` handed these handles to us.
        let (child, _thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(process.hProcess),
                OwnedHandle::from_raw_handle(process.hThread),
            )
        };
        // On an error below, dropping `job` kills the program.
        loop {
            // SAFETY: a process handle we own.
            let waited = unsafe { WaitForSingleObject(child.as_raw_handle(), REPORT_INTERVAL) };
            if waited == WAIT_FAILED {
                return Err(io::Error::last_os_error());
            }
            job.open_reported()?;
            if waited == WAIT_OBJECT_0 {
                break;
            }
        }
        let mut code = 0;
        // SAFETY: a process handle we own and an out-pointer.
        if unsafe { GetExitCodeProcess(child.as_raw_handle(), &raw mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        job.end()?;
        Ok(ExitStatus::from_raw(code))
    }
}

/// An inheritable copy of `file`'s handle.
fn inheritable(file: &File) -> io::Result<OwnedHandle> {
    let mut handle = null_mut();
    // SAFETY: duplicates a handle we hold, within this process.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            GetCurrentProcess(),
            &raw mut handle,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `DuplicateHandle` handed this handle to us.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// A job, the port it reports each process it takes in to, and the
/// processes it reported that were not seen to be gone.
struct Job {
    handle: OwnedHandle,
    port: OwnedHandle,
    reported: u32,
    members: Vec<OwnedHandle>,
}

impl Job {
    /// A job that kills all in it once its handle closes, which no
    /// process in it can leave, that keeps them off the user's desktop
    /// and clipboard, and where a crash ends the process instead of
    /// waiting on an error box.
    fn new() -> io::Result<Self> {
        // SAFETY: no attributes, no name.
        let job = unsafe { CreateJobObjectW(null(), null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateJobObjectW` handed this handle to us.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        set_job_information(&job, JobObjectExtendedLimitInformation, &limits)?;
        let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_EXITWINDOWS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        };
        set_job_information(&job, JobObjectBasicUIRestrictions, &restrictions)?;
        // SAFETY: a new port, tied to no file.
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1) };
        if port.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateIoCompletionPort` handed this handle to us.
        let port = unsafe { OwnedHandle::from_raw_handle(port) };
        // Before the job holds a process, so that none goes unreported.
        let reports = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
            CompletionKey: null_mut(),
            CompletionPort: port.as_raw_handle(),
        };
        set_job_information(&job, JobObjectAssociateCompletionPortInformation, &reports)?;
        Ok(Self {
            handle: job,
            port,
            reported: 0,
            members: Vec::new(),
        })
    }

    /// Open each process the job has reported since the last call, and
    /// close those that are gone by now.
    fn open_reported(&mut self) -> io::Result<()> {
        loop {
            let (mut message, mut key, mut id) = (0, 0, null_mut());
            // SAFETY: a port we own and out-pointers.
            let dequeued = unsafe {
                GetQueuedCompletionStatus(
                    self.port.as_raw_handle(),
                    &raw mut message,
                    &raw mut key,
                    &raw mut id,
                    0,
                )
            };
            if dequeued == 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(WAIT_TIMEOUT.cast_signed()) {
                    break;
                }
                return Err(err);
            }
            if message == JOB_OBJECT_MSG_NEW_PROCESS {
                self.reported += 1;
                // The job sends the process's identifier as the pointer.
                self.open(u32::try_from(id.addr()).expect("process ids fit u32"))?;
            }
        }
        self.members.retain(|process| {
            // SAFETY: a process handle we own.
            unsafe { WaitForSingleObject(process.as_raw_handle(), 0) != WAIT_OBJECT_0 }
        });
        Ok(())
    }

    /// Keep a handle to the process `id` the job reported, unless that
    /// process is gone.
    fn open(&mut self, id: u32) -> io::Result<()> {
        let access = PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: plain system call.
        let process = unsafe { OpenProcess(access, 0, id) };
        if process.is_null() {
            let err = io::Error::last_os_error();
            // An identifier lasts as long as its process: this one is gone.
            if err.raw_os_error() == Some(ERROR_INVALID_PARAMETER.cast_signed()) {
                eprintln!("probe-open {id} gone");
                return Ok(());
            }
            eprintln!("probe-open {id} error {err}");
            // A process Cabin cannot open, it cannot wait for.
            return Err(io::Error::new(
                err.kind(),
                format!("cannot open its process {id}: {err}"),
            ));
        }
        // SAFETY: `OpenProcess` handed this handle to us.
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        let mut member = 0;
        // SAFETY: handles we own and an out-pointer.
        let asked = unsafe {
            IsProcessInJob(
                process.as_raw_handle(),
                self.handle.as_raw_handle(),
                &raw mut member,
            )
        };
        if asked == 0 {
            return Err(io::Error::last_os_error());
        }
        // One outside the job took the identifier over once the process
        // reported was gone.
        eprintln!("probe-open {id} member {member}");
        if member != 0 {
            self.members.push(process);
        }
        Ok(())
    }

    /// Kill what is left in the job, then wait until every process it
    /// ever held is gone, and with it the handles that process had open.
    fn end(mut self) -> io::Result<()> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        loop {
            // Each turn: this ends the processes in the job at that moment,
            // and one whose start was under way joins it after them.
            // SAFETY: a job handle we own.
            if unsafe { TerminateJobObject(self.handle.as_raw_handle(), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a job handle we own and the structure the class names.
            let queried = unsafe {
                QueryInformationJobObject(
                    self.handle.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast(),
                    u32::try_from(size_of_val(&accounting)).unwrap(),
                    null_mut(),
                )
            };
            if queried == 0 {
                return Err(io::Error::last_os_error());
            }
            if accounting.ActiveProcesses == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // No process is left to start another: the job has taken in, and
        // reported, its last.
        self.open_reported()?;
        let dying: Vec<_> = self
            .members
            .iter()
            .map(|process| probe_state(process, &self.handle))
            .collect();
        eprintln!(
            "probe-end reported {} total {} held {} (signaled, in job) before the waits: {dying:?}",
            self.reported,
            accounting.TotalProcesses,
            self.members.len()
        );
        if PROBE_NO_WAIT.get() {
            return Ok(());
        }
        if self.reported != accounting.TotalProcesses {
            return Err(io::Error::other(format!(
                "Windows reported {} of its {} processes",
                self.reported, accounting.TotalProcesses
            )));
        }
        for process in &self.members {
            // Signaled once Windows has closed all the process had open;
            // the job stops counting it sooner.
            // SAFETY: a process handle we own.
            if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
        }
        let dead: Vec<_> = self
            .members
            .iter()
            .map(|process| probe_state(process, &self.handle))
            .collect();
        eprintln!("probe-end after the waits: {dead:?}");
        Ok(())
    }
}

/// `information` must be the structure `class` names.
fn set_job_information<T>(
    job: &OwnedHandle,
    class: JOBOBJECTINFOCLASS,
    information: &T,
) -> io::Result<()> {
    // SAFETY: a job handle we own and the structure `class` names.
    let set = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            class,
            (&raw const *information).cast(),
            u32::try_from(size_of::<T>()).unwrap(),
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A process and thread attribute list, deleted on drop.
struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _buffer: Vec<usize>,
}

impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: a size query, which fails by design.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &raw mut size) };
        let mut buffer = vec![0_usize; size.div_ceil(size_of::<usize>())];
        let list = buffer.as_mut_ptr().cast();
        // SAFETY: `buffer` holds `size` bytes, aligned for the list.
        if unsafe { InitializeProcThreadAttributeList(list, count, 0, &raw mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            list,
            _buffer: buffer,
        })
    }

    /// Set `attribute` to the `size` bytes at `value`, which must outlive
    /// every use of the list.
    fn set(&self, attribute: u32, value: *const c_void, size: usize) -> io::Result<()> {
        // SAFETY: an initialized list; `value` is valid for `size` bytes.
        let set = unsafe {
            UpdateProcThreadAttribute(
                self.list,
                0,
                attribute as usize,
                value,
                size,
                null_mut(),
                null(),
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized in `new`.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

/// The command line that the Microsoft C runtime parses back into
/// `program` and `args`.
fn command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let (quote, backslash) = (u16::from(b'"'), u16::from(b'\\'));
    let mut line = Vec::new();
    for arg in iter::once(program.as_os_str()).chain(args.iter().map(OsString::as_os_str)) {
        if !line.is_empty() {
            line.push(u16::from(b' '));
        }
        let arg: Vec<u16> = arg.encode_wide().collect();
        let special = *b" \t\n\x0b\"";
        let special = special.map(u16::from);
        if !arg.is_empty() && !arg.iter().any(|unit| special.contains(unit)) {
            line.extend(arg);
            continue;
        }
        // Backslashes are literal but before a quote, where each pair
        // stands for one.
        line.push(quote);
        let mut backslashes = 0;
        for unit in arg {
            if unit == backslash {
                backslashes += 1;
            } else {
                if unit == quote {
                    line.extend(iter::repeat_n(backslash, backslashes + 1));
                }
                backslashes = 0;
            }
            line.push(unit);
        }
        line.extend(iter::repeat_n(backslash, backslashes));
        line.push(quote);
    }
    line.push(0);
    line
}

/// `KEY=VALUE` strings sorted by key as Windows keeps them: by code
/// unit, ignoring case.  `env` is never empty.
fn environment(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut env: Vec<_> = env.iter().map(|(key, value)| (wide(key), value)).collect();
    env.sort_by(|(a, _), (b, _)| {
        // SAFETY: two NUL-terminated strings.
        let order = unsafe { CompareStringOrdinal(a.as_ptr(), -1, b.as_ptr(), -1, 1) };
        order.cmp(&CSTR_EQUAL)
    });
    let mut block = Vec::new();
    for (mut key, value) in env {
        key.pop();
        block.extend(key);
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &[u16]) -> String {
        String::from_utf16(&line[..line.len() - 1]).unwrap()
    }

    /// Windows' own parser reads back what `command_line` quoted.
    #[test]
    fn arguments_survive_the_windows_parser() {
        use std::os::windows::ffi::OsStringExt as _;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

        let program = PathBuf::from(r"C:\Program Files\tool.exe");
        let args = [
            r"plain",
            "",
            "with space",
            r#"a"b"#,
            r"tail\",
            r#"x\"y"#,
            r"dir\ x\",
        ]
        .map(OsString::from);
        let line = command_line(&program, &args);
        let mut count = 0;
        // SAFETY: a NUL-terminated line; the array is freed below.
        let array = unsafe { CommandLineToArgvW(line.as_ptr(), &raw mut count) };
        assert!(!array.is_null());
        let parsed: Vec<OsString> = (0..usize::try_from(count).unwrap())
            .map(|i| {
                // SAFETY: `count` NUL-terminated strings.
                unsafe {
                    let arg = *array.add(i);
                    let mut len = 0;
                    while *arg.add(len) != 0 {
                        len += 1;
                    }
                    OsString::from_wide(std::slice::from_raw_parts(arg, len))
                }
            })
            .collect();
        // SAFETY: allocated by `CommandLineToArgvW`.
        unsafe { LocalFree(array.cast()) };
        let expected: Vec<OsString> = iter::once(program.into_os_string()).chain(args).collect();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn entries_set_at_once_all_land() {
        use std::sync::Barrier;
        use windows_sys::Win32::Security::ACL;
        use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;

        const RUNS: u16 = 8;
        let dir = assert_fs::TempDir::new().unwrap();
        let lock = dir.path().join("lock");
        let granted = dir.path().join("granted");
        std::fs::create_dir(&granted).unwrap();
        let entries = || {
            let name = wide(granted.as_os_str());
            let mut acl: *mut ACL = null_mut();
            let mut descriptor = null_mut();
            // SAFETY: a NUL-terminated name and out-pointers; `acl` points
            // into `descriptor`, freed once the count is read.
            unsafe {
                let error = GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &raw mut acl,
                    null_mut(),
                    &raw mut descriptor,
                );
                assert_eq!(error, 0);
                let count = acl.as_ref().expect("a file has a list").AceCount;
                LocalFree(descriptor);
                count
            }
        };
        let set_all = |mode| {
            let barrier = Barrier::new(RUNS.into());
            std::thread::scope(|scope| {
                for run in 0..RUNS {
                    let (barrier, lock, granted) = (&barrier, &lock, &granted);
                    scope.spawn(move || {
                        let name = wide(OsStr::new(&format!("cabin.test.{run}")));
                        let mut sid = null_mut();
                        // SAFETY: a NUL-terminated name and an out-pointer;
                        // the SID is freed below.
                        let result = unsafe {
                            DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &raw mut sid)
                        };
                        assert!(result >= 0);
                        barrier.wait();
                        set_entry(lock, granted, sid, mode, READ, NO_INHERITANCE).unwrap();
                        // SAFETY: allocated by the call above.
                        unsafe { FreeSid(sid) };
                    });
                }
            });
        };
        let before = entries();
        for _ in 0..20 {
            set_all(GRANT_ACCESS);
            assert_eq!(entries(), before + RUNS);
            set_all(REVOKE_ACCESS);
            assert_eq!(entries(), before);
        }
    }

    #[test]
    fn the_environment_block_is_sorted_and_terminated() {
        // `ä` sorts as `Ä`, before `Æ`.
        let env = [("b", "2"), ("Æ", "4"), ("A", "1"), ("ä", "3")]
            .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        assert_eq!(parsed(&environment(&env)), "A=1\0b=2\0ä=3\0Æ=4\0");
    }
}
