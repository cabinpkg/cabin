//! Compile, link, and run probes: a small program built the way the
//! build compiles and links, whose outcome answers one question about
//! the toolchain (is this header there, does this sysroot link).
//!
//! The answer tells a capability the toolchain lacks ([`Answer::No`])
//! from a fact no probe can establish here ([`Answer::Unknown`]: a
//! program that must run for a target the build machine cannot run),
//! and both from a tool that could not be invoked at all
//! ([`ProbeError`]).  Commands come from `cabin-driver`'s lowering, so a
//! probe passes the target, sysroot, and flags exactly as the build
//! does.  Nothing is cached: every probe runs the current toolchain.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use cabin_core::{LanguageStandard, SourceLanguage, Sysroot};
use cabin_driver::{
    BuildAction, CompileAction, CompileArguments, CompileMode, Dialect, LinkAction,
};
use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use crate::detect::{ProcessRunner, RunError, RunOutput, run_command_with_timeout};

/// The compiler a probe runs, set up as the build runs it.
pub struct ProbeToolchain<'a> {
    pub dialect: Dialect,
    /// Compiler driver for the probe's language; it links too.
    pub compiler: &'a Utf8Path,
    /// The `--target=` triple the build passes.
    pub target: Option<&'a str>,
    pub sysroot: Option<&'a Sysroot>,
    /// Environment the build runs the compiler in (the MSVC overlay).
    pub env: &'a [(OsString, OsString)],
    /// Absolute directory the build runs its commands in, so relative
    /// paths in the flags mean what they mean there.  The probe builds
    /// in a fresh directory beneath it.
    pub dir: &'a Utf8Path,
    pub execution: Execution<'a>,
}

/// How a probe program runs, when a check needs it to.
pub enum Execution<'a> {
    /// Directly on the build machine.
    Native,
    /// Under this `[runner]` command.
    Runner(&'a [String]),
    /// Not at all: the build machine cannot run the target's programs.
    Unavailable,
}

/// The question a probe answers.
#[derive(Debug)]
pub enum Check {
    /// `#include <name>` compiles.
    Header(String),
    /// `name` is a complete type after the includes.
    Type { name: String, includes: Vec<String> },
    /// `name` is a macro after the includes, or a function or variable
    /// they declare and the link resolves.  Built like the
    /// `check_symbol_exists` template, with its limits: GCC rejects a
    /// function in C under `-pedantic-errors`, and an overloaded C++
    /// function is ambiguous.
    Symbol { name: String, includes: Vec<String> },
    /// The source compiles.
    Compiles(String),
    /// The source compiles and links into an executable.
    Links(String),
    /// The source's program builds and exits with status 0.
    Runs(String),
}

impl Check {
    fn source(&self) -> String {
        match self {
            Check::Header(header) => {
                format!("#include <{header}>\nint main(void) {{ return 0; }}\n")
            }
            // The typedef refuses an expression; an array of it refuses an
            // incomplete type, `void` (whose `sizeof` GNU compilers accept),
            // and a function type.  MSVC's C compiler gives that `sizeof`
            // 0 instead, which the negative bound refuses.
            Check::Type { name, includes } => format!(
                "{}typedef {name} cabin_probe_type;\ntypedef char cabin_probe_size[sizeof(cabin_probe_type[1]) ? 1 : -1];\nint main(void) {{ return 0; }}\n",
                include_lines(includes)
            ),
            // Taking the address makes the linker resolve a function or
            // variable; indexing by `argc` keeps it from folding away.  Only
            // an identifier can be a macro (`std::printf` cannot).
            Check::Symbol { name, includes } => {
                let reference = format!("  return ((int *)(&{name}))[argc];\n");
                let body = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    format!(
                        "#ifndef {name}\n{reference}#else\n  (void)argc;\n  return 0;\n#endif\n"
                    )
                } else {
                    reference
                };
                format!(
                    "{}int main(int argc, char **argv) {{\n  (void)argv;\n{body}}}\n",
                    include_lines(includes)
                )
            }
            Check::Compiles(source) | Check::Links(source) | Check::Runs(source) => source.clone(),
        }
    }

    fn last_step(&self) -> Step {
        match self {
            Check::Header(_) | Check::Type { .. } | Check::Compiles(_) => Step::Compile,
            Check::Symbol { .. } | Check::Links(_) => Step::Link,
            Check::Runs(_) => Step::Run,
        }
    }
}

fn include_lines(includes: &[String]) -> String {
    includes
        .iter()
        .flat_map(|header| ["#include <", header, ">\n"])
        .collect()
}

/// One probe: a check, in a language, with the build's flags.
pub struct ProbeRequest {
    /// Language and standard the probe compiles as.  On the MSVC
    /// dialect it must be one `cl` can spell, as the planner validates
    /// for every build compile.
    pub standard: LanguageStandard,
    /// Compile in the standard's GNU mode (`-std=gnu…`).
    pub gnu_extensions: bool,
    pub check: Check,
    pub arguments: CompileArguments,
    /// Raw link flags, as a target's `ldflags`.
    pub link_flags: Vec<String>,
    /// System libraries to link, as bare names.
    pub link_libs: Vec<String>,
}

/// A probe's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Yes,
    /// The toolchain does not support it: `step` exited non-zero.
    No {
        step: Step,
        output: String,
    },
    /// The program built, but nothing can run it to find out.
    Unknown {
        reason: &'static str,
    },
}

/// A probe step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Compile,
    Link,
    Run,
}

/// A probe that could not ask its question.
#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("failed to prepare probe directory {path}: {source}")]
    Scratch {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("{program} crashed or was killed by a signal")]
    Killed { program: String },
}

/// Runs a probe's commands; [`ProcessRunner`] spawns them.
pub trait ProbeRunner {
    /// Run `argv` in `dir` with `env` added to the environment.
    ///
    /// # Errors
    /// [`RunError`] when the program cannot be spawned, read, or
    /// finished within the deadline.
    fn run_in(
        &self,
        argv: &[String],
        env: &[(OsString, OsString)],
        dir: &Path,
    ) -> Result<RunOutput, RunError>;
}

impl ProbeRunner for ProcessRunner {
    fn run_in(
        &self,
        argv: &[String],
        env: &[(OsString, OsString)],
        dir: &Path,
    ) -> Result<RunOutput, RunError> {
        let (program, rest) = argv.split_first().expect("a probe command names a program");
        let mut command = Command::new(program);
        command
            .args(rest)
            .envs(env.iter().cloned())
            .current_dir(dir);
        run_command_with_timeout(command, Path::new(program), Self::DEFAULT_TIMEOUT)
    }
}

/// Answer `request` with `toolchain`, building in a fresh directory
/// under [`ProbeToolchain::dir`] that is removed afterwards.
///
/// # Errors
/// [`ProbeError`] when the directory cannot be prepared or a step's
/// tool cannot be invoked; a step that runs and fails is an answer.
pub fn probe(
    toolchain: &ProbeToolchain<'_>,
    request: &ProbeRequest,
    runner: &dyn ProbeRunner,
) -> Result<Answer, ProbeError> {
    // Concurrent commands share the build directory, so each probe
    // builds in a directory of its own.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = toolchain.dir.join(format!(
        "probe-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let answer = std::fs::create_dir_all(&dir)
        .map_err(|source| ProbeError::Scratch {
            path: dir.clone(),
            source,
        })
        .and_then(|()| probe_in(toolchain, request, &dir, runner));
    let _ = std::fs::remove_dir_all(&dir);
    answer
}

fn probe_in(
    toolchain: &ProbeToolchain<'_>,
    request: &ProbeRequest,
    dir: &Utf8Path,
    runner: &dyn ProbeRunner,
) -> Result<Answer, ProbeError> {
    let source = dir.join(match request.standard.language() {
        SourceLanguage::C => "probe.c",
        SourceLanguage::Cxx => "probe.cc",
    });
    std::fs::write(&source, request.check.source()).map_err(|source_error| {
        ProbeError::Scratch {
            path: source.clone(),
            source: source_error,
        }
    })?;
    let object = dir.join(format!("probe.{}", toolchain.dialect.object_extension()));
    let compile = CompileAction {
        standard: request.standard,
        gnu_extensions: request.gnu_extensions,
        source,
        object: object.clone(),
        mode: CompileMode::Object,
        depfile: None,
        compiler: toolchain.compiler.to_owned(),
        target: toolchain.target.map(str::to_owned),
        sysroot: toolchain.sysroot.cloned(),
        compiler_wrapper: None,
        arguments: request.arguments.clone(),
        description: String::new(),
    };
    let argv = cabin_driver::compile_argv(toolchain.dialect, &compile);
    if let Some(no) = run_step(runner, toolchain, &argv, Step::Compile)? {
        return Ok(no);
    }
    let last = request.check.last_step();
    if last == Step::Compile {
        return Ok(Answer::Yes);
    }

    // Windows needs the `.exe`; elsewhere it is only a file name.
    let executable = dir.join("probe.exe");
    let link = BuildAction::Link(LinkAction {
        linker: toolchain.compiler.to_owned(),
        target: toolchain.target.map(str::to_owned),
        sysroot: toolchain.sysroot.cloned(),
        output: executable.clone(),
        inputs: vec![object],
        arguments: request.link_flags.clone(),
        link_libs: request.link_libs.clone(),
        frameworks: Vec::new(),
        description: String::new(),
    });
    let argv = cabin_driver::lower(toolchain.dialect, &link).command;
    if let Some(no) = run_step(runner, toolchain, &argv, Step::Link)? {
        return Ok(no);
    }
    if last == Step::Link {
        return Ok(Answer::Yes);
    }

    let argv = match toolchain.execution {
        Execution::Native => vec![executable.into_string()],
        Execution::Runner(runner_command) => {
            let mut argv = runner_command.to_vec();
            argv.push(executable.into_string());
            argv
        }
        Execution::Unavailable => {
            return Ok(Answer::Unknown {
                reason: "the build machine cannot run the target's programs and no runner is configured",
            });
        }
    };
    Ok(run_step(runner, toolchain, &argv, Step::Run)?.unwrap_or(Answer::Yes))
}

/// Run one step: `None` when it succeeded, the failing answer when it
/// exited non-zero.
fn run_step(
    runner: &dyn ProbeRunner,
    toolchain: &ProbeToolchain<'_>,
    argv: &[String],
    step: Step,
) -> Result<Option<Answer>, ProbeError> {
    let output = runner.run_in(argv, toolchain.env, toolchain.dir.as_std_path())?;
    match output.status {
        0 => Ok(None),
        // A compiler or linker killed by a signal (-1) or crashed (a
        // negative NTSTATUS on Windows) did not answer; a probe program
        // that crashes did.
        status if status < 0 && step != Step::Run => Err(ProbeError::Killed {
            program: argv[0].clone(),
        }),
        _ => Ok(Some(Answer::No {
            step,
            output: output.combined(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use cabin_core::{CStandard, CxxStandard, OptLevel};

    use super::*;

    /// Answers each step with the next scripted exit status and
    /// records what it ran.
    struct Scripted {
        statuses: RefCell<Vec<Result<i32, ()>>>,
        ran: RefCell<Vec<(Vec<String>, PathBuf)>>,
    }

    impl Scripted {
        fn new(statuses: &[Result<i32, ()>]) -> Self {
            Self {
                statuses: RefCell::new(statuses.iter().rev().copied().collect()),
                ran: RefCell::new(Vec::new()),
            }
        }

        fn argvs(&self) -> Vec<Vec<String>> {
            self.ran
                .borrow()
                .iter()
                .map(|(argv, _)| argv.clone())
                .collect()
        }
    }

    impl ProbeRunner for Scripted {
        fn run_in(
            &self,
            argv: &[String],
            _env: &[(OsString, OsString)],
            dir: &Path,
        ) -> Result<RunOutput, RunError> {
            assert!(dir.is_dir(), "steps run in the build's directory");
            self.ran
                .borrow_mut()
                .push((argv.to_vec(), dir.to_path_buf()));
            match self
                .statuses
                .borrow_mut()
                .pop()
                .expect("an unexpected step ran")
            {
                Ok(status) => Ok(RunOutput {
                    status,
                    stdout: String::new(),
                    stderr: format!("status {status}"),
                }),
                Err(()) => Err(RunError::Spawn {
                    path: PathBuf::from(&argv[0]),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                }),
            }
        }
    }

    fn request(standard: LanguageStandard, check: Check) -> ProbeRequest {
        ProbeRequest {
            standard,
            gnu_extensions: true,
            check,
            arguments: CompileArguments {
                opt_level: OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: Vec::new(),
                system_include_dirs: Vec::new(),
                defines: Vec::new(),
                instruction_sets: Vec::new(),
                extra_flags: vec!["-Werror".to_owned()],
            },
            link_flags: vec!["-static".to_owned()],
            link_libs: vec!["m".to_owned()],
        }
    }

    fn c() -> LanguageStandard {
        LanguageStandard::C(CStandard::C11)
    }

    fn run(
        execution: Execution<'_>,
        request: &ProbeRequest,
        runner: &Scripted,
    ) -> (Result<Answer, ProbeError>, assert_fs::TempDir) {
        let scratch = assert_fs::TempDir::new().unwrap();
        let sysroot = Sysroot {
            path: "/sdk root".into(),
            apple: false,
        };
        let toolchain = ProbeToolchain {
            dialect: Dialect::GnuLike,
            compiler: Utf8Path::new("/bin/cc"),
            target: Some("aarch64-unknown-linux-gnu"),
            sysroot: Some(&sysroot),
            env: &[],
            dir: Utf8Path::from_path(scratch.path()).unwrap(),
            execution,
        };
        (probe(&toolchain, request, runner), scratch)
    }

    #[test]
    fn each_check_stops_at_its_last_step() {
        for (check, steps) in [
            (Check::Header("stdio.h".into()), 1),
            (Check::Compiles("int x;".into()), 1),
            (
                Check::Type {
                    name: "size_t".into(),
                    includes: vec!["stddef.h".into()],
                },
                1,
            ),
            (
                Check::Symbol {
                    name: "printf".into(),
                    includes: vec!["stdio.h".into()],
                },
                2,
            ),
            (Check::Links("int main(void) { return 0; }".into()), 2),
            (Check::Runs("int main(void) { return 0; }".into()), 3),
        ] {
            let runner = Scripted::new(&[Ok(0), Ok(0), Ok(0)][..steps]);
            let (answer, _scratch) = run(Execution::Native, &request(c(), check), &runner);
            assert_eq!(answer.unwrap(), Answer::Yes);
            assert_eq!(runner.argvs().len(), steps);
        }
    }

    #[test]
    fn a_failing_step_is_an_answer_naming_the_step() {
        let check = || Check::Runs("int main(void) { return 0; }".into());
        for (statuses, step) in [
            (&[Ok(1)][..], Step::Compile),
            (&[Ok(0), Ok(1)][..], Step::Link),
            (&[Ok(0), Ok(0), Ok(3)][..], Step::Run),
            // A crashing probe program is an answer too.
            (&[Ok(0), Ok(0), Ok(-1)][..], Step::Run),
        ] {
            let runner = Scripted::new(statuses);
            let (answer, _scratch) = run(Execution::Native, &request(c(), check()), &runner);
            let Answer::No {
                step: failed,
                output,
            } = answer.unwrap()
            else {
                panic!("expected a failing step");
            };
            assert_eq!(failed, step);
            assert!(output.starts_with("status "), "{output}");
        }
    }

    #[test]
    fn a_tool_that_cannot_run_is_an_error_not_an_answer() {
        let check = || Check::Links("int main(void) { return 0; }".into());
        let runner = Scripted::new(&[Err(())]);
        let (answer, _scratch) = run(Execution::Native, &request(c(), check()), &runner);
        assert!(matches!(
            answer,
            Err(ProbeError::Run(RunError::Spawn { .. }))
        ));

        // Killed by a signal, and crashed with an access violation on Windows.
        for statuses in [&[Ok(0), Ok(-1)][..], &[Ok(-1_073_741_819)][..]] {
            let runner = Scripted::new(statuses);
            let (answer, _scratch) = run(Execution::Native, &request(c(), check()), &runner);
            assert!(matches!(answer, Err(ProbeError::Killed { .. })));
        }
    }

    #[test]
    fn a_program_nothing_can_run_is_unknown_once_it_builds() {
        let runs = || Check::Runs("int main(void) { return 0; }".into());
        let runner = Scripted::new(&[Ok(0), Ok(0)]);
        let (answer, _scratch) = run(Execution::Unavailable, &request(c(), runs()), &runner);
        assert!(matches!(answer.unwrap(), Answer::Unknown { .. }));
        assert_eq!(runner.argvs().len(), 2);

        // What cannot build cannot run either.
        let runner = Scripted::new(&[Ok(0), Ok(1)]);
        let (answer, _scratch) = run(Execution::Unavailable, &request(c(), runs()), &runner);
        assert!(matches!(
            answer.unwrap(),
            Answer::No {
                step: Step::Link,
                ..
            }
        ));
    }

    #[test]
    fn steps_build_as_the_build_does_and_run_under_the_runner() {
        let runner_command = ["qemu-aarch64".to_owned(), "-L".to_owned(), "/sr".to_owned()];
        let runner = Scripted::new(&[Ok(0), Ok(0), Ok(0)]);
        let cxx = request(
            LanguageStandard::Cxx(CxxStandard::Cxx17),
            Check::Runs("int main() { return 0; }".into()),
        );
        let (answer, scratch) = run(Execution::Runner(&runner_command), &cxx, &runner);
        assert_eq!(answer.unwrap(), Answer::Yes);
        assert!(
            runner
                .ran
                .borrow()
                .iter()
                .all(|(_, dir)| dir == scratch.path()),
            "steps run where the build runs its commands"
        );

        let [compile, link, program] = &runner.argvs()[..] else {
            panic!("expected three steps");
        };
        // Files live in a directory of the probe's own, removed afterwards.
        let source = Utf8Path::new(&compile[compile.len() - 3]);
        let dir = source.parent().unwrap();
        assert_eq!(dir.parent().unwrap(), scratch.path());
        assert!(!dir.exists());
        assert_eq!(source, dir.join("probe.cc"));
        let object = dir.join("probe.o");
        let executable = dir.join("probe.exe");
        assert_eq!(
            compile[..4],
            [
                "/bin/cc",
                "--target=aarch64-unknown-linux-gnu",
                "--sysroot=/sdk root",
                "-std=gnu++17"
            ]
        );
        assert!(compile.contains(&"-Werror".to_owned()));
        assert_eq!(
            link[..3],
            [
                "/bin/cc",
                "--target=aarch64-unknown-linux-gnu",
                "--sysroot=/sdk root"
            ]
        );
        assert!(link.contains(&object.to_string()));
        assert!(link.contains(&"-static".to_owned()));
        assert!(link.contains(&"-lm".to_owned()));
        assert_eq!(
            program[..],
            ["qemu-aarch64", "-L", "/sr", executable.as_str()]
        );
    }
}
