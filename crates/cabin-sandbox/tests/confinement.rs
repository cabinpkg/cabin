//! The operating system enforces the confinement: each test starts this
//! test binary itself confined, as `enforced::probe`, and reads back what the
//! probe could do.  Every denial is paired with a control showing the
//! same operation succeeds when granted, so a denial for an unrelated
//! reason cannot pass.

#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos",
    windows
))]
mod enforced {
    use std::ffi::OsString;
    use std::fs::File;
    use std::path::Path;
    use std::process::{Command, ExitStatus};
    use std::time::Duration;

    use assert_fs::TempDir;
    use cabin_sandbox::{Confinement, SandboxError};

    /// The action the confined copy of this binary performs.
    const PROBE: &str = "SANDBOX_TEST_PROBE";
    const PROBE_ARGS: [&str; 4] = [
        "--exact",
        "enforced::probe",
        "--nocapture",
        "--test-threads=1",
    ];

    /// Does nothing in a normal test run.  Started with [`PROBE`] set, it
    /// performs that action and prints one `<action>: <outcome>` line.
    #[test]
    #[allow(clippy::too_many_lines)] // one independent arm per action
    fn probe() {
        let Some(action) = std::env::var_os(PROBE) else {
            return;
        };
        let action = action.into_string().unwrap();
        let (verb, arg) = action.split_once(':').unwrap_or((&action, ""));
        match verb {
            "read" => report(verb, std::fs::read(arg).map(drop)),
            "list" => report(verb, std::fs::read_dir(arg).map(drop)),
            "write" => report(verb, std::fs::write(arg, "x")),
            "rmdir" => report(verb, std::fs::remove_dir(arg)),
            // Mount `devfs` on `arg`, change that mount, or end it.
            #[cfg(target_os = "macos")]
            "mount" | "remount" | "unmount" => {
                let path = std::ffi::CString::new(arg).unwrap();
                let mount = |flags| {
                    let data = std::ptr::null_mut();
                    // SAFETY: valid C strings; the file system takes no data.
                    unsafe { libc::mount(c"devfs".as_ptr(), path.as_ptr(), flags, data) }
                };
                let rc = match verb {
                    "mount" => mount(0),
                    "remount" => mount(libc::MNT_UPDATE),
                    // SAFETY: valid C string.
                    _ => unsafe { libc::unmount(path.as_ptr(), 0) },
                };
                report(verb, errno(rc));
            }
            "rename" => {
                let (from, to) = arg.split_once(',').unwrap();
                report(verb, std::fs::rename(from, to));
            }
            // Exchange the directory `<dir>` with a symlink made at `<link>`.
            #[cfg(target_os = "macos")]
            "swap" => {
                let (dir, link) = arg.split_once(',').unwrap();
                std::os::unix::fs::symlink("elsewhere", link).unwrap();
                let [dir, link] = [dir, link].map(|path| std::ffi::CString::new(path).unwrap());
                // SAFETY: valid C strings.
                let rc =
                    unsafe { libc::renamex_np(link.as_ptr(), dir.as_ptr(), libc::RENAME_SWAP) };
                report(verb, errno(rc));
            }
            "overwrite" => report(
                verb,
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(arg)
                    .and_then(|mut file| std::io::Write::write_all(&mut file, b"x")),
            ),
            // Make, rename, and remove a directory and a file in `arg`.
            "churn" => {
                let (made, moved) = (Path::new(arg).join("made"), Path::new(arg).join("moved"));
                let churned = std::fs::create_dir(&made)
                    .and_then(|()| std::fs::write(made.join("file"), "x"))
                    .and_then(|()| std::fs::rename(made.join("file"), made.join("renamed")))
                    .and_then(|()| std::fs::rename(&made, &moved))
                    .and_then(|()| std::fs::remove_file(moved.join("renamed")))
                    .and_then(|()| std::fs::remove_dir(&moved));
                report(verb, churned);
            }
            #[cfg(unix)]
            "truncate" => {
                let path = std::ffi::CString::new(arg).unwrap();
                // SAFETY: valid C string.
                report(verb, errno(unsafe { libc::truncate(path.as_ptr(), 0) }));
            }
            // Windows drops a container's packets, which a plain connect
            // takes some 20 seconds to give up on.
            "connect" => report(
                verb,
                std::net::TcpStream::connect_timeout(&arg.parse().unwrap(), Duration::from_secs(2))
                    .map(drop),
            ),
            "udp" => report(
                verb,
                std::net::UdpSocket::bind("127.0.0.1:0")
                    .and_then(|socket| socket.send_to(b"x", arg).map(drop)),
            ),
            #[cfg(target_os = "linux")]
            "socketpair" => {
                let mut pair = [0; 2];
                // SAFETY: `pair` is valid for two descriptors.
                let rc = unsafe {
                    let (family, kind) = arg.split_once(',').unwrap();
                    libc::socketpair(
                        family.parse().unwrap(),
                        kind.parse().unwrap(),
                        0,
                        pair.as_mut_ptr(),
                    )
                };
                report(verb, errno(rc));
            }
            #[cfg(target_os = "linux")]
            "keyctl" => {
                // KEYCTL_GET_KEYRING_ID of KEY_SPEC_USER_KEYRING, creating it.
                // SAFETY: plain system call.
                let id = unsafe { libc::syscall(libc::SYS_keyctl, 0, -4, 1) };
                report(verb, errno(if id < 0 { -1 } else { 0 }));
            }
            // Attach the System V segment `arg`.
            #[cfg(unix)]
            "shm" => {
                // SAFETY: plain system call; the mapping dies with the probe.
                let at = unsafe {
                    libc::shmat(arg.parse().unwrap(), std::ptr::null(), libc::SHM_RDONLY)
                };
                report(verb, errno(if at as isize == -1 { -1 } else { 0 }));
            }
            #[cfg(unix)]
            "posix-shm" => {
                let name = std::ffi::CString::new(arg).unwrap();
                // SAFETY: valid C string; the descriptor is closed below.
                let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
                report(verb, errno(fd.min(0)));
                if fd >= 0 {
                    // SAFETY: closes the descriptor just opened.
                    unsafe { libc::close(fd) };
                }
            }
            #[cfg(unix)]
            "posix-sem" => {
                let name = std::ffi::CString::new(arg).unwrap();
                // SAFETY: valid C string; the semaphore is closed below.
                let semaphore = unsafe { libc::sem_open(name.as_ptr(), 0) };
                let opened = semaphore != libc::SEM_FAILED;
                report(verb, errno(if opened { 0 } else { -1 }));
                if opened {
                    // SAFETY: closes the semaphore just opened.
                    unsafe { libc::sem_close(semaphore) };
                }
            }
            #[cfg(unix)]
            "dgram" => report(
                verb,
                std::os::unix::net::UnixDatagram::pair()
                    .and_then(|(end, _)| end.connect(arg).and_then(|()| end.send(b"x")).map(drop)),
            ),
            #[cfg(target_os = "linux")]
            "mq-unlink" => {
                let name = std::ffi::CString::new(arg).unwrap();
                // SAFETY: valid C string.
                report(verb, errno(unsafe { libc::mq_unlink(name.as_ptr()) }));
            }
            // Signal 0 only checks permission.
            #[cfg(target_os = "linux")]
            // SAFETY: plain system call.
            "kill-all" => report(verb, errno(unsafe { libc::kill(-1, 0) })),
            // Leave `arg` open as descriptor 200, above a lowered soft limit,
            // then report what a confined program sees of it.
            #[cfg(unix)]
            "high-fd" => {
                use std::os::fd::AsRawFd as _;
                let file = File::open(arg).unwrap();
                // SAFETY: plain system calls on valid pointers.
                unsafe {
                    assert_eq!(libc::dup2(file.as_raw_fd(), 200), 200);
                    let mut limit: libc::rlimit = std::mem::zeroed();
                    assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit), 0);
                    limit.rlim_cur = 64;
                    assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit), 0);
                }
                println!(
                    "high-fd: {}",
                    confine("fd:200", &[], &[]).unwrap().outcome("fd")
                );
            }
            #[cfg(unix)]
            "fd" => {
                // SAFETY: only queries the descriptor.
                let open = unsafe { libc::fcntl(arg.parse().unwrap(), libc::F_GETFD) } != -1;
                println!("fd: {}", if open { "open" } else { "closed" });
            }
            "env" => {
                for (key, _) in std::env::vars_os() {
                    println!("env: {}", key.to_string_lossy());
                }
            }
            // Run the probe `arg` in a child process.
            "child" => {
                let status = probe_command(arg).status().unwrap();
                assert!(status.success());
            }
            // Leave the probe `arg` (`late` or `detach`) behind in a child,
            // and exit once it has started.
            #[allow(clippy::zombie_processes)] // the child outlives this probe by design
            "linger" => {
                let dir = Path::new(arg.split_once(':').unwrap().1);
                let mut child = probe_command(arg).spawn().unwrap();
                while !dir.join("started").exists() {
                    assert!(child.try_wait().unwrap().is_none(), "{arg} failed");
                    std::thread::sleep(Duration::from_millis(10));
                }
                println!("linger: {}", child.id());
            }
            // Write `<arg>/started`, then `<arg>/late` a second later.
            "late" => late(Path::new(arg)),
            // Write `<signal>/started`, wait for `<signal>/go`, then read.
            "wait-read" => {
                let (signal, path) = arg.split_once(',').unwrap();
                wait_for_go(Path::new(signal));
                report(verb, std::fs::read(path).map(drop));
            }
            // The same, then make a symlink at `<link>`.
            #[cfg(unix)]
            "wait-symlink" => {
                let (signal, link) = arg.split_once(',').unwrap();
                wait_for_go(Path::new(signal));
                report(verb, std::os::unix::fs::symlink("elsewhere", link));
            }
            // Leave the process group first; report whether that seemed to
            // work.
            #[cfg(unix)]
            "detach" => {
                // SAFETY: plain system calls.
                let left = unsafe { libc::setsid() != -1 && libc::setpgid(0, 0) == 0 };
                println!("detach: {}", if left { "ok" } else { "failed" });
                late(Path::new(arg));
            }
            // Stand in for Cabin: confined, run `late:<arg>` in a child, which
            // the parent-death signal does not reach, and wait.
            "supervise" => {
                // As it is: on Windows `supervisor` started a copy the test
                // removes, which one made here, by a process about to be
                // killed, would not be.
                let exe = std::env::current_exe().unwrap();
                let action = format!("child:late:{arg}");
                confine_as(&exe, &action, &[], &[Path::new(arg)], &[]).unwrap();
            }
            "exit" => std::process::exit(arg.parse().unwrap()),
            // Give the file `<file>` the second name `<link>`, and write to
            // it under that name.
            "link" => {
                let (link, file) = arg.split_once(',').unwrap();
                report(
                    verb,
                    std::fs::hard_link(file, link).and_then(|()| std::fs::write(link, "x")),
                );
            }
            // Leave a file in the temporary directory and print where.
            "temp" => {
                let file = std::env::temp_dir().join("planted");
                std::fs::write(&file, "x").unwrap();
                println!("temp: {}", file.display());
            }
            #[cfg(unix)]
            "session" => {
                // SAFETY: plain system calls.
                let leader = unsafe { libc::getsid(0) == libc::getpid() };
                println!("session: {}", if leader { "leader" } else { "inherited" });
            }
            #[cfg(unix)]
            "signal" => {
                // SAFETY: signal 0 only checks permission.
                report(verb, errno(unsafe { libc::kill(arg.parse().unwrap(), 0) }));
            }
            #[cfg(target_os = "macos")]
            "mach" => println!("mach: {}", mach_lookup(arg)),
            #[cfg(target_os = "macos")]
            "task" => {
                let [control, read] = task_ports(arg.parse().unwrap());
                println!("task-for-pid: {control}");
                println!("task-read-for-pid: {read}");
            }
            // Try to create `<arg>/started` from a nested sandbox.
            #[cfg(unix)]
            "nested" => {
                let out = || File::create("/dev/null").unwrap();
                let exe = std::env::current_exe().unwrap();
                let nested = cabin_sandbox::run(Confinement {
                    args: PROBE_ARGS.map(OsString::from).to_vec(),
                    current_dir: arg.into(),
                    env: vec![(PROBE.into(), format!("late:{arg}").into())],
                    read: vec![exe.parent().unwrap().to_owned()],
                    program: exe,
                    write: vec![arg.into()],
                    hidden: Vec::new(),
                    stdout: out(),
                    stderr: out(),
                });
                let outcome = match nested {
                    Err(SandboxError::Unavailable(_)) => "unavailable".to_owned(),
                    other => format!("{other:?}"),
                };
                println!("nested: {outcome}");
            }
            // Whether `arg` is an open handle to a file holding `token`; the
            // value alone could be another of this process's handles.
            #[cfg(windows)]
            "handle" => {
                use std::os::windows::fs::FileExt as _;
                use std::os::windows::io::{FromRawHandle as _, RawHandle};
                use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
                let handle = arg.parse::<usize>().unwrap() as RawHandle;
                let mut start = [0; 5];
                // SAFETY: only queries the handle and reads through it; the
                // `ManuallyDrop` leaves it open.
                let open = unsafe {
                    GetFileType(handle) == FILE_TYPE_DISK
                        && std::mem::ManuallyDrop::new(File::from_raw_handle(handle))
                            .seek_read(&mut start, 0)
                            .is_ok_and(|read| start[..read] == *b"token")
                };
                println!("handle: {}", if open { "open" } else { "closed" });
            }
            #[cfg(windows)]
            "process" => {
                use windows_sys::Win32::Foundation::CloseHandle;
                use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_VM_READ};
                // SAFETY: plain system calls; the handle is closed at once.
                let opened = unsafe {
                    let process = OpenProcess(PROCESS_VM_READ, 0, arg.parse().unwrap());
                    !process.is_null() && CloseHandle(process) != 0
                };
                report(verb, last_error(opened));
            }
            #[cfg(windows)]
            "pipe" => report(
                verb,
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(arg)
                    .map(drop),
            ),
            #[cfg(windows)]
            "registry" => {
                use windows_sys::Win32::System::Registry::{
                    HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW,
                };
                let mut key = std::ptr::null_mut();
                // SAFETY: a NUL-terminated name and an out-pointer; the key
                // is closed at once.
                let error = unsafe {
                    let name = wide(arg);
                    let error =
                        RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &raw mut key);
                    if error == 0 {
                        RegCloseKey(key);
                    }
                    error
                };
                report(
                    verb,
                    match error {
                        0 => Ok(()),
                        error => Err(std::io::Error::from_raw_os_error(error.cast_signed())),
                    },
                );
            }
            #[cfg(windows)]
            "credential" => {
                use windows_sys::Win32::Security::Credentials::{
                    CRED_TYPE_GENERIC, CredFree, CredReadW,
                };
                let mut credential = std::ptr::null_mut();
                // SAFETY: a NUL-terminated name and an out-pointer.
                let read = unsafe {
                    CredReadW(
                        wide(arg).as_ptr(),
                        CRED_TYPE_GENERIC,
                        0,
                        &raw mut credential,
                    )
                } != 0;
                let result = last_error(read);
                if read {
                    // SAFETY: allocated by `CredReadW`.
                    unsafe { CredFree(credential.cast()) };
                }
                report(verb, result);
            }
            #[cfg(windows)]
            "junction" => {
                let (link, target) = arg.split_once(',').unwrap();
                // The standard streams are inherited: a container's program
                // cannot open the null device on every Windows.
                let made = junction(Path::new(link), Path::new(target)).status();
                let made = made.unwrap().success();
                println!("junction: {}", if made { "ok" } else { "failed" });
            }
            // Whether this process runs in a container, and how many
            // capabilities its token holds.
            #[cfg(windows)]
            "capabilities" => {
                use windows_sys::Win32::Security::{
                    GetTokenInformation, TOKEN_GROUPS, TOKEN_QUERY, TokenCapabilities,
                    TokenIsAppContainer,
                };
                use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
                let mut token = std::ptr::null_mut();
                let (mut contained, mut length) = (0_u32, 0);
                // Aligned for, and far larger than, a list of no capabilities.
                let mut list = [0_u64; 512];
                // SAFETY: out-pointers valid for the sizes given; the token's
                // handle ends with the probe.
                let queried = unsafe {
                    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) != 0
                        && GetTokenInformation(
                            token,
                            TokenIsAppContainer,
                            (&raw mut contained).cast(),
                            4,
                            &raw mut length,
                        ) != 0
                        && GetTokenInformation(
                            token,
                            TokenCapabilities,
                            list.as_mut_ptr().cast(),
                            4096,
                            &raw mut length,
                        ) != 0
                };
                assert!(queried, "{}", std::io::Error::last_os_error());
                // SAFETY: `GetTokenInformation` wrote the list's header.
                let count = unsafe { (*list.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount };
                println!("capabilities: {contained} {count}");
            }
            // What this process's job keeps it from on the desktop.
            #[cfg(windows)]
            "ui" => {
                use windows_sys::Win32::System::JobObjects::{
                    JOBOBJECT_BASIC_UI_RESTRICTIONS, JobObjectBasicUIRestrictions,
                    QueryInformationJobObject,
                };
                let mut limits = JOBOBJECT_BASIC_UI_RESTRICTIONS::default();
                // SAFETY: no handle stands for this process's own job; the
                // structure is the one the class names.
                let queried = unsafe {
                    QueryInformationJobObject(
                        std::ptr::null_mut(),
                        JobObjectBasicUIRestrictions,
                        (&raw mut limits).cast(),
                        u32::try_from(size_of_val(&limits)).unwrap(),
                        std::ptr::null_mut(),
                    )
                } != 0;
                match last_error(queried) {
                    Ok(()) => println!("ui: {:#x}", limits.UIRestrictionsClass),
                    Err(err) => println!("ui: {:?}", err.kind()),
                }
            }
            other => panic!("unknown probe {other}"),
        }
        std::process::exit(0);
    }

    /// Write `<signal>/started`, then wait for `<signal>/go`.
    fn wait_for_go(signal: &Path) {
        std::fs::write(signal.join("started"), "").unwrap();
        while !signal.join("go").exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `started` holds the program's `LOCALAPPDATA`, which on Windows
    /// lies in the directory named after its container.
    fn late(dir: &Path) {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        std::fs::write(dir.join("started"), local).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        std::fs::write(dir.join("late"), "").unwrap();
    }

    /// The status of `task_for_pid` and `task_read_for_pid` for `pid`.
    #[cfg(target_os = "macos")]
    fn task_ports(pid: libc::pid_t) -> [i32; 2] {
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_read_for_pid(task: u32, pid: libc::pid_t, read: *mut u32) -> i32;
        }
        let mut port = 0;
        // SAFETY: valid out-pointers.
        unsafe {
            [
                libc::task_for_pid(mach_task_self_, pid, &raw mut port),
                task_read_for_pid(mach_task_self_, pid, &raw mut port),
            ]
        }
    }

    /// `bootstrap_look_up`'s status for the Mach service `name`.
    #[cfg(target_os = "macos")]
    fn mach_lookup(name: &str) -> i32 {
        unsafe extern "C" {
            static bootstrap_port: u32;
            fn bootstrap_look_up(bp: u32, name: *const libc::c_char, sp: *mut u32) -> i32;
        }
        let name = std::ffi::CString::new(name).unwrap();
        let mut port = 0;
        // SAFETY: valid C string and out-pointer.
        unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &raw mut port) }
    }

    /// A C call's `0` or `-1` result, the latter with its `errno`.
    #[cfg(unix)]
    fn errno(rc: libc::c_int) -> std::io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// A Windows call's success, or the error it left behind.
    #[cfg(windows)]
    fn last_error(succeeded: bool) -> std::io::Result<()> {
        if succeeded {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(windows)]
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// `mklink`, to make the junction `link` to the directory `target`.
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) -> Command {
        use std::os::windows::process::CommandExt as _;
        // Quoted as a whole and in its parts, which keeps a path's space or
        // `&` out of `cmd`'s own syntax.
        let quoted = format!(
            r#"/C "mklink /J "{}" "{}"""#,
            link.display(),
            target.display()
        );
        let mut mklink = Command::new("cmd");
        mklink.raw_arg(quoted);
        mklink
    }

    /// A symlink at `link` to the directory `target`: on Windows a
    /// junction, which takes no privilege to create.
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        assert!(junction(link, target).output().unwrap().status.success());
    }

    /// Whether the process `pid` is gone or was ended by the job, which
    /// ends a process with the code 1, one the probe never exits with.
    #[cfg(windows)]
    fn killed(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        // SAFETY: plain system calls on a handle closed before returning.
        unsafe {
            let access = PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
            let process = OpenProcess(access, 0, pid);
            if process.is_null() {
                // Any other failure says nothing about the process.
                let gone = Some(ERROR_INVALID_PARAMETER.cast_signed());
                assert_eq!(std::io::Error::last_os_error().raw_os_error(), gone);
                return true;
            }
            // A killed process can outlast the run by a moment.  The code
            // of one still running after the wait reads as 259.
            let mut code = 0;
            WaitForSingleObject(process, 5_000);
            GetExitCodeProcess(process, &raw mut code);
            CloseHandle(process);
            eprintln!("PROBE killed: pid={pid} code={code}");
            code == 1
        }
    }

    fn report(action: &str, result: std::io::Result<()>) {
        match result {
            Ok(()) => println!("{action}: ok"),
            Err(err) => println!("{action}: {:?}", err.kind()),
        }
    }

    fn probe_command(action: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(PROBE_ARGS).env(PROBE, action);
        command
    }

    struct Probed {
        status: ExitStatus,
        stdout: String,
    }

    impl Probed {
        /// Every outcome the probe reported for `action`.  The test harness
        /// may print its own text before a report on the same line.
        fn outcomes(&self, action: &str) -> Vec<&str> {
            let marker = format!("{action}: ");
            self.stdout
                .lines()
                .filter_map(|line| line.split_once(&marker).map(|(_, outcome)| outcome))
                .collect()
        }

        fn outcome(&self, action: &str) -> &str {
            match self.outcomes(action)[..] {
                [outcome] => outcome,
                _ => panic!("expected one `{action}` report in {:?}", self.stdout),
            }
        }
    }

    /// Run the probe `action` confined, granting this test binary's
    /// directory plus `read` for reading and `write` for writing.
    fn confine(action: &str, read: &[&Path], write: &[&Path]) -> Result<Probed, SandboxError> {
        confine_hiding(action, read, write, &[])
    }

    fn confine_hiding(
        action: &str,
        read: &[&Path],
        write: &[&Path],
        hidden: &[&Path],
    ) -> Result<Probed, SandboxError> {
        #[cfg(unix)]
        let exe = std::env::current_exe().unwrap();
        #[cfg(windows)]
        let (_dir, exe) = copied_exe();
        confine_as(&exe, action, read, write, hidden)
    }

    /// A copy of this test binary in a directory of its own: a grant on
    /// Windows rewrites the access-control list of all the directory
    /// holds, so the one every test binary shares is not granted.
    #[cfg(windows)]
    fn copied_exe() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let copy = dir.path().join("probe.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
        (dir, copy)
    }

    /// What a confined probe is given: its action, and on Windows the
    /// `SystemRoot` without which no socket can be created.
    fn probe_env(action: &str) -> Vec<(OsString, OsString)> {
        Vec::from([
            (PROBE.into(), action.into()),
            #[cfg(windows)]
            ("SystemRoot".into(), std::env::var_os("SystemRoot").unwrap()),
        ])
    }

    /// [`confine_hiding`] with `exe`, a copy of this test binary.
    fn confine_as(
        exe: &Path,
        action: &str,
        read: &[&Path],
        write: &[&Path],
        hidden: &[&Path],
    ) -> Result<Probed, SandboxError> {
        let exe = exe.to_owned();
        let exe_dir = exe.parent().unwrap().to_owned();
        let logs = TempDir::new().unwrap();
        let (stdout, stderr) = (logs.path().join("stdout"), logs.path().join("stderr"));
        let status = cabin_sandbox::run(Confinement {
            program: exe,
            args: PROBE_ARGS.map(OsString::from).to_vec(),
            current_dir: exe_dir.clone(),
            env: probe_env(action),
            read: std::iter::once(exe_dir)
                .chain(read.iter().map(|p| p.to_path_buf()))
                .collect(),
            write: write.iter().map(|p| p.to_path_buf()).collect(),
            hidden: hidden.iter().map(|p| p.to_path_buf()).collect(),
            stdout: File::create(&stdout).unwrap(),
            stderr: File::create(&stderr).unwrap(),
        })?;
        let stdout = std::fs::read_to_string(stdout).unwrap();
        let stderr = std::fs::read_to_string(stderr).unwrap();
        assert!(
            status.success() || action.starts_with("exit:") || action.starts_with("handle:"),
            "probe `{action}` failed with {status}: {stdout}{stderr}"
        );
        Ok(Probed { status, stdout })
    }

    fn arg(path: &Path) -> String {
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn reads_are_limited_to_the_granted_paths() {
        let forbidden_dir = TempDir::new().unwrap();
        let forbidden = forbidden_dir.path().join("secret");
        std::fs::write(&forbidden, "token").unwrap();
        let read = format!("read:{}", arg(&forbidden));

        assert_eq!(
            confine(&read, &[], &[]).unwrap().outcome("read"),
            "PermissionDenied"
        );
        assert_eq!(
            confine(&read, &[forbidden_dir.path()], &[])
                .unwrap()
                .outcome("read"),
            "ok"
        );
    }

    #[test]
    fn hidden_paths_stay_unreadable_inside_a_grant() {
        let package = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let (visible, git) = (package.path().join("build.c"), package.path().join(".git"));
        std::fs::write(&visible, "int main(void) { return 0; }").unwrap();
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("config"), "token").unwrap();
        std::fs::write(outside.path().join("secret"), "token").unwrap();
        // The package's own symlinks must not widen the grant.
        link_dir(&git, &package.path().join("git"));
        link_dir(outside.path(), &package.path().join("outside"));
        let read = |path: &Path| {
            confine_hiding(
                &format!("read:{}", arg(path)),
                &[package.path()],
                &[],
                &[&git],
            )
            .unwrap()
        };
        assert_eq!(read(&visible).outcome("read"), "ok");
        for denied in [
            git.join("config"),
            package.path().join("git").join("config"),
            package.path().join("outside").join("secret"),
        ] {
            assert_eq!(
                read(&denied).outcome("read"),
                "PermissionDenied",
                "{denied:?}"
            );
        }
    }

    /// A symlink where a directory the program may write was would lead
    /// the next run's grant elsewhere.
    #[test]
    fn a_write_grant_stays_where_it_is() {
        for verb in [
            "rmdir",
            "rename",
            #[cfg(target_os = "macos")]
            "swap",
        ] {
            // Whether the program reported success, and whether the
            // directory is still there.
            let attempt = |confined: bool| {
                let dir = TempDir::new().unwrap();
                let (out, other) = (dir.path().join("out"), dir.path().join("other"));
                std::fs::create_dir(&out).unwrap();
                std::fs::create_dir(&other).unwrap();
                let action = match verb {
                    "rmdir" => format!("rmdir:{}", arg(&out)),
                    _ => format!("{verb}:{},{}", arg(&out), arg(&other.join("new"))),
                };
                let stdout = if confined {
                    confine(&action, &[], &[&out, &other]).unwrap().stdout
                } else {
                    let unconfined = probe_command(&action).output().unwrap();
                    String::from_utf8(unconfined.stdout).unwrap()
                };
                let done = stdout.contains(&format!("{verb}: ok"));
                (done, out.is_dir() && !out.is_symlink())
            };
            assert_eq!(attempt(false), (true, false), "{verb} unconfined");
            assert_eq!(attempt(true), (false, true), "{verb}");
        }
    }

    /// A link the program left in its write grant on an earlier run leads
    /// only where the grants already reach.
    #[test]
    fn a_link_inside_a_grant_does_not_widen_it() {
        let out = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let forbidden = outside.path().join("secret");
        std::fs::write(&forbidden, "token").unwrap();
        link_dir(outside.path(), &out.path().join("link"));
        for action in [
            format!("read:{}", arg(&out.path().join("link").join("secret"))),
            format!("read:{}", arg(&forbidden)),
            format!("write:{}", arg(&out.path().join("link").join("planted"))),
        ] {
            let verb = action.split_once(':').unwrap().0;
            let probed = confine(&action, &[], &[out.path()]).unwrap();
            assert_eq!(probed.outcome(verb), "PermissionDenied", "{action}");
        }
        assert!(!outside.path().join("planted").exists());
    }

    #[test]
    #[cfg(unix)]
    fn a_grant_through_a_symlink_keeps_hidden_paths_hidden() {
        let package = TempDir::new().unwrap();
        let links = TempDir::new().unwrap();
        let git = package.path().join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("config"), "token").unwrap();
        std::fs::write(package.path().join("build.c"), "").unwrap();
        let link = links.path().join("package");
        std::os::unix::fs::symlink(package.path(), &link).unwrap();
        let read = |path: &Path| {
            let action = format!("read:{}", arg(path));
            let probed = confine_hiding(&action, &[&link], &[], &[&git]).unwrap();
            probed.outcome("read").to_owned()
        };
        assert_eq!(read(&package.path().join("build.c")), "ok");
        assert_eq!(read(&git.join("config")), "PermissionDenied");
    }

    /// The outcome of reading `path`, granted `package` but `hidden`, once
    /// `create` has run after the program started.
    fn read_created_later(
        package: &Path,
        hidden: &Path,
        path: &Path,
        create: impl FnOnce(),
    ) -> String {
        let signal = TempDir::new().unwrap();
        let action = format!("wait-read:{},{}", arg(signal.path()), arg(path));
        std::thread::scope(|scope| {
            let probe = scope.spawn(|| {
                confine_hiding(&action, &[package], &[signal.path()], &[hidden]).unwrap()
            });
            while !signal.path().join("started").exists() {
                assert!(!probe.is_finished(), "the probe failed");
                std::thread::sleep(Duration::from_millis(10));
            }
            create();
            std::fs::write(signal.path().join("go"), "").unwrap();
            probe.join().unwrap().outcome("wait-read").to_owned()
        })
    }

    /// Hidden paths, and their parents, need not exist when the program
    /// starts.
    #[test]
    fn a_hidden_path_created_later_stays_hidden() {
        let package = TempDir::new().unwrap();
        let private = package.path().join("private");
        let forbidden = private.join("secret");
        let outcome = read_created_later(package.path(), &forbidden, &forbidden, || {
            std::fs::create_dir(&private).unwrap();
            std::fs::write(&forbidden, "token").unwrap();
        });
        assert_eq!(outcome, "PermissionDenied");
    }

    /// Nor need a hidden symlink's target.
    #[test]
    #[cfg(unix)]
    fn a_dangling_hidden_symlink_keeps_its_target_hidden() {
        let package = TempDir::new().unwrap();
        let git = package.path().join(".git");
        let worktree = package.path().join("meta/worktree");
        std::fs::create_dir(package.path().join("meta")).unwrap();
        std::os::unix::fs::symlink(&worktree, &git).unwrap();
        let config = worktree.join("config");
        let outcome = read_created_later(package.path(), &git, &config, || {
            std::fs::create_dir(&worktree).unwrap();
            std::fs::write(&config, "token").unwrap();
        });
        assert_eq!(outcome, "PermissionDenied");
    }

    /// A package's symlink loop or deep chain ends resolution, not Cabin.
    #[test]
    #[cfg(unix)]
    fn a_hidden_symlink_loop_fails_the_run() {
        let package = TempDir::new().unwrap();
        let (a, b) = (package.path().join("a"), package.path().join("b"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let deep = package.path().join("d");
        std::os::unix::fs::symlink(["d"; 500].join("/"), &deep).unwrap();
        for hidden in [&a, &deep] {
            let result = confine_hiding("exit:0", &[package.path()], &[], &[hidden]);
            assert!(
                matches!(result, Err(SandboxError::Run { .. })),
                "{hidden:?}"
            );
        }
    }

    /// Windows resolves `..` before it looks at the directory.
    #[test]
    #[cfg(unix)]
    fn a_hidden_path_through_a_missing_directory_fails_the_run() {
        let package = TempDir::new().unwrap();
        let hidden = package.path().join("future/../secret");
        let result = confine_hiding("exit:0", &[package.path()], &[], &[&hidden]);
        assert!(matches!(result, Err(SandboxError::Run { .. })));
    }

    /// Nor can Cabin tell where a symlink it may not read leads.
    #[test]
    #[cfg(unix)]
    fn a_hidden_symlink_through_an_unsearchable_directory_fails_the_run() {
        use std::os::unix::fs::PermissionsExt as _;
        let package = TempDir::new().unwrap();
        let (git, locked) = (package.path().join(".git"), package.path().join("locked"));
        std::fs::create_dir(&locked).unwrap();
        std::os::unix::fs::symlink("../secret", locked.join("ref")).unwrap();
        std::os::unix::fs::symlink("locked/ref", &git).unwrap();
        let mode = |mode| std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(mode));
        mode(0o000).unwrap();
        // Root searches through 0o000; only then is there nothing to test.
        if std::fs::read_link(locked.join("ref")).is_ok() {
            mode(0o700).unwrap();
            return;
        }
        let result = confine_hiding("exit:0", &[package.path()], &[], &[&git]);
        mode(0o700).unwrap();
        assert!(matches!(result, Err(SandboxError::Run { .. })));
    }

    #[test]
    #[cfg(unix)]
    fn a_hidden_symlink_hides_its_target() {
        let package = TempDir::new().unwrap();
        let (git, data) = (package.path().join(".git"), package.path().join("git-data"));
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("config"), "token").unwrap();
        std::os::unix::fs::symlink(&data, &git).unwrap();
        for denied in [git.join("config"), data.join("config")] {
            let probed = confine_hiding(
                &format!("read:{}", arg(&denied)),
                &[package.path()],
                &[],
                &[&git],
            )
            .unwrap();
            assert_eq!(probed.outcome("read"), "PermissionDenied", "{denied:?}");
        }
    }

    /// Nor may the program put one where the directory was, once Cabin
    /// or the user has removed it under a process the program left.
    #[cfg(unix)]
    #[test]
    fn a_removed_write_grant_is_not_replaced() {
        // Whether the program reported making the symlink.
        let attempt = |confined: bool| {
            let (dir, signal) = (TempDir::new().unwrap(), TempDir::new().unwrap());
            let out = dir.path().join("out");
            std::fs::create_dir(&out).unwrap();
            let action = format!("wait-symlink:{},{}", arg(signal.path()), arg(&out));
            std::thread::scope(|scope| {
                let probe = scope.spawn(|| {
                    if confined {
                        confine(&action, &[], &[&out, signal.path()])
                            .unwrap()
                            .stdout
                    } else {
                        let unconfined = probe_command(&action).output().unwrap();
                        String::from_utf8(unconfined.stdout).unwrap()
                    }
                });
                while !signal.path().join("started").exists() {
                    assert!(!probe.is_finished(), "the probe failed");
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::fs::remove_dir(&out).unwrap();
                std::fs::write(signal.path().join("go"), "").unwrap();
                probe.join().unwrap().contains("wait-symlink: ok")
            })
        };
        assert!(attempt(false));
        assert!(!attempt(true));
    }

    /// A mount hides what a directory held, and the end of one shows what
    /// its mount point held before: either way paths lead elsewhere.
    #[cfg(target_os = "macos")]
    #[test]
    fn volumes_are_neither_mounted_nor_unmounted() {
        /// Ends the mount at its path, whatever the test came to.
        struct Unmount<'a>(&'a Path);
        impl Drop for Unmount<'_> {
            fn drop(&mut self) {
                let path = std::ffi::CString::new(arg(self.0)).unwrap();
                // SAFETY: valid C string.
                unsafe { libc::unmount(path.as_ptr(), libc::MNT_FORCE) };
            }
        }
        let dir = TempDir::new().unwrap();
        let _unmount = Unmount(dir.path());
        // Each control follows its attempt: the first makes the mount the
        // later attempts are at, and the last ends it.
        let attempts = ["mount", "remount", "unmount"].map(|verb| {
            let action = format!("{verb}:{}", arg(dir.path()));
            let confined = confine(&action, &[], &[dir.path()]).unwrap();
            let unconfined = probe_command(&action).output().unwrap();
            let control = String::from_utf8_lossy(&unconfined.stdout).into_owned();
            (verb, confined.outcome(verb).to_owned(), control)
        });
        for (verb, outcome, control) in attempts {
            assert_eq!(outcome, "PermissionDenied", "{verb}");
            assert!(
                control.contains(&format!("{verb}: ok")),
                "{verb}: {control}"
            );
        }
    }

    #[test]
    fn the_home_directory_is_not_readable() {
        let home = std::env::home_dir().unwrap();
        assert!(std::fs::read_dir(&home).is_ok());
        let probed = confine(&format!("list:{}", arg(&home)), &[], &[]).unwrap();
        assert_eq!(probed.outcome("list"), "PermissionDenied");
    }

    #[test]
    fn writes_are_limited_to_the_granted_directories() {
        let readable = TempDir::new().unwrap();
        let writable = TempDir::new().unwrap();
        let existing = readable.path().join("existing");
        std::fs::write(&existing, "kept").unwrap();
        for action in [
            format!("write:{}", arg(&readable.path().join("planted"))),
            format!("overwrite:{}", arg(&existing)),
            #[cfg(unix)]
            format!("truncate:{}", arg(&existing)),
        ] {
            let verb = action.split_once(':').unwrap().0;
            let denied = confine(&action, &[readable.path()], &[writable.path()]).unwrap();
            assert_eq!(denied.outcome(verb), "PermissionDenied");
        }
        assert!(!readable.path().join("planted").exists());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "kept");

        let allowed = confine(
            &format!("write:{}", arg(&writable.path().join("out"))),
            &[],
            &[writable.path()],
        )
        .unwrap();
        assert_eq!(allowed.outcome("write"), "ok");
        assert!(writable.path().join("out").is_file());
        let churn = format!("churn:{}", arg(writable.path()));
        let churned = confine(&churn, &[], &[writable.path()]).unwrap();
        assert_eq!(churned.outcome("churn"), "ok");

        #[cfg(unix)]
        {
            let null = confine("write:/dev/null", &[], &[]).unwrap();
            assert_eq!(null.outcome("write"), "ok");
        }
    }

    /// A hard link is the file under a second name: one in a write grant
    /// must not make a file the program may only read writable.
    #[test]
    fn a_hard_link_does_not_make_a_readable_file_writable() {
        let readable = TempDir::new().unwrap();
        let writable = TempDir::new().unwrap();
        let link = |name: &str| {
            let file = readable.path().join(name);
            std::fs::write(&file, "kept").unwrap();
            let link = writable.path().join(name);
            (format!("link:{},{}", arg(&link), arg(&file)), file)
        };
        let (action, _) = link("control");
        let unconfined = probe_command(&action).output().unwrap();
        assert!(String::from_utf8_lossy(&unconfined.stdout).contains("link: ok"));

        let (action, file) = link("confined");
        let probed = confine(&action, &[readable.path()], &[writable.path()]).unwrap();
        assert_ne!(probed.outcome("link"), "ok");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "kept");
    }

    /// Cabin, and a later run's grants, follow a junction in a write
    /// grant.  Windows lets a container's program make one only to a
    /// directory it may write.
    #[test]
    #[cfg(windows)]
    fn a_junction_leads_only_where_the_program_may_write() {
        let out = TempDir::new().unwrap();
        let readable = TempDir::new().unwrap();
        let inner = out.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        let junction = |name: &str, target: &Path| {
            let action = format!("junction:{},{}", arg(&out.path().join(name)), arg(target));
            let probed = confine(&action, &[readable.path()], &[out.path()]).unwrap();
            probed.outcome("junction").to_owned()
        };
        assert_eq!(junction("allowed", &inner), "ok");
        assert_eq!(junction("denied", readable.path()), "failed");
    }

    #[test]
    fn the_network_is_unreachable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let tcp = listener.local_addr().unwrap();
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_nonblocking(true).unwrap();
        let udp = receiver.local_addr().unwrap();
        let arrivals: [(String, &dyn Fn() -> bool); 2] = [
            (format!("connect:{tcp}"), &|| listener.accept().is_ok()),
            (format!("udp:{udp}"), &|| receiver.recv(&mut [0]).is_ok()),
        ];
        for (action, arrived) in arrivals {
            let verb = action.split_once(':').unwrap().0;
            let unconfined = probe_command(&action).output().unwrap();
            assert!(
                String::from_utf8_lossy(&unconfined.stdout).contains(&format!("{verb}: ok")),
                "{action} unconfined"
            );
            assert!(arrived(), "{action} unconfined");
            let probed = confine(&action, &[], &[]).unwrap();
            // Windows drops a container's packets without telling it: a
            // connection times out, and a datagram seems sent.
            let refused = match verb {
                _ if cfg!(unix) => "PermissionDenied",
                "connect" => "TimedOut",
                _ => "ok",
            };
            assert_eq!(probed.outcome(verb), refused, "{action}");
            assert!(!arrived(), "{action}: {}", probed.stdout);
        }
    }

    /// A capability is what lets a container reach other machines; the
    /// machine's own addresses stay closed to it even then.
    #[test]
    #[cfg(windows)]
    fn the_container_holds_no_capability() {
        let unconfined = probe_command("capabilities").output().unwrap();
        assert!(String::from_utf8_lossy(&unconfined.stdout).contains("capabilities: 0 0"));
        let probed = confine("capabilities", &[], &[]).unwrap();
        assert_eq!(probed.outcome("capabilities"), "1 0");
    }

    /// `AF_TIPC` pairs are network sockets, and a datagram end (`AF_UNIX`
    /// takes `SOCK_RAW` as one) can send to any socket path.  The kernel
    /// itself would answer `EOPNOTSUPP` for `AF_INET`, so `EPERM` shows the
    /// filter.
    #[cfg(target_os = "linux")]
    #[test]
    fn socket_pairs_are_local_only() {
        let pair = |family: libc::c_int, kind: libc::c_int| {
            let action = format!("socketpair:{family},{kind}");
            confine(&action, &[], &[])
                .unwrap()
                .outcome("socketpair")
                .to_owned()
        };
        assert_eq!(pair(libc::AF_UNIX, libc::SOCK_STREAM), "ok");
        assert_eq!(pair(libc::AF_UNIX, libc::SOCK_SEQPACKET), "ok");
        for (family, kind) in [
            (libc::AF_INET, libc::SOCK_STREAM),
            (libc::AF_UNIX, libc::SOCK_DGRAM),
            (libc::AF_UNIX, libc::SOCK_RAW),
        ] {
            assert_eq!(pair(family, kind), "PermissionDenied", "{family},{kind}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn descriptors_cabin_inherited_are_not_passed_on() {
        use std::os::fd::AsRawFd as _;
        let forbidden_dir = TempDir::new().unwrap();
        let forbidden = forbidden_dir.path().join("secret");
        std::fs::write(&forbidden, "token").unwrap();
        let file = File::open(&forbidden).unwrap();
        // As if Cabin's parent had left it open for Cabin.
        // SAFETY: plain system call on a descriptor this test owns.
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) },
            0
        );
        let action = format!("fd:{}", file.as_raw_fd());
        let unconfined = probe_command(&action).output().unwrap();
        assert!(String::from_utf8_lossy(&unconfined.stdout).contains("fd: open"));
        assert_eq!(confine(&action, &[], &[]).unwrap().outcome("fd"), "closed");

        // A descriptor above the current soft limit, as when a parent
        // lowered the limit after opening it.
        let high = probe_command(&format!("high-fd:{}", arg(&forbidden)))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&high.stdout).contains("high-fd: closed"),
            "{high:?}"
        );
    }

    /// A grant is an entry in the path's access-control list, and in
    /// those of what the path holds, that Cabin removes again.
    #[test]
    #[cfg(windows)]
    fn grants_end_with_the_run() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let lists = || {
            [dir.path(), &file].map(|path| {
                let list = Command::new("icacls").arg(path).output().unwrap().stdout;
                // Windows marks anew which entries came from the directory
                // above.
                String::from_utf8_lossy(&list).replace("(I)", "")
            })
        };
        let before = lists();
        let write = format!("overwrite:{}", arg(&file));
        let probed = confine(&write, &[], &[dir.path()]).unwrap();
        assert_eq!(probed.outcome("overwrite"), "ok");
        assert_eq!(lists(), before);
    }

    #[test]
    #[cfg(windows)]
    fn handles_cabin_inherited_are_not_passed_on() {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        let forbidden_dir = TempDir::new().unwrap();
        let forbidden = forbidden_dir.path().join("secret");
        std::fs::write(&forbidden, "token").unwrap();
        let file = File::open(&forbidden).unwrap();
        // As if Cabin's parent had left it open for Cabin.
        // SAFETY: plain system call on a handle this test owns.
        let inheritable = unsafe {
            SetHandleInformation(
                file.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        };
        assert_ne!(inheritable, 0);
        let action = format!("handle:{}", file.as_raw_handle() as usize);
        let unconfined = probe_command(&action).output().unwrap();
        assert!(String::from_utf8_lossy(&unconfined.stdout).contains("handle: open"));
        // Windows ends a container's program on its use of a handle it
        // does not have.
        let probed = confine(&action, &[], &[]).unwrap();
        assert!(
            probed.status.code() == Some(0xC000_0008_u32.cast_signed())
                || probed.outcome("handle") == "closed",
            "{}",
            probed.stdout
        );
    }

    /// Adding an entry to a path with no list at all, which admits
    /// everyone, would leave it admitting the container alone.
    #[test]
    #[cfg(windows)]
    fn a_path_without_an_access_control_list_fails_the_run() {
        use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };
        let dir = TempDir::new().unwrap();
        // SAFETY: a NUL-terminated name; no list is a list one can set.
        let error = unsafe {
            SetNamedSecurityInfoW(
                wide(&arg(dir.path())).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(error, 0);
        let result = confine("exit:0", &[dir.path()], &[]);
        assert!(matches!(result, Err(SandboxError::Run { .. })));
        assert!(std::fs::read_dir(dir.path()).is_ok());
    }

    /// Windows gives each container a directory of its own, where the
    /// program's `TEMP` lies.
    #[test]
    #[cfg(windows)]
    fn the_containers_own_directory_ends_with_the_run() {
        let probed = confine("temp", &[], &[]).unwrap();
        assert!(!Path::new(probed.outcome("temp")).exists());
    }

    /// The clipboard and the rest of the user's desktop: the job the
    /// program runs in carries every limit on their use.
    #[test]
    #[cfg(windows)]
    fn the_desktop_is_out_of_reach() {
        assert_eq!(confine("ui", &[], &[]).unwrap().outcome("ui"), "0xff");
    }

    /// Another process's memory, a local service's named pipe (an SSH
    /// agent's), and the user's registry hive and stored credentials.
    #[test]
    #[cfg(windows)]
    fn other_processes_and_the_users_secrets_are_out_of_reach() {
        use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Security::Credentials::{
            CRED_PERSIST_SESSION, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredWriteW,
        };
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_UNLIMITED_INSTANCES};
        use windows_sys::Win32::System::Registry::{
            HKEY_CURRENT_USER, RegCloseKey, RegCreateKeyW, RegDeleteKeyW,
        };

        let name = format!("cabin-sandbox-{}", std::process::id());
        let (pipe, key) = (format!(r"\\.\pipe\{name}"), format!(r"Software\{name}"));
        let mut target = wide(&name);
        // One pipe instance for each attempt to open it.
        let _instances = [(); 2].map(|()| {
            // SAFETY: a NUL-terminated name and the default security.
            let instance = unsafe {
                CreateNamedPipeW(
                    wide(&pipe).as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    0,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    0,
                    0,
                    std::ptr::null(),
                )
            };
            assert_ne!(instance, INVALID_HANDLE_VALUE);
            // SAFETY: `CreateNamedPipeW` handed this handle to us.
            unsafe { OwnedHandle::from_raw_handle(instance) }
        });
        let mut blob = *b"token";
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: 5,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_SESSION,
            ..CREDENTIALW::default()
        };
        let mut created = std::ptr::null_mut();
        // SAFETY: NUL-terminated names, a credential whose pointers are
        // valid, and an out-pointer; the key and credential are removed
        // below.
        let stored = unsafe {
            assert_eq!(
                RegCreateKeyW(HKEY_CURRENT_USER, wide(&key).as_ptr(), &raw mut created),
                0
            );
            RegCloseKey(created);
            CredWriteW(&raw const credential, 0) != 0
        };
        let mut actions = vec![
            format!("process:{}", std::process::id()),
            format!("pipe:{pipe}"),
            format!("registry:{key}"),
        ];
        // A session logged on over the network can store no credential.
        if stored {
            actions.push(format!("credential:{name}"));
        }
        // Asserted once the key and credential are gone again.
        let attempts: Vec<_> = actions
            .iter()
            .map(|action| {
                let verb = action.split_once(':').unwrap().0;
                let unconfined = probe_command(action).output().unwrap();
                let control =
                    String::from_utf8_lossy(&unconfined.stdout).contains(&format!("{verb}: ok"));
                let probed = confine(action, &[], &[]).unwrap();
                (control, probed.outcome(verb).to_owned())
            })
            .collect();
        // SAFETY: the key and credential from above.
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&key).as_ptr());
            CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
        for (action, (control, outcome)) in actions.iter().zip(attempts) {
            assert!(control, "{action} unconfined");
            assert_eq!(outcome, "PermissionDenied", "{action}");
        }
    }

    #[test]
    fn child_processes_inherit_the_confinement() {
        let forbidden_dir = TempDir::new().unwrap();
        let forbidden = forbidden_dir.path().join("secret");
        std::fs::write(&forbidden, "token").unwrap();
        let probed = confine(&format!("child:read:{}", arg(&forbidden)), &[], &[]).unwrap();
        assert_eq!(probed.outcome("read"), "PermissionDenied");
    }

    #[test]
    fn the_environment_is_exactly_the_given_one() {
        let probed = confine("env", &[], &[]).unwrap();
        let mut given: Vec<_> = probe_env("env").into_iter().map(|(key, _)| key).collect();
        // Windows points the program at its container's own directory,
        // and on Arm hardware names the processor.
        #[cfg(windows)]
        given.extend(["LOCALAPPDATA", "TEMP", "TMP"].map(OsString::from));
        given.sort();
        let mut seen = probed.outcomes("env");
        #[cfg(windows)]
        seen.retain(|key| *key != "PROCESSOR_ARCHITECTURE");
        seen.sort_unstable();
        assert_eq!(seen, given);
    }

    /// A NUL would end a variable early and start another.
    #[test]
    fn a_nul_in_the_environment_fails_the_run() {
        let result = confine("exit:0\0SMUGGLED=1", &[], &[]);
        assert!(matches!(result, Err(SandboxError::Run { .. })));
    }

    #[test]
    fn processes_left_running_are_killed() {
        let dir = TempDir::new().unwrap();
        let probed = confine(
            &format!("linger:late:{}", arg(dir.path())),
            &[],
            &[dir.path()],
        )
        .unwrap();
        assert!(dir.path().join("started").exists());
        // On Windows the grant ends with the run, so a survivor could not
        // write `late`: look at how the process ended.  `late` there shows a
        // run that waited for the process instead of killing it.
        #[cfg(windows)]
        assert!(killed(probed.outcome("linger").parse().unwrap()));
        #[cfg(unix)]
        {
            drop(probed);
            std::thread::sleep(Duration::from_secs(2));
        }
        assert!(!dir.path().join("late").exists());
    }

    /// Leaving the group appears to work, so callers such as ninja do not
    /// fail, but the process stays in the group Cabin kills.
    #[cfg(target_os = "linux")]
    #[test]
    fn descendants_cannot_leave_the_killed_group() {
        let dir = TempDir::new().unwrap();
        let probed = confine(
            &format!("linger:detach:{}", arg(dir.path())),
            &[],
            &[dir.path()],
        )
        .unwrap();
        assert_eq!(probed.outcome("detach"), "ok");
        std::thread::sleep(Duration::from_secs(2));
        assert!(!dir.path().join("late").exists());
    }

    /// Other processes' shared memory, semaphores, and local sockets.
    #[test]
    #[cfg(unix)]
    fn other_processes_memory_and_sockets_are_out_of_reach() {
        let name = format!("/cabin-{}", std::process::id());
        let c_name = std::ffi::CString::new(name.clone()).unwrap();
        // SAFETY: valid C string; the object is unlinked below.
        let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600) };
        assert!(fd >= 0);
        // SAFETY: valid C string; the semaphore is unlinked below.
        let semaphore = unsafe { libc::sem_open(c_name.as_ptr(), libc::O_CREAT, 0o600_u32, 0_u32) };
        assert_ne!(semaphore, libc::SEM_FAILED);
        // SAFETY: plain system call; the segment is removed below.
        let segment = unsafe { libc::shmget(libc::IPC_PRIVATE, 4096, libc::IPC_CREAT | 0o600) };
        assert!(segment >= 0);
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("socket");
        let _receiver = std::os::unix::net::UnixDatagram::bind(&socket).unwrap();
        for action in [
            format!("shm:{segment}"),
            format!("posix-shm:{name}"),
            format!("posix-sem:{name}"),
            format!("dgram:{}", arg(&socket)),
        ] {
            let verb = action
                .split_once(':')
                .map_or(action.as_str(), |(verb, _)| verb);
            let unconfined = probe_command(&action).output().unwrap();
            assert!(
                String::from_utf8_lossy(&unconfined.stdout).contains(&format!("{verb}: ok")),
                "{action} unconfined"
            );
            assert_eq!(
                confine(&action, &[], &[]).unwrap().outcome(verb),
                "PermissionDenied",
                "{action}"
            );
        }
        // SAFETY: valid C string, and the objects from above.
        unsafe {
            libc::close(fd);
            libc::shm_unlink(c_name.as_ptr());
            libc::sem_close(semaphore);
            libc::sem_unlink(c_name.as_ptr());
            libc::shmctl(segment, libc::IPC_RMID, std::ptr::null_mut());
        }
    }

    /// Kerberos tickets in the user's keyring, and every process of the
    /// user via `kill(-1, …)`.
    #[cfg(target_os = "linux")]
    #[test]
    fn other_processes_state_is_out_of_reach() {
        for action in ["keyctl", "kill-all"] {
            let unconfined = probe_command(action).output().unwrap();
            let control =
                String::from_utf8_lossy(&unconfined.stdout).contains(&format!("{action}: ok"));
            // Docker's default filter already denies the keyrings.
            if action == "keyctl" && !control {
                continue;
            }
            assert!(control, "{action} unconfined");
            assert_eq!(
                confine(action, &[], &[]).unwrap().outcome(action),
                "PermissionDenied",
                "{action}"
            );
        }
    }

    /// Landlock does not cover `mq_unlink`.
    #[cfg(target_os = "linux")]
    #[test]
    fn other_processes_message_queues_are_out_of_reach() {
        let name = format!("/cabin-mq-{}", std::process::id());
        let c_name = std::ffi::CString::new(name.clone()).unwrap();
        let mode: libc::mode_t = 0o600;
        // SAFETY: valid C string and default attributes; closed below.
        let queue = unsafe {
            libc::mq_open(
                c_name.as_ptr(),
                libc::O_CREAT | libc::O_RDWR,
                mode,
                std::ptr::null_mut::<libc::mq_attr>(),
            )
        };
        assert_ne!(queue, -1, "{}", std::io::Error::last_os_error());
        let action = format!("mq-unlink:{name}");
        assert_eq!(
            confine(&action, &[], &[]).unwrap().outcome("mq-unlink"),
            "PermissionDenied"
        );
        // The control unlinks the queue.
        let unconfined = probe_command(&action).output().unwrap();
        assert!(String::from_utf8_lossy(&unconfined.stdout).contains("mq-unlink: ok"));
        // SAFETY: the descriptor from above.
        unsafe { libc::mq_close(queue) };
    }

    /// Stand in for Cabin: `exe`, this test binary, running a confined
    /// program that has written `<dir>/started` and will write
    /// `<dir>/late`.
    fn supervisor(exe: &Path, dir: &Path) -> std::process::Child {
        let mut parent = Command::new(exe)
            .args(PROBE_ARGS)
            .env(PROBE, format!("supervise:{}", arg(dir)))
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        while !dir.join("started").exists() {
            assert!(
                parent.try_wait().unwrap().is_none(),
                "the supervisor failed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        parent
    }

    /// However Cabin exits, its handle to the job closes.
    #[test]
    #[cfg(windows)]
    fn killing_the_parent_kills_the_confined_program() {
        use windows_sys::Win32::Security::Isolation::DeleteAppContainerProfile;
        let dir = TempDir::new().unwrap();
        let (_exe_dir, exe) = copied_exe();
        let mut parent = supervisor(&exe, dir.path());
        parent.kill().unwrap();
        parent.wait().unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(!dir.path().join("late").exists());
        // The killed supervisor left its run's container registered.
        let local = std::fs::read_to_string(dir.path().join("started")).unwrap();
        let container = Path::new(&local).parent().unwrap();
        let name = wide(container.file_name().unwrap().to_str().unwrap());
        // SAFETY: a NUL-terminated name.
        unsafe { DeleteAppContainerProfile(name.as_ptr()) };
        assert!(!container.exists());
    }

    /// The confined program leads its own process group, so the
    /// terminal's Ctrl-C reaches only Cabin; Cabin must pass it on.
    #[test]
    #[cfg(unix)]
    fn interrupting_the_parent_kills_the_confined_program() {
        let dir = TempDir::new().unwrap();
        let mut parent = supervisor(&std::env::current_exe().unwrap(), dir.path());
        let pid = libc::pid_t::try_from(parent.id()).unwrap();
        // SAFETY: plain system call on our own child.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        let status = parent.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGINT)
        );
        std::thread::sleep(Duration::from_secs(2));
        assert!(!dir.path().join("late").exists());
    }

    /// Without a controlling terminal of its own the program cannot read
    /// the user's terminal or push input into it.
    #[test]
    #[cfg(unix)]
    fn the_program_leads_a_new_session() {
        assert_eq!(
            confine("session", &[], &[]).unwrap().outcome("session"),
            "leader"
        );
    }

    /// Seatbelt matches paths, so each place user data lives is listed.
    #[cfg(target_os = "macos")]
    #[test]
    fn shared_temporary_files_are_not_readable() {
        for dir in ["/private/tmp", "/private/var/tmp"] {
            let shared = TempDir::new_in(dir).unwrap();
            let forbidden = shared.path().join("secret");
            std::fs::write(&forbidden, "token").unwrap();
            let probed = confine(&format!("read:{}", arg(&forbidden)), &[], &[]).unwrap();
            assert_eq!(probed.outcome("read"), "PermissionDenied", "{dir}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn processes_outside_the_sandbox_cannot_be_signaled() {
        let me = std::process::id().to_string();
        let probed = confine(&format!("signal:{me}"), &[], &[]).unwrap();
        assert_eq!(probed.outcome("signal"), "PermissionDenied");
    }

    /// Any program may sign itself to debug, and then read the memory of
    /// any process that allows debugging, through its task port.
    #[cfg(target_os = "macos")]
    #[test]
    fn task_ports_outside_the_sandbox_are_unreachable() {
        const ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.cs.debugger</key><true/>
<key>com.apple.security.get-task-allow</key><true/>
</dict></plist>
"#;
        let dir = TempDir::new().unwrap();
        let (exe, entitlements) = (dir.path().join("probe"), dir.path().join("entitlements"));
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        std::fs::write(&entitlements, ENTITLEMENTS).unwrap();
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--entitlements"])
            .args([&entitlements, &exe])
            .status()
            .unwrap();
        assert!(signed.success());
        // The target, signed the same way, waits for `go`.
        let signal = TempDir::new().unwrap();
        let mut target = Command::new(&exe)
            .args(PROBE_ARGS)
            .env(PROBE, format!("wait-read:{},/dev/null", arg(signal.path())))
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        while !signal.path().join("started").exists() {
            assert!(target.try_wait().unwrap().is_none(), "the target failed");
            std::thread::sleep(Duration::from_millis(10));
        }
        let action = format!("task:{}", target.id());
        let unconfined = Command::new(&exe)
            .args(PROBE_ARGS)
            .env(PROBE, &action)
            .output()
            .unwrap();
        let confined = confine_as(&exe, &action, &[], &[], &[]).unwrap();
        std::fs::write(signal.path().join("go"), "").unwrap();
        target.wait().unwrap();
        for call in ["task-for-pid", "task-read-for-pid"] {
            let control = format!("{call}: 0");
            assert!(String::from_utf8_lossy(&unconfined.stdout).contains(&control));
            assert_ne!(confined.outcome(call), "0", "{call}");
        }
    }

    /// A daemon would fetch for the program: the DNS resolver here.
    #[cfg(target_os = "macos")]
    #[test]
    fn mach_services_beyond_the_basics_are_unreachable() {
        const RESOLVER: &str = "com.apple.dnssd.service";
        assert_eq!(mach_lookup(RESOLVER), 0);
        let probed = confine(&format!("mach:{RESOLVER}"), &[], &[]).unwrap();
        assert_ne!(probed.outcome("mach"), "0");
    }

    #[test]
    fn the_exit_status_is_the_programs() {
        let probed = confine("exit:3", &[], &[]).unwrap();
        assert_eq!(probed.status.code(), Some(3));
    }

    /// Seatbelt cannot apply a profile inside a sandboxed process, and on
    /// Linux the outer filter would keep the inner run from killing what
    /// its program leaves: a nested run must not start the program.
    #[test]
    #[cfg(unix)]
    fn a_nested_run_does_not_start_the_program() {
        let dir = TempDir::new().unwrap();
        let probed = confine(&format!("nested:{}", arg(dir.path())), &[], &[dir.path()]).unwrap();
        let outcome = probed.outcome("nested");
        #[cfg(target_os = "macos")]
        assert_eq!(outcome, "unavailable");
        assert!(!outcome.starts_with("Ok"), "{outcome}");
        assert!(!dir.path().join("started").exists());
    }
}

#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos",
    windows
)))]
#[test]
fn confinement_is_unavailable() {
    let logs = assert_fs::TempDir::new().unwrap();
    let log = |name| std::fs::File::create(logs.path().join(name)).unwrap();
    let exe = std::env::current_exe().unwrap();
    let result = cabin_sandbox::run(cabin_sandbox::Confinement {
        current_dir: exe.parent().unwrap().to_owned(),
        program: exe,
        args: Vec::new(),
        env: Vec::new(),
        read: Vec::new(),
        write: Vec::new(),
        hidden: Vec::new(),
        stdout: log("stdout"),
        stderr: log("stderr"),
    });
    assert!(matches!(
        result,
        Err(cabin_sandbox::SandboxError::Unavailable(_))
    ));
}
