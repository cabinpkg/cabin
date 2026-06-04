//! Lower semantic [`BuildAction`]s into concrete command argv for a
//! GNU/Clang-like toolchain.
//!
//! This is the single toolchain-lowering boundary. A future MSVC
//! driver adds a sibling `lower_msvc` here (different flag spellings,
//! `/showIncludes` dependency tracking, `lib.exe` archiving) and the
//! planner, the semantic IR, and the Ninja writer stay unchanged: the
//! writer renders whatever [`LoweredAction`] it is handed.

use std::path::{Path, PathBuf};

use cabin_core::SourceLanguage;

use crate::action::{ArchiveAction, BuildAction, CompileAction, CompileMode, LinkAction};
use crate::error::BuildError;

/// A fully-lowered action: the backend artifact the Ninja writer
/// renders.
///
/// Mirrors the pre-IR action shape — argv plus the metadata Ninja
/// needs — but is *produced* by lowering, never authored directly by
/// the planner. Keep it a backend artifact, not a planner
/// representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredAction {
    /// Categorization the backend switches on to pick a rule.
    pub kind: LoweredActionKind,
    /// Inputs that participate in the command.
    pub inputs: Vec<PathBuf>,
    /// Inputs the action implicitly depends on but that are not
    /// arguments.
    pub implicit_inputs: Vec<PathBuf>,
    /// Files this action produces.
    pub outputs: Vec<PathBuf>,
    /// Optional Makefile-style depfile path.
    pub depfile: Option<PathBuf>,
    /// Argv-style command, ready to be shell-quoted by the backend.
    pub command: Vec<String>,
    /// Short, human-readable description for build output.
    pub description: String,
}

/// Categorization of a lowered action. A closed set; new variants
/// require explicit handling by every backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweredActionKind {
    /// Compile a single C translation unit (`.c`) to an object file.
    CompileC,
    /// Compile a single C++ translation unit to an object file.
    CompileCpp,
    /// Parse + semantic-check a C translation unit with
    /// `-fsyntax-only`. Produces no object file; a stamp output
    /// records success.
    SyntaxCheckC,
    /// Parse + semantic-check a C++ translation unit with
    /// `-fsyntax-only`. Produces no object file; a stamp output
    /// records success.
    SyntaxCheckCpp,
    /// Archive a set of object files into a static library.
    ArchiveStaticLibrary,
    /// Link object files plus static archives into an executable.
    LinkExecutable,
}

/// Lower one semantic [`BuildAction`] into a [`LoweredAction`] for a
/// GNU/Clang-like toolchain.
///
/// # Errors
/// Returns [`BuildError::NonUtf8Path`] when any path that must be
/// embedded in the command line is not valid UTF-8.
pub fn lower_gnu_like(action: &BuildAction) -> Result<LoweredAction, BuildError> {
    match action {
        BuildAction::Compile(compile) => lower_compile(compile),
        BuildAction::Archive(archive) => lower_archive(archive),
        BuildAction::Link(link) => lower_link(link),
    }
}

fn lower_compile(compile: &CompileAction) -> Result<LoweredAction, BuildError> {
    // `compile_argv_gnu` returns the unwrapped compiler argv; the
    // compiler-cache wrapper is a run-command-only prefix and is
    // applied here, never in the shared builder (so
    // `compile_commands.json` keeps the underlying compiler).
    let mut command = compile_argv_gnu(compile)?;
    if let Some(wrapper) = &compile.compiler_wrapper {
        command.insert(0, path_to_str(wrapper)?.to_owned());
    }
    let (kind, outputs) = match &compile.mode {
        CompileMode::Object => {
            let kind = match compile.language {
                SourceLanguage::C => LoweredActionKind::CompileC,
                SourceLanguage::Cxx => LoweredActionKind::CompileCpp,
            };
            (kind, vec![compile.object.clone()])
        }
        CompileMode::SyntaxOnly { stamp } => {
            let kind = match compile.language {
                SourceLanguage::C => LoweredActionKind::SyntaxCheckC,
                SourceLanguage::Cxx => LoweredActionKind::SyntaxCheckCpp,
            };
            (kind, vec![stamp.clone()])
        }
    };
    Ok(LoweredAction {
        kind,
        inputs: vec![compile.source.clone()],
        implicit_inputs: compile.implicit_inputs.clone(),
        outputs,
        depfile: compile.depfile.clone(),
        command,
        description: compile.description.clone(),
    })
}

/// Build the unwrapped GNU/Clang compiler argv for a compile action.
///
/// "Unwrapped" means the compiler-cache wrapper is *not* applied; this
/// is the form recorded in `compile_commands.json`. [`lower_gnu_like`]
/// prepends the wrapper on top of this for the run command.
///
/// The layout is fixed so it reproduces the historic command lines
/// byte-for-byte: driver, standard/profile flags, the
/// `-MMD -MF <depfile>` (plus `-MT <stamp>` in syntax-only mode)
/// dependency block, defines, includes, escape-hatch flags, and
/// finally the mode-specific tail (`-c <src> -o <obj>` for an object,
/// `<src> -fsyntax-only` for a check).
///
/// # Errors
/// Returns [`BuildError::NonUtf8Path`] when the compiler, depfile,
/// stamp, include, source, or object path is not valid UTF-8.
pub(crate) fn compile_argv_gnu(compile: &CompileAction) -> Result<Vec<String>, BuildError> {
    let args = &compile.arguments;
    let mut out: Vec<String> = Vec::new();
    out.push(path_to_str(&compile.compiler)?.to_owned());
    out.extend(args.std_and_profile_flags.iter().cloned());
    if let Some(depfile) = &compile.depfile {
        out.push("-MMD".to_owned());
        out.push("-MF".to_owned());
        out.push(path_to_str(depfile)?.to_owned());
        // In syntax-only mode the depfile records the stamp (not an
        // object) as its target, so header edits still invalidate the
        // check via Ninja's `deps = gcc` machinery.
        if let CompileMode::SyntaxOnly { stamp } = &compile.mode {
            out.push("-MT".to_owned());
            out.push(path_to_str(stamp)?.to_owned());
        }
    }
    for define in &args.defines {
        out.push(format!("-D{define}"));
    }
    for include in &args.include_dirs {
        out.push("-I".to_owned());
        out.push(path_to_str(include)?.to_owned());
    }
    out.extend(args.extra_flags.iter().cloned());
    match &compile.mode {
        CompileMode::Object => {
            out.push("-c".to_owned());
            out.push(path_to_str(&compile.source)?.to_owned());
            out.push("-o".to_owned());
            out.push(path_to_str(&compile.object)?.to_owned());
        }
        CompileMode::SyntaxOnly { .. } => {
            out.push(path_to_str(&compile.source)?.to_owned());
            out.push("-fsyntax-only".to_owned());
        }
    }
    Ok(out)
}

fn lower_archive(archive: &ArchiveAction) -> Result<LoweredAction, BuildError> {
    // GNU `ar` archives with the `crs` mode flags (create, replace,
    // write index): `ar crs <lib> <obj>...`. A future MSVC lowering
    // would instead emit `lib.exe /OUT:<lib> <obj>...`.
    let mut command = vec![
        path_to_str(&archive.archiver)?.to_owned(),
        "crs".to_owned(),
        path_to_str(&archive.output)?.to_owned(),
    ];
    for input in &archive.inputs {
        command.push(path_to_str(input)?.to_owned());
    }
    Ok(LoweredAction {
        kind: LoweredActionKind::ArchiveStaticLibrary,
        inputs: archive.inputs.clone(),
        implicit_inputs: Vec::new(),
        outputs: vec![archive.output.clone()],
        depfile: None,
        command,
        description: archive.description.clone(),
    })
}

fn lower_link(link: &LinkAction) -> Result<LoweredAction, BuildError> {
    // `<driver> <inputs...> <ldflags...> -o <exe>`.
    let mut command = vec![path_to_str(&link.linker)?.to_owned()];
    for input in &link.inputs {
        command.push(path_to_str(input)?.to_owned());
    }
    command.extend(link.arguments.iter().cloned());
    command.push("-o".to_owned());
    command.push(path_to_str(&link.output)?.to_owned());
    Ok(LoweredAction {
        kind: LoweredActionKind::LinkExecutable,
        inputs: link.inputs.clone(),
        implicit_inputs: link.implicit_inputs.clone(),
        outputs: vec![link.output.clone()],
        depfile: None,
        command,
        description: link.description.clone(),
    })
}

fn path_to_str(p: &Path) -> Result<&str, BuildError> {
    p.to_str()
        .ok_or_else(|| BuildError::NonUtf8Path(p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::CompileArguments;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// A C++ compile exercising every argv segment: standard +
    /// profile flags, depfile, a define, an include dir, and an
    /// escape-hatch flag.
    fn cxx_compile(mode: CompileMode) -> CompileAction {
        CompileAction {
            language: SourceLanguage::Cxx,
            source: PathBuf::from("/abs/src/main.cc"),
            object: PathBuf::from("/abs/build/main.o"),
            mode,
            implicit_inputs: vec![PathBuf::from("/abs/build/generated.h")],
            depfile: Some(PathBuf::from("/abs/build/main.o.d")),
            compiler: PathBuf::from("/usr/bin/g++"),
            compiler_wrapper: None,
            arguments: CompileArguments {
                std_and_profile_flags: strs(&["-std=c++17", "-O0"]),
                include_dirs: vec![PathBuf::from("/abs/include")],
                defines: strs(&["FOO=1"]),
                extra_flags: strs(&["-Wall"]),
            },
            description: "CXX /abs/build/main.o".to_owned(),
        }
    }

    #[test]
    fn object_mode_lowers_to_historic_compile_argv() {
        let lowered =
            lower_gnu_like(&BuildAction::Compile(cxx_compile(CompileMode::Object))).unwrap();
        assert_eq!(lowered.kind, LoweredActionKind::CompileCpp);
        assert_eq!(lowered.outputs, vec![PathBuf::from("/abs/build/main.o")]);
        assert_eq!(lowered.inputs, vec![PathBuf::from("/abs/src/main.cc")]);
        assert_eq!(
            lowered.implicit_inputs,
            vec![PathBuf::from("/abs/build/generated.h")]
        );
        assert_eq!(lowered.depfile, Some(PathBuf::from("/abs/build/main.o.d")));
        assert_eq!(lowered.description, "CXX /abs/build/main.o");
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/g++",
                "-std=c++17",
                "-O0",
                "-MMD",
                "-MF",
                "/abs/build/main.o.d",
                "-DFOO=1",
                "-I",
                "/abs/include",
                "-Wall",
                "-c",
                "/abs/src/main.cc",
                "-o",
                "/abs/build/main.o",
            ])
        );
    }

    #[test]
    fn syntax_only_mode_lowers_to_historic_check_argv() {
        let stamp = PathBuf::from("/abs/build/main.o.check");
        let lowered = lower_gnu_like(&BuildAction::Compile(cxx_compile(
            CompileMode::SyntaxOnly {
                stamp: stamp.clone(),
            },
        )))
        .unwrap();
        // SyntaxOnly flips the kind and the single output to the stamp;
        // depfile and implicit inputs are preserved for incrementality.
        assert_eq!(lowered.kind, LoweredActionKind::SyntaxCheckCpp);
        assert_eq!(lowered.outputs, vec![stamp]);
        assert_eq!(lowered.depfile, Some(PathBuf::from("/abs/build/main.o.d")));
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/g++",
                "-std=c++17",
                "-O0",
                "-MMD",
                "-MF",
                "/abs/build/main.o.d",
                "-MT",
                "/abs/build/main.o.check",
                "-DFOO=1",
                "-I",
                "/abs/include",
                "-Wall",
                "/abs/src/main.cc",
                "-fsyntax-only",
            ])
        );
    }

    #[test]
    fn c_compile_lowers_to_c_kinds() {
        let mut c = cxx_compile(CompileMode::Object);
        c.language = SourceLanguage::C;
        assert_eq!(
            lower_gnu_like(&BuildAction::Compile(c.clone()))
                .unwrap()
                .kind,
            LoweredActionKind::CompileC
        );
        c.mode = CompileMode::SyntaxOnly {
            stamp: PathBuf::from("/abs/build/main.o.check"),
        };
        assert_eq!(
            lower_gnu_like(&BuildAction::Compile(c)).unwrap().kind,
            LoweredActionKind::SyntaxCheckC
        );
    }

    #[test]
    fn compiler_wrapper_prefixes_only_the_run_command() {
        let mut c = cxx_compile(CompileMode::Object);
        c.compiler_wrapper = Some(PathBuf::from("/usr/local/bin/ccache"));
        // The run command (lowered) is wrapped...
        let lowered = lower_gnu_like(&BuildAction::Compile(c.clone())).unwrap();
        assert_eq!(lowered.command[0], "/usr/local/bin/ccache");
        assert_eq!(lowered.command[1], "/usr/bin/g++");
        // ...but the shared argv builder (used for compile_commands)
        // never sees the wrapper.
        let unwrapped = compile_argv_gnu(&c).unwrap();
        assert_eq!(unwrapped[0], "/usr/bin/g++");
        assert!(!unwrapped.iter().any(|a| a == "/usr/local/bin/ccache"));
    }

    #[test]
    fn archive_lowers_to_ar_crs_command() {
        let action = BuildAction::Archive(ArchiveAction {
            archiver: PathBuf::from("/usr/bin/ar"),
            output: PathBuf::from("/abs/build/libfoo.a"),
            inputs: vec![
                PathBuf::from("/abs/build/a.o"),
                PathBuf::from("/abs/build/b.o"),
            ],
            description: "AR /abs/build/libfoo.a".to_owned(),
        });
        let lowered = lower_gnu_like(&action).unwrap();
        assert_eq!(lowered.kind, LoweredActionKind::ArchiveStaticLibrary);
        assert_eq!(lowered.outputs, vec![PathBuf::from("/abs/build/libfoo.a")]);
        assert_eq!(lowered.depfile, None);
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/ar",
                "crs",
                "/abs/build/libfoo.a",
                "/abs/build/a.o",
                "/abs/build/b.o",
            ])
        );
    }

    #[test]
    fn link_lowers_to_driver_inputs_ldflags_output() {
        let action = BuildAction::Link(LinkAction {
            linker: PathBuf::from("/usr/bin/g++"),
            output: PathBuf::from("/abs/build/app"),
            inputs: vec![
                PathBuf::from("/abs/build/main.o"),
                PathBuf::from("/abs/build/libfoo.a"),
            ],
            implicit_inputs: vec![],
            arguments: strs(&["-Wl,--as-needed"]),
            description: "LINK /abs/build/app".to_owned(),
        });
        let lowered = lower_gnu_like(&action).unwrap();
        assert_eq!(lowered.kind, LoweredActionKind::LinkExecutable);
        assert_eq!(lowered.outputs, vec![PathBuf::from("/abs/build/app")]);
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/g++",
                "/abs/build/main.o",
                "/abs/build/libfoo.a",
                "-Wl,--as-needed",
                "-o",
                "/abs/build/app",
            ])
        );
    }
}
