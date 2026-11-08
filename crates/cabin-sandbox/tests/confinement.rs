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
    target_os = "macos"
))]
mod enforced {
    use std::ffi::OsString;
    use std::fs::File;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitStatus};
    use std::time::Duration;

    use assert_fs::TempDir;
    use cabin_sandbox::{Confinement, SandboxError};

    /// The action the confined copy of this binary performs.
    const PROBE: &str = "CABIN_SANDBOX_TEST_PROBE";
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
            "overwrite" => report(
                verb,
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(arg)
                    .and_then(|mut file| std::io::Write::write_all(&mut file, b"x")),
            ),
            "truncate" => {
                let path = std::ffi::CString::new(arg).unwrap();
                // SAFETY: valid C string.
                report(verb, errno(unsafe { libc::truncate(path.as_ptr(), 0) }));
            }
            "connect" => report(verb, std::net::TcpStream::connect(arg).map(drop)),
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
            "shm" => {
                // SAFETY: plain system call; the mapping dies with the probe.
                let at = unsafe {
                    libc::shmat(arg.parse().unwrap(), std::ptr::null(), libc::SHM_RDONLY)
                };
                report(verb, errno(if at as isize == -1 { -1 } else { 0 }));
            }
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
            // Stand in for Cabin, with this process's `HOME`.
            #[cfg(target_os = "linux")]
            "home-read" => {
                let probed = confine(&format!("read:{arg}"), &[], &[]).unwrap();
                println!("home-read: {}", probed.outcome("read"));
            }
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
            }
            // Write `<arg>/started`, then `<arg>/late` a second later.
            "late" => late(Path::new(arg)),
            // Write `<signal>/started`, wait for `<signal>/go`, then read.
            "wait-read" => {
                let (signal, path) = arg.split_once(',').unwrap();
                std::fs::write(Path::new(signal).join("started"), "").unwrap();
                while !Path::new(signal).join("go").exists() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                report(verb, std::fs::read(path).map(drop));
            }
            // Leave the process group first; report whether that seemed to
            // work.
            "detach" => {
                // SAFETY: plain system calls.
                let left = unsafe { libc::setsid() != -1 && libc::setpgid(0, 0) == 0 };
                println!("detach: {}", if left { "ok" } else { "failed" });
                late(Path::new(arg));
            }
            // Stand in for Cabin: confined, run `late:<arg>` in a child, which
            // the parent-death signal does not reach, and wait.
            "supervise" => {
                confine(&format!("child:late:{arg}"), &[], &[Path::new(arg)]).unwrap();
            }
            "exit" => std::process::exit(arg.parse().unwrap()),
            "session" => {
                // SAFETY: plain system calls.
                let leader = unsafe { libc::getsid(0) == libc::getpid() };
                println!("session: {}", if leader { "leader" } else { "inherited" });
            }
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
            "nested" => {
                let out = || File::create("/dev/null").unwrap();
                let exe = std::env::current_exe().unwrap();
                let nested = cabin_sandbox::run(Confinement {
                    args: PROBE_ARGS.map(OsString::from).to_vec(),
                    current_dir: PathBuf::from(arg),
                    env: vec![(PROBE.into(), format!("late:{arg}").into())],
                    read: vec![exe.parent().unwrap().to_owned()],
                    program: exe,
                    write: vec![PathBuf::from(arg)],
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
            other => panic!("unknown probe {other}"),
        }
        std::process::exit(0);
    }

    fn late(dir: &Path) {
        std::fs::write(dir.join("started"), "").unwrap();
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
    fn errno(rc: libc::c_int) -> std::io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
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
        confine_as(
            &std::env::current_exe().unwrap(),
            action,
            read,
            write,
            hidden,
        )
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
            env: vec![(PROBE.into(), action.into())],
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
            status.success() || action.starts_with("exit:"),
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
        std::os::unix::fs::symlink(&git, package.path().join("git")).unwrap();
        std::os::unix::fs::symlink(outside.path(), package.path().join("outside")).unwrap();
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
            package.path().join("git/config"),
            package.path().join("outside/secret"),
        ] {
            assert_eq!(
                read(&denied).outcome("read"),
                "PermissionDenied",
                "{denied:?}"
            );
        }
    }

    #[test]
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

    #[test]
    fn a_hidden_path_through_a_missing_directory_fails_the_run() {
        let package = TempDir::new().unwrap();
        let hidden = package.path().join("future/../secret");
        let result = confine_hiding("exit:0", &[package.path()], &[], &[&hidden]);
        assert!(matches!(result, Err(SandboxError::Run { .. })));
    }

    /// Nor can Cabin tell where a symlink it may not read leads.
    #[test]
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

    /// The system directories are granted, but not a home inside one.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_home_under_a_system_directory_stays_unreadable() {
        // Any readable file (not a symlink, which could lead elsewhere) one
        // directory down `/usr/share`, if there is one.
        let Some(file) = std::fs::read_dir("/usr/share")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|dir| std::fs::read_dir(dir.path()).ok())
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .find(|path| File::open(path).is_ok())
        else {
            return;
        };
        let read = |home: &Path| {
            let action = format!("home-read:{}", arg(&file));
            let output = probe_command(&action).env("HOME", home).output().unwrap();
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        assert!(read(Path::new("/nonexistent")).contains("home-read: ok"));
        let denied = read(file.parent().unwrap());
        assert!(denied.contains("home-read: PermissionDenied"), "{denied}");
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

        let null = confine("write:/dev/null", &[], &[]).unwrap();
        assert_eq!(null.outcome("write"), "ok");
    }

    #[test]
    fn the_network_is_unreachable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp = listener.local_addr().unwrap().to_string();
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp = receiver.local_addr().unwrap().to_string();
        for action in [format!("connect:{tcp}"), format!("udp:{udp}")] {
            let verb = action.split_once(':').unwrap().0;
            let mut unconfined = probe_command(&action).output().unwrap();
            unconfined.stdout.extend(unconfined.stderr);
            assert!(
                String::from_utf8_lossy(&unconfined.stdout).contains(&format!("{verb}: ok")),
                "{action} unconfined"
            );
            let probed = confine(&action, &[], &[]).unwrap();
            assert_eq!(probed.outcome(verb), "PermissionDenied", "{action}");
        }
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
        assert_eq!(probed.outcomes("env"), [PROBE]);
    }

    #[test]
    fn processes_left_running_are_killed() {
        let dir = TempDir::new().unwrap();
        confine(
            &format!("linger:late:{}", arg(dir.path())),
            &[],
            &[dir.path()],
        )
        .unwrap();
        assert!(dir.path().join("started").exists());
        std::thread::sleep(Duration::from_secs(2));
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

    /// The confined program leads its own process group, so the
    /// terminal's Ctrl-C reaches only Cabin; Cabin must pass it on.
    #[test]
    fn interrupting_the_parent_kills_the_confined_program() {
        let dir = TempDir::new().unwrap();
        let mut parent = probe_command(&format!("supervise:{}", arg(dir.path())))
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        while !dir.path().join("started").exists() {
            assert!(
                parent.try_wait().unwrap().is_none(),
                "the supervisor failed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
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
    target_os = "macos"
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
