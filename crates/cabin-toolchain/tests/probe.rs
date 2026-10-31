//! Probes against the build machine's default toolchain: GCC or Clang
//! on Linux, Apple Clang on macOS, MSVC on Windows.

use cabin_core::{
    CStandard, CxxStandard, LanguageStandard, OptLevel, TargetPlatform, ToolchainSelection,
    ToolchainSettings,
};
use cabin_driver::{CompileArguments, Dialect};
use cabin_toolchain::{
    Answer, Check, Execution, ProbeError, ProbeRequest, ProbeToolchain, ProcessRunner,
    ResolveInputs, Step, detect_toolchain, msvc_environment, probe, resolve_toolchain,
};
use camino::{Utf8Path, Utf8PathBuf};

struct Host {
    dialect: Dialect,
    cc: Utf8PathBuf,
    cxx: Utf8PathBuf,
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

fn host() -> Host {
    let selection = ToolchainSelection::default();
    let settings = ToolchainSettings::default();
    let platform = TargetPlatform::current();
    let toolchain = resolve_toolchain(&ResolveInputs::from_process(
        &selection, &settings, &platform,
    ))
    .unwrap();
    let report = detect_toolchain(&toolchain, &ProcessRunner).unwrap();
    let dialect = Dialect::from_compiler_kind(report.cxx.identity.kind);
    Host {
        dialect,
        cc: toolchain.cc.expect("a C compiler").path,
        cxx: toolchain.cxx.path,
        env: msvc_environment(dialect == Dialect::Msvc),
    }
}

fn ask(
    host: &Host,
    standard: LanguageStandard,
    check: Check,
    execution: Execution<'_>,
    compiler: Option<&Utf8Path>,
) -> Result<Answer, ProbeError> {
    let compiler = compiler.unwrap_or(match standard {
        LanguageStandard::C(_) => &host.cc,
        LanguageStandard::Cxx(_) => &host.cxx,
    });
    let dir = assert_fs::TempDir::new().unwrap();
    let toolchain = ProbeToolchain {
        dialect: host.dialect,
        compiler,
        target: None,
        sysroot: None,
        env: &host.env,
        dir: Utf8Path::from_path(dir.path()).unwrap(),
        execution,
    };
    let request = ProbeRequest {
        standard,
        gnu_extensions: false,
        check,
        arguments: CompileArguments {
            opt_level: OptLevel::O0,
            debug_info: false,
            define_ndebug: false,
            include_dirs: Vec::new(),
            system_include_dirs: Vec::new(),
            defines: Vec::new(),
            instruction_sets: Vec::new(),
            extra_flags: Vec::new(),
        },
        link_flags: Vec::new(),
        link_libs: Vec::new(),
    };
    probe(&toolchain, &request, &ProcessRunner)
}

fn failed_at(answer: Result<Answer, ProbeError>) -> Step {
    match answer.unwrap() {
        Answer::No { step, .. } => step,
        other => panic!("expected a failing step, got {other:?}"),
    }
}

const C: LanguageStandard = LanguageStandard::C(CStandard::C11);
const CXX: LanguageStandard = LanguageStandard::Cxx(CxxStandard::Cxx17);

#[test]
fn c_checks_answer_from_the_real_compiler() {
    let host = host();
    let yes = |check| ask(&host, C, check, Execution::Native, None).unwrap();
    assert_eq!(yes(Check::Header("stdio.h".into())), Answer::Yes);
    assert_eq!(
        yes(Check::Type {
            name: "size_t".into(),
            includes: vec!["stddef.h".into()],
        }),
        Answer::Yes
    );
    assert_eq!(
        yes(Check::Symbol {
            name: "printf".into(),
            includes: vec!["stdio.h".into()],
        }),
        Answer::Yes
    );
    assert_eq!(
        yes(Check::Symbol {
            name: "EOF".into(),
            includes: vec!["stdio.h".into()],
        }),
        Answer::Yes
    );
    assert_eq!(
        yes(Check::Runs("int main(void) { return 0; }".into())),
        Answer::Yes
    );

    let no = |check| failed_at(ask(&host, C, check, Execution::Native, None));
    assert_eq!(
        no(Check::Header("cabin_no_such_header.h".into())),
        Step::Compile
    );
    assert_eq!(
        no(Check::Type {
            name: "struct cabin_no_such_type".into(),
            includes: vec!["stddef.h".into()],
        }),
        Step::Compile
    );
    // An expression is not a type, and `void` is not complete.
    for name in ["stdin", "void"] {
        assert_eq!(
            no(Check::Type {
                name: name.into(),
                includes: vec!["stdio.h".into()],
            }),
            Step::Compile
        );
    }
    assert_eq!(
        no(Check::Links(
            "int cabin_no_such_function(void);\nint main(void) { return cabin_no_such_function(); }\n"
                .into()
        )),
        Step::Link
    );
    assert_eq!(
        no(Check::Runs("int main(void) { return 3; }".into())),
        Step::Run
    );
}

#[test]
fn cxx_checks_answer_from_the_real_compiler() {
    let host = host();
    assert_eq!(
        ask(
            &host,
            CXX,
            Check::Symbol {
                name: "std::printf".into(),
                includes: vec!["cstdio".into()],
            },
            Execution::Native,
            None,
        )
        .unwrap(),
        Answer::Yes
    );
    assert_eq!(
        ask(
            &host,
            CXX,
            Check::Type {
                name: "std::string".into(),
                includes: vec!["string".into()],
            },
            Execution::Native,
            None,
        )
        .unwrap(),
        Answer::Yes
    );
    assert_eq!(
        failed_at(ask(
            &host,
            CXX,
            Check::Header("cabin_no_such_header".into()),
            Execution::Native,
            None,
        )),
        Step::Compile
    );
}

#[test]
fn a_program_that_cannot_run_here_is_unknown_and_a_missing_compiler_is_an_error() {
    let host = host();
    let runs = || Check::Runs("int main(void) { return 3; }".into());
    assert!(matches!(
        ask(&host, C, runs(), Execution::Unavailable, None).unwrap(),
        Answer::Unknown { .. }
    ));

    let missing = assert_fs::TempDir::new().unwrap();
    let missing = Utf8Path::from_path(missing.path())
        .unwrap()
        .join("no-such-cc");
    assert!(matches!(
        ask(&host, C, runs(), Execution::Native, Some(&missing)),
        Err(ProbeError::Run(_))
    ));
}
