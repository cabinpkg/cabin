//! Windows: run the program in an `AppContainer` with no capabilities,
//! inside a job object.
//!
//! An `AppContainer` process opens an object only when both its user and
//! its container may.  Every container may read the system directories
//! and Program Files; none may reach the user's profile, registry hive,
//! or credentials, another process, or a named pipe (an SSH agent's)
//! that does not admit containers.  With no capabilities it has no
//! network, loopback included.  Each run uses a container of its own,
//! named at random: its SID is granted the read and write paths, through
//! an access-control entry their contents inherit, for that run only, so
//! no other run's program reaches them.  Should Cabin be killed before it
//! removes them, the entries stay behind for a container no process will
//! run in again.
//!
//! The job kills the program and everything it started once the program
//! exits, or as soon as Cabin's handle to the job closes, which Cabin's
//! exit does however it exits.  The program gets a console of its own,
//! without a window, and inherits only the three standard handles.

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

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, LocalFree, WAIT_OBJECT_0,
};
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
    DELETE, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::{Confinement, SandboxError};

const READ: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
const WRITE: u32 = READ | FILE_GENERIC_WRITE | DELETE | FILE_DELETE_CHILD;

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
    let failed = |source| SandboxError::Run {
        program: program.clone(),
        source,
    };
    let hidden = crate::resolve_hidden(&hidden).map_err(failed)?;
    let container = Container::new().map_err(|err| {
        SandboxError::Unavailable(format!("cannot register an AppContainer: {err}"))
    })?;
    let mut grants = Grants {
        sid: container.sid,
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
        set_entry(&path, self.sid, GRANT_ACCESS, access, inheritance).map_err(|err| {
            let path = path.display();
            io::Error::new(err.kind(), format!("cannot grant access to {path}: {err}"))
        })?;
        self.paths.push(path);
        Ok(())
    }
}

impl Drop for Grants {
    fn drop(&mut self) {
        for path in &self.paths {
            // An entry left behind names a container no process runs in.
            let _ = set_entry(path, self.sid, REVOKE_ACCESS, 0, NO_INHERITANCE);
        }
    }
}

/// Apply `mode` for `sid` to `path`'s access-control list, which Windows
/// then passes on to what the directory holds.
fn set_entry(
    path: &Path,
    sid: PSID,
    mode: ACCESS_MODE,
    access: u32,
    inheritance: ACE_FLAGS,
) -> io::Result<()> {
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
    // SAFETY: one valid entry, the list read above, and an out-pointer.
    let error = unsafe { SetEntriesInAclW(1, &raw const entry, acl, &raw mut updated) };
    // SAFETY: allocated by `GetNamedSecurityInfoW`.
    unsafe { LocalFree(descriptor) };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error.cast_signed()));
    }
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
    /// Start the program suspended, put it in a job, resume it, and wait.
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
        // Declared after the values it points to, so dropped before them.
        let attributes = Attributes::new(2)?;
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
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_NO_WINDOW,
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
        let (child, thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(process.hProcess),
                OwnedHandle::from_raw_handle(process.hThread),
            )
        };
        let job = match job_for(&child) {
            Ok(job) => job,
            Err(err) => {
                // SAFETY: a process handle we own; it has not run yet.
                unsafe { TerminateProcess(child.as_raw_handle(), 1) };
                return Err(err);
            }
        };
        // On an error below, dropping `job` kills the program.
        // SAFETY: a thread handle we own.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0;
        // SAFETY: a process handle we own and an out-pointer.
        let exited = unsafe {
            WaitForSingleObject(child.as_raw_handle(), INFINITE) == WAIT_OBJECT_0
                && GetExitCodeProcess(child.as_raw_handle(), &raw mut code) != 0
        };
        if !exited {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a job handle we own.
        unsafe { TerminateJobObject(job.as_raw_handle(), 1) };
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

/// A job that kills `child`'s whole tree once its handle closes, which
/// no process in it can leave, and that keeps it off the user's desktop
/// and clipboard.
fn job_for(child: &OwnedHandle) -> io::Result<OwnedHandle> {
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
    // SAFETY: handles we own.
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
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

/// `KEY=VALUE` strings sorted by key, ignoring case, as Windows keeps
/// them.
fn environment(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut env: Vec<_> = env.iter().collect();
    env.sort_by_cached_key(|(key, _)| key.to_ascii_uppercase());
    let mut block: Vec<u16> = env
        .iter()
        .flat_map(|(key, value)| {
            key.encode_wide()
                .chain(iter::once(u16::from(b'=')))
                .chain(value.encode_wide())
                .chain(iter::once(0))
        })
        .collect();
    if block.is_empty() {
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
    fn the_environment_block_is_sorted_and_terminated() {
        let env = [("b", "2"), ("A", "1")].map(|(k, v)| (OsString::from(k), OsString::from(v)));
        assert_eq!(parsed(&environment(&env)), "A=1\0b=2\0");
        assert_eq!(environment(&[]), [0, 0]);
    }
}
