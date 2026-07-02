//! Lower the semantic [`BuildAction`] IR into concrete command argv
//! for a [`Dialect`].
//!
//! This is the single point where compile/archive/link intent becomes
//! a real command line. [`lower()`] dispatches on the dialect; the
//! GNU/Clang and MSVC spellings live side by side below.  The planner,
//! the IR, and the Ninja writer never spell a flag themselves - they
//! call [`lower()`] (or [`compile_argv`] for the compilation database).

use camino::Utf8PathBuf;

#[cfg(test)]
use cabin_core::{CStandard, CxxStandard};
use cabin_core::{LanguageStandard, OptLevel, SourceLanguage};

use crate::action::{ArchiveAction, BuildAction, CompileAction, CompileMode, LinkAction};
use crate::dialect::Dialect;

/// A fully-lowered action: the backend artifact the Ninja writer
/// renders.
///
/// Mirrors the IR action shape - argv plus the metadata Ninja needs -
/// but is *produced* by lowering, never authored directly by the
/// planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredAction {
    /// Categorization the backend switches on to pick a rule.
    pub kind: LoweredActionKind,
    /// Inputs that participate in the command.
    pub inputs: Vec<Utf8PathBuf>,
    /// Inputs the action implicitly depends on but that are not
    /// arguments.
    pub implicit_inputs: Vec<Utf8PathBuf>,
    /// Files this action produces.
    pub outputs: Vec<Utf8PathBuf>,
    /// Optional Makefile-style depfile path.  Only the GNU/Clang
    /// dialect populates this; the MSVC dialect tracks dependencies
    /// through Ninja's `deps = msvc` and leaves it `None`.
    pub depfile: Option<Utf8PathBuf>,
    /// Argv-style command, ready to be shell-quoted by the backend.
    pub command: Vec<String>,
    /// Short, human-readable description for build output.
    pub description: String,
}

/// Categorization of a lowered action.  A closed set; new variants
/// require explicit handling by every backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweredActionKind {
    /// Compile a single C translation unit (`.c`) to an object file.
    CompileC,
    /// Compile a single C++ translation unit to an object file.
    CompileCpp,
    /// Parse + semantic-check a C translation unit.  Produces no
    /// object file; a stamp output records success.
    SyntaxCheckC,
    /// Parse + semantic-check a C++ translation unit.  Produces no
    /// object file; a stamp output records success.
    SyntaxCheckCpp,
    /// Archive a set of object files into a static library.
    ArchiveStaticLibrary,
    /// Link object files plus static archives into an executable.
    LinkExecutable,
}

/// Lower one semantic [`BuildAction`] for `dialect`.
///
/// Infallible: every path in the semantic IR is already
/// [`camino::Utf8Path`], so embedding it in a command line cannot
/// fail.
#[must_use]
pub fn lower(dialect: Dialect, action: &BuildAction) -> LoweredAction {
    match action {
        BuildAction::Compile(compile) => lower_compile(dialect, compile),
        BuildAction::Archive(archive) => lower_archive(dialect, archive),
        BuildAction::Link(link) => lower_link(dialect, link),
    }
}

/// Build the unwrapped compiler argv for a compile action in
/// `dialect` - the form recorded in `compile_commands.json` (no
/// compiler wrapper). [`lower()`] prepends the wrapper on top of
/// this for the run command.
#[must_use]
pub fn compile_argv(dialect: Dialect, compile: &CompileAction) -> Vec<String> {
    match dialect {
        Dialect::GnuLike => compile_argv_gnu(compile),
        Dialect::Msvc => compile_argv_msvc(compile),
    }
}

fn lower_compile(dialect: Dialect, compile: &CompileAction) -> LoweredAction {
    // The compiler wrapper is a run-command-only prefix and is
    // applied here, never in the shared argv builder (so
    // `compile_commands.json` keeps the underlying compiler).
    let mut command = compile_argv(dialect, compile);
    if let Some(wrapper) = &compile.compiler_wrapper {
        command.insert(0, wrapper.as_str().to_owned());
    }
    let (kind, outputs) = match &compile.mode {
        CompileMode::Object => {
            let kind = match compile.standard.language() {
                SourceLanguage::C => LoweredActionKind::CompileC,
                SourceLanguage::Cxx => LoweredActionKind::CompileCpp,
            };
            (kind, vec![compile.object.clone()])
        }
        CompileMode::SyntaxOnly { stamp } => {
            let kind = match compile.standard.language() {
                SourceLanguage::C => LoweredActionKind::SyntaxCheckC,
                SourceLanguage::Cxx => LoweredActionKind::SyntaxCheckCpp,
            };
            (kind, vec![stamp.clone()])
        }
    };
    // Only the GNU/Clang dialect emits a Makefile depfile; MSVC
    // tracks headers via `/showIncludes` (Ninja `deps = msvc`).
    let depfile = match dialect {
        Dialect::GnuLike => compile.depfile.clone(),
        Dialect::Msvc => None,
    };
    LoweredAction {
        kind,
        inputs: vec![compile.source.clone()],
        implicit_inputs: compile.implicit_inputs.clone(),
        outputs,
        depfile,
        command,
        description: compile.description.clone(),
    }
}

// ---------------------------------------------------------------
// GNU / Clang dialect.
// ---------------------------------------------------------------

fn gnu_std_flag(standard: LanguageStandard) -> String {
    format!("-std={standard}")
}

/// GNU/Clang compile argv.  The layout is fixed so it reproduces the
/// historic command lines byte-for-byte: driver, standard, profile
/// (`-O<n>` / `-g` / `-DNDEBUG`), the `-MD -MF <depfile>` (plus
/// `-MT <stamp>` in syntax-only mode) dependency block, defines,
/// includes, system includes, escape-hatch flags, and the
/// mode-specific tail.
fn compile_argv_gnu(compile: &CompileAction) -> Vec<String> {
    let args = &compile.arguments;
    let mut out: Vec<String> = Vec::new();
    out.push(compile.compiler.as_str().to_owned());
    out.push(gnu_std_flag(compile.standard));
    out.push(args.opt_level.as_flag().to_owned());
    if args.debug_info {
        out.push("-g".to_owned());
    }
    if args.define_ndebug {
        out.push("-DNDEBUG".to_owned());
    }
    if let Some(depfile) = &compile.depfile {
        // `-MD`, not `-MMD`: `-MMD` omits headers found through
        // system include dirs, so an edit under an `-isystem` path
        // (a foundation port, an extracted registry package, a
        // pkg-config dir) would stop invalidating rebuilds.
        out.push("-MD".to_owned());
        out.push("-MF".to_owned());
        out.push(depfile.as_str().to_owned());
        // In syntax-only mode the depfile records the stamp (not an
        // object) as its target, so header edits still invalidate the
        // check via Ninja's `deps = gcc` machinery.
        if let CompileMode::SyntaxOnly { stamp } = &compile.mode {
            out.push("-MT".to_owned());
            out.push(stamp.as_str().to_owned());
        }
    }
    for define in &args.defines {
        out.push(format!("-D{define}"));
    }
    for include in &args.include_dirs {
        out.push("-I".to_owned());
        out.push(include.as_str().to_owned());
    }
    // System include dirs come after the user dirs: `-isystem`
    // paths are searched after every `-I` path, so spelling them
    // last keeps argv order aligned with the actual search order.
    for include in &args.system_include_dirs {
        out.push("-isystem".to_owned());
        out.push(include.as_str().to_owned());
    }
    out.extend(args.extra_flags.iter().cloned());
    match &compile.mode {
        CompileMode::Object => {
            out.push("-c".to_owned());
            out.push(compile.source.as_str().to_owned());
            out.push("-o".to_owned());
            out.push(compile.object.as_str().to_owned());
        }
        CompileMode::SyntaxOnly { .. } => {
            out.push(compile.source.as_str().to_owned());
            out.push("-fsyntax-only".to_owned());
        }
    }
    out
}

fn lower_archive_gnu(archive: &ArchiveAction) -> Vec<String> {
    // GNU `ar` archives with the `crs` mode flags (create, replace,
    // write index): `ar crs <lib> <obj>...`.
    let mut command = vec![
        archive.archiver.as_str().to_owned(),
        "crs".to_owned(),
        archive.output.as_str().to_owned(),
    ];
    for input in &archive.inputs {
        command.push(input.as_str().to_owned());
    }
    command
}

fn lower_link_gnu(link: &LinkAction) -> Vec<String> {
    // `<driver> <inputs...> <ldflags...> -l<lib>... -o <exe>`.
    // System libraries follow the archives so a static library's
    // dependencies resolve left-to-right under GNU `ld`.
    let mut command = vec![link.linker.as_str().to_owned()];
    for input in &link.inputs {
        command.push(input.as_str().to_owned());
    }
    command.extend(link.arguments.iter().cloned());
    for lib in &link.link_libs {
        command.push(format!("-l{lib}"));
    }
    command.push("-o".to_owned());
    command.push(link.output.as_str().to_owned());
    command
}

// ---------------------------------------------------------------
// MSVC (`cl.exe` / `lib.exe`) dialect.
// ---------------------------------------------------------------

fn msvc_std_flag(standard: LanguageStandard) -> &'static str {
    standard.msvc_spelling().unwrap_or_else(|| {
        unreachable!(
            "the planner validates MSVC-dialect standards before lowering; `{standard}` has no stable /std: flag"
        )
    })
}

/// The `cl.exe` flag that forces the source's language, prepended to the
/// file name (`/Tp<file>` for C++, `/Tc<file>` for C).
///
/// `cl` infers language from extension and only defaults `.cpp` / `.cxx`
/// to C++; Cabin also classifies `.cc`, `.c++`, and `.C` as C++ (and
/// `.c` as C).  Driving the language explicitly makes the translation
/// unit follow Cabin's own source classification rather than `cl`'s
/// extension table, so every supported extension compiles as the
/// language Cabin intends.
fn msvc_source_flag(language: SourceLanguage) -> &'static str {
    match language {
        SourceLanguage::C => "/Tc",
        SourceLanguage::Cxx => "/Tp",
    }
}

fn msvc_opt_flag(opt: OptLevel) -> &'static str {
    match opt {
        OptLevel::O0 => "/Od",
        OptLevel::O1 | OptLevel::S | OptLevel::Z => "/O1",
        // cl.exe has no `/O3`; `/O2` is its maximum speed setting.
        OptLevel::O2 | OptLevel::O3 => "/O2",
    }
}

/// MSVC `cl.exe` compile argv.  Mirrors the GNU layout with MSVC
/// spellings: `/std:` standard, `/utf-8` source/execution charset,
/// `/EHsc` for C++ exceptions, `/O` optimization, `/Z7` debug info
/// (embedded in the object so parallel compiles never contend on a
/// shared PDB), `/showIncludes` for dependency discovery (no
/// Makefile depfile), `/D` defines, `/I` includes, escape-hatch
/// flags, and the mode-specific tail (`/c /Tp<src> /Fo<obj>` or
/// `/Tp<src> /Zs`, with `/Tc` for C).
fn compile_argv_msvc(compile: &CompileAction) -> Vec<String> {
    let args = &compile.arguments;
    let mut out: Vec<String> = vec![compile.compiler.as_str().to_owned(), "/nologo".to_owned()];
    // GCC and Clang interpret source files as UTF-8 by default, while
    // `cl` falls back to the machine's active code page unless told
    // otherwise.  Pinning `/utf-8` makes the dialects agree on what a
    // source file means, and lets UTF-8-requiring headers (e.g.
    // {fmt}'s `static_assert` on the literal encoding) compile out of
    // the box.  Every `cl` new enough to pass Cabin's `/std:`
    // validation (19.11+) understands the flag, as does `clang-cl`.
    out.push("/utf-8".to_owned());
    out.push(msvc_std_flag(compile.standard).to_owned());
    if compile.standard.language() == SourceLanguage::Cxx {
        out.push("/EHsc".to_owned());
    }
    out.push(msvc_opt_flag(args.opt_level).to_owned());
    if args.debug_info {
        out.push("/Z7".to_owned());
    }
    if args.define_ndebug {
        out.push("/DNDEBUG".to_owned());
    }
    // `/showIncludes` drives Ninja's `deps = msvc`.  Emitted whenever
    // the planner asked for dependency tracking, matching the GNU
    // dialect's `-MD -MF` condition.
    if compile.depfile.is_some() {
        out.push("/showIncludes".to_owned());
    }
    for define in &args.defines {
        out.push(format!("/D{define}"));
    }
    for include in &args.include_dirs {
        out.push("/I".to_owned());
        out.push(include.as_str().to_owned());
    }
    // `/external:I` marks the directory as external; `/external:W0`
    // (emitted once, ahead of the block) silences warnings inside
    // those headers, matching the GNU dialect's `-isystem`
    // semantics.  The planner only populates the system bucket when
    // the detected `cl` / `clang-cl` understands `/external:`.
    if !args.system_include_dirs.is_empty() {
        out.push("/external:W0".to_owned());
    }
    for include in &args.system_include_dirs {
        out.push("/external:I".to_owned());
        out.push(include.as_str().to_owned());
    }
    out.extend(args.extra_flags.iter().cloned());
    let source = format!(
        "{}{}",
        msvc_source_flag(compile.standard.language()),
        compile.source.as_str()
    );
    match &compile.mode {
        CompileMode::Object => {
            out.push("/c".to_owned());
            out.push(source);
            out.push(format!("/Fo{}", compile.object));
        }
        CompileMode::SyntaxOnly { .. } => {
            out.push(source);
            out.push("/Zs".to_owned());
        }
    }
    out
}

fn lower_archive_msvc(archive: &ArchiveAction) -> Vec<String> {
    // `lib /nologo /OUT:<lib> <obj>...`.
    let mut command = vec![
        archive.archiver.as_str().to_owned(),
        "/nologo".to_owned(),
        format!("/OUT:{}", archive.output),
    ];
    for input in &archive.inputs {
        command.push(input.as_str().to_owned());
    }
    command
}

fn lower_link_msvc(link: &LinkAction) -> Vec<String> {
    // `<driver> /nologo <inputs...> <lib>.lib... /Fe<exe> [/link <ldflags...>]`.
    // cl.exe consumes object and `.lib` inputs positionally and
    // forwards `/link` options to the linker, so system libraries are
    // spelled `<name>.lib` and passed as positional inputs after the
    // archives rather than as GNU `-l<name>` flags.
    let mut command = vec![link.linker.as_str().to_owned(), "/nologo".to_owned()];
    for input in &link.inputs {
        command.push(input.as_str().to_owned());
    }
    for lib in &link.link_libs {
        command.push(format!("{lib}.lib"));
    }
    command.push(format!("/Fe{}", link.output));
    if !link.arguments.is_empty() {
        command.push("/link".to_owned());
        command.extend(link.arguments.iter().cloned());
    }
    command
}

// ---------------------------------------------------------------
// Archive / link dispatch.
// ---------------------------------------------------------------

fn lower_archive(dialect: Dialect, archive: &ArchiveAction) -> LoweredAction {
    let command = match dialect {
        Dialect::GnuLike => lower_archive_gnu(archive),
        Dialect::Msvc => lower_archive_msvc(archive),
    };
    LoweredAction {
        kind: LoweredActionKind::ArchiveStaticLibrary,
        inputs: archive.inputs.clone(),
        implicit_inputs: Vec::new(),
        outputs: vec![archive.output.clone()],
        depfile: None,
        command,
        description: archive.description.clone(),
    }
}

fn lower_link(dialect: Dialect, link: &LinkAction) -> LoweredAction {
    let command = match dialect {
        Dialect::GnuLike => lower_link_gnu(link),
        Dialect::Msvc => lower_link_msvc(link),
    };
    LoweredAction {
        kind: LoweredActionKind::LinkExecutable,
        inputs: link.inputs.clone(),
        implicit_inputs: link.implicit_inputs.clone(),
        outputs: vec![link.output.clone()],
        depfile: None,
        command,
        description: link.description.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::CompileArguments;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// A C++ compile exercising every argv segment: optimization +
    /// debug + NDEBUG, depfile, a define, an include dir, a system
    /// include dir, and an escape-hatch flag.
    fn cxx_compile(mode: CompileMode) -> CompileAction {
        CompileAction {
            standard: LanguageStandard::Cxx(CxxStandard::Cxx17),
            source: Utf8PathBuf::from("/abs/src/main.cc"),
            object: Utf8PathBuf::from("/abs/build/main.o"),
            mode,
            implicit_inputs: vec![Utf8PathBuf::from("/abs/build/generated.h")],
            depfile: Some(Utf8PathBuf::from("/abs/build/main.o.d")),
            compiler: Utf8PathBuf::from("/usr/bin/g++"),
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: vec![Utf8PathBuf::from("/abs/include")],
                system_include_dirs: vec![Utf8PathBuf::from("/abs/dep/include")],
                defines: strs(&["FOO=1"]),
                extra_flags: strs(&["-Wall"]),
            },
            description: "CXX /abs/build/main.o".to_owned(),
        }
    }

    #[test]
    fn gnu_object_mode_lowers_to_historic_compile_argv() {
        let lowered = lower(
            Dialect::GnuLike,
            &BuildAction::Compile(cxx_compile(CompileMode::Object)),
        );
        assert_eq!(lowered.kind, LoweredActionKind::CompileCpp);
        assert_eq!(
            lowered.outputs,
            vec![Utf8PathBuf::from("/abs/build/main.o")]
        );
        assert_eq!(lowered.inputs, vec![Utf8PathBuf::from("/abs/src/main.cc")]);
        assert_eq!(
            lowered.depfile,
            Some(Utf8PathBuf::from("/abs/build/main.o.d"))
        );
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/g++",
                "-std=c++17",
                "-O0",
                "-MD",
                "-MF",
                "/abs/build/main.o.d",
                "-DFOO=1",
                "-I",
                "/abs/include",
                "-isystem",
                "/abs/dep/include",
                "-Wall",
                "-c",
                "/abs/src/main.cc",
                "-o",
                "/abs/build/main.o",
            ])
        );
    }

    #[test]
    fn gnu_dialect_spells_declared_standards() {
        let mut compile = cxx_compile(CompileMode::Object);
        compile.standard = LanguageStandard::Cxx(CxxStandard::Cxx20);
        let lowered = lower(Dialect::GnuLike, &BuildAction::Compile(compile));
        assert_eq!(lowered.command[1], "-std=c++20");

        let mut compile = cxx_compile(CompileMode::Object);
        compile.standard = LanguageStandard::C(CStandard::C99);
        let lowered = lower(Dialect::GnuLike, &BuildAction::Compile(compile));
        assert_eq!(lowered.kind, LoweredActionKind::CompileC);
        assert_eq!(lowered.command[1], "-std=c99");
    }

    #[test]
    fn msvc_dialect_spells_declared_standards() {
        let mut compile = cxx_compile(CompileMode::Object);
        compile.standard = LanguageStandard::Cxx(CxxStandard::Cxx20);
        let lowered = lower(Dialect::Msvc, &BuildAction::Compile(compile));
        // `/std:` sits after `cl` `/nologo` `/utf-8`.
        assert_eq!(lowered.command[3], "/std:c++20");

        let mut compile = cxx_compile(CompileMode::Object);
        compile.standard = LanguageStandard::C(CStandard::C17);
        let lowered = lower(Dialect::Msvc, &BuildAction::Compile(compile));
        assert_eq!(lowered.kind, LoweredActionKind::CompileC);
        assert_eq!(lowered.command[3], "/std:c17");
        // The C compile keeps /EHsc off and forces /Tc.
        assert!(!lowered.command.iter().any(|a| a == "/EHsc"));
        assert!(lowered.command.iter().any(|a| a.starts_with("/Tc")));
    }

    #[test]
    fn gnu_debug_and_ndebug_flags_follow_std_and_opt() {
        let mut c = cxx_compile(CompileMode::Object);
        c.arguments.opt_level = OptLevel::O3;
        c.arguments.debug_info = true;
        c.arguments.define_ndebug = true;
        let lowered = lower(Dialect::GnuLike, &BuildAction::Compile(c));
        assert_eq!(
            &lowered.command[1..5],
            &strs(&["-std=c++17", "-O3", "-g", "-DNDEBUG"])[..]
        );
    }

    #[test]
    fn gnu_syntax_only_mode_lowers_to_historic_check_argv() {
        let stamp = Utf8PathBuf::from("/abs/build/main.o.check");
        let lowered = lower(
            Dialect::GnuLike,
            &BuildAction::Compile(cxx_compile(CompileMode::SyntaxOnly {
                stamp: stamp.clone(),
            })),
        );
        assert_eq!(lowered.kind, LoweredActionKind::SyntaxCheckCpp);
        assert_eq!(lowered.outputs, vec![stamp]);
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/g++",
                "-std=c++17",
                "-O0",
                "-MD",
                "-MF",
                "/abs/build/main.o.d",
                "-MT",
                "/abs/build/main.o.check",
                "-DFOO=1",
                "-I",
                "/abs/include",
                "-isystem",
                "/abs/dep/include",
                "-Wall",
                "/abs/src/main.cc",
                "-fsyntax-only",
            ])
        );
    }

    #[test]
    fn c_compile_lowers_to_c_kinds_and_c_standard() {
        let mut c = cxx_compile(CompileMode::Object);
        c.standard = LanguageStandard::C(CStandard::C11);
        let lowered = lower(Dialect::GnuLike, &BuildAction::Compile(c.clone()));
        assert_eq!(lowered.kind, LoweredActionKind::CompileC);
        assert_eq!(lowered.command[1], "-std=c11");
        c.mode = CompileMode::SyntaxOnly {
            stamp: Utf8PathBuf::from("/abs/build/main.o.check"),
        };
        assert_eq!(
            lower(Dialect::GnuLike, &BuildAction::Compile(c)).kind,
            LoweredActionKind::SyntaxCheckC
        );
    }

    #[test]
    fn compiler_wrapper_prefixes_only_the_run_command() {
        let mut c = cxx_compile(CompileMode::Object);
        c.compiler_wrapper = Some(Utf8PathBuf::from("/usr/local/bin/ccache"));
        let lowered = lower(Dialect::GnuLike, &BuildAction::Compile(c.clone()));
        assert_eq!(lowered.command[0], "/usr/local/bin/ccache");
        assert_eq!(lowered.command[1], "/usr/bin/g++");
        // The shared argv builder (used for compile_commands) never
        // sees the wrapper.
        let unwrapped = compile_argv(Dialect::GnuLike, &c);
        assert_eq!(unwrapped[0], "/usr/bin/g++");
        assert!(!unwrapped.iter().any(|a| a == "/usr/local/bin/ccache"));
    }

    #[test]
    fn gnu_archive_lowers_to_ar_crs_command() {
        let action = BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("/usr/bin/ar"),
            output: Utf8PathBuf::from("/abs/build/libfoo.a"),
            inputs: vec![
                Utf8PathBuf::from("/abs/build/a.o"),
                Utf8PathBuf::from("/abs/build/b.o"),
            ],
            description: "AR /abs/build/libfoo.a".to_owned(),
        });
        let lowered = lower(Dialect::GnuLike, &action);
        assert_eq!(lowered.kind, LoweredActionKind::ArchiveStaticLibrary);
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
    fn gnu_link_lowers_to_driver_inputs_ldflags_output() {
        let action = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("/usr/bin/g++"),
            output: Utf8PathBuf::from("/abs/build/app"),
            inputs: vec![
                Utf8PathBuf::from("/abs/build/main.o"),
                Utf8PathBuf::from("/abs/build/libfoo.a"),
            ],
            implicit_inputs: vec![],
            arguments: strs(&["-Wl,--as-needed"]),
            link_libs: vec![],
            description: "LINK /abs/build/app".to_owned(),
        });
        let lowered = lower(Dialect::GnuLike, &action);
        assert_eq!(lowered.kind, LoweredActionKind::LinkExecutable);
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

    // -----------------------------------------------------------
    // MSVC dialect.
    // -----------------------------------------------------------

    /// The MSVC analogue of [`cxx_compile`], with MSVC-spelled
    /// escape-hatch flags and `cl` paths.
    fn msvc_cxx_compile(mode: CompileMode) -> CompileAction {
        CompileAction {
            standard: LanguageStandard::Cxx(CxxStandard::Cxx17),
            source: Utf8PathBuf::from("C:/src/main.cc"),
            object: Utf8PathBuf::from("C:/build/main.obj"),
            mode,
            implicit_inputs: vec![],
            depfile: Some(Utf8PathBuf::from("C:/build/main.obj.d")),
            compiler: Utf8PathBuf::from("cl.exe"),
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: OptLevel::O2,
                debug_info: true,
                define_ndebug: true,
                include_dirs: vec![Utf8PathBuf::from("C:/include")],
                system_include_dirs: vec![Utf8PathBuf::from("C:/dep/include")],
                defines: strs(&["FOO=1"]),
                extra_flags: strs(&["/W4"]),
            },
            description: "CXX C:/build/main.obj".to_owned(),
        }
    }

    #[test]
    fn msvc_object_mode_lowers_to_cl_argv_without_depfile() {
        let lowered = lower(
            Dialect::Msvc,
            &BuildAction::Compile(msvc_cxx_compile(CompileMode::Object)),
        );
        assert_eq!(lowered.kind, LoweredActionKind::CompileCpp);
        // MSVC tracks headers via `/showIncludes`, so no Makefile
        // depfile is carried on the lowered action.
        assert_eq!(lowered.depfile, None);
        assert_eq!(
            lowered.command,
            strs(&[
                "cl.exe",
                "/nologo",
                "/utf-8",
                "/std:c++17",
                "/EHsc",
                "/O2",
                "/Z7",
                "/DNDEBUG",
                "/showIncludes",
                "/DFOO=1",
                "/I",
                "C:/include",
                "/external:W0",
                "/external:I",
                "C:/dep/include",
                "/W4",
                "/c",
                "/TpC:/src/main.cc",
                "/FoC:/build/main.obj",
            ])
        );
    }

    #[test]
    fn system_include_markers_are_omitted_when_no_system_dirs_exist() {
        // `/external:W0` must not appear on a command with no external
        // include block, and the GNU spelling must not emit a stray
        // `-isystem`.
        let mut gnu = cxx_compile(CompileMode::Object);
        gnu.arguments.system_include_dirs = Vec::new();
        let gnu_argv = compile_argv(Dialect::GnuLike, &gnu);
        assert!(!gnu_argv.iter().any(|a| a == "-isystem"));

        let mut msvc = msvc_cxx_compile(CompileMode::Object);
        msvc.arguments.system_include_dirs = Vec::new();
        let msvc_argv = compile_argv(Dialect::Msvc, &msvc);
        assert!(!msvc_argv.iter().any(|a| a.starts_with("/external:")));
    }

    #[test]
    fn msvc_syntax_only_uses_zs_and_no_output() {
        let stamp = Utf8PathBuf::from("C:/build/main.obj.check");
        let lowered = lower(
            Dialect::Msvc,
            &BuildAction::Compile(msvc_cxx_compile(CompileMode::SyntaxOnly {
                stamp: stamp.clone(),
            })),
        );
        assert_eq!(lowered.kind, LoweredActionKind::SyntaxCheckCpp);
        assert_eq!(lowered.outputs, vec![stamp]);
        let tail = &lowered.command[lowered.command.len() - 2..];
        assert_eq!(tail, &strs(&["/TpC:/src/main.cc", "/Zs"])[..]);
        assert!(!lowered.command.iter().any(|a| a.starts_with("/Fo")));
    }

    #[test]
    fn msvc_c_compile_uses_c_standard_and_no_ehsc() {
        let mut c = msvc_cxx_compile(CompileMode::Object);
        c.standard = LanguageStandard::C(CStandard::C11);
        let lowered = lower(Dialect::Msvc, &BuildAction::Compile(c));
        assert_eq!(lowered.kind, LoweredActionKind::CompileC);
        assert!(lowered.command.iter().any(|a| a == "/std:c11"));
        assert!(!lowered.command.iter().any(|a| a == "/EHsc"));
        // The UTF-8 charset pin is language-independent.
        assert!(lowered.command.iter().any(|a| a == "/utf-8"));
    }

    #[test]
    fn msvc_forces_source_language_per_classification() {
        // `cl` only defaults `.cpp`/`.cxx` to C++; Cabin drives the
        // language explicitly so any supported extension compiles as the
        // language Cabin classified it.  The source token carries `/Tp`
        // for C++ and `/Tc` for C, with no bare source argument.
        let cxx = lower(
            Dialect::Msvc,
            &BuildAction::Compile(msvc_cxx_compile(CompileMode::Object)),
        );
        assert!(cxx.command.iter().any(|a| a == "/TpC:/src/main.cc"));
        assert!(!cxx.command.iter().any(|a| a == "C:/src/main.cc"));

        let mut c_action = msvc_cxx_compile(CompileMode::Object);
        c_action.standard = LanguageStandard::C(CStandard::C11);
        let c = lower(Dialect::Msvc, &BuildAction::Compile(c_action));
        assert!(c.command.iter().any(|a| a == "/TcC:/src/main.cc"));
        assert!(!c.command.iter().any(|a| a == "/TpC:/src/main.cc"));
    }

    #[test]
    fn msvc_debug_off_omits_z7_and_opt_maps_o3_to_o2() {
        let mut c = msvc_cxx_compile(CompileMode::Object);
        c.arguments.debug_info = false;
        c.arguments.define_ndebug = false;
        c.arguments.opt_level = OptLevel::O3;
        let lowered = lower(Dialect::Msvc, &BuildAction::Compile(c));
        assert!(!lowered.command.iter().any(|a| a == "/Z7"));
        assert!(!lowered.command.iter().any(|a| a == "/DNDEBUG"));
        assert!(lowered.command.iter().any(|a| a == "/O2"));
    }

    #[test]
    fn msvc_archive_lowers_to_lib_out_command() {
        let action = BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("lib.exe"),
            output: Utf8PathBuf::from("C:/build/foo.lib"),
            inputs: vec![
                Utf8PathBuf::from("C:/build/a.obj"),
                Utf8PathBuf::from("C:/build/b.obj"),
            ],
            description: "AR C:/build/foo.lib".to_owned(),
        });
        let lowered = lower(Dialect::Msvc, &action);
        assert_eq!(
            lowered.command,
            strs(&[
                "lib.exe",
                "/nologo",
                "/OUT:C:/build/foo.lib",
                "C:/build/a.obj",
                "C:/build/b.obj",
            ])
        );
    }

    #[test]
    fn msvc_link_lowers_to_cl_fe_with_link_options() {
        let action = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("cl.exe"),
            output: Utf8PathBuf::from("C:/build/app.exe"),
            inputs: vec![
                Utf8PathBuf::from("C:/build/main.obj"),
                Utf8PathBuf::from("C:/build/foo.lib"),
            ],
            implicit_inputs: vec![],
            arguments: strs(&["/SUBSYSTEM:CONSOLE"]),
            link_libs: vec![],
            description: "LINK C:/build/app.exe".to_owned(),
        });
        let lowered = lower(Dialect::Msvc, &action);
        assert_eq!(
            lowered.command,
            strs(&[
                "cl.exe",
                "/nologo",
                "C:/build/main.obj",
                "C:/build/foo.lib",
                "/FeC:/build/app.exe",
                "/link",
                "/SUBSYSTEM:CONSOLE",
            ])
        );
    }

    #[test]
    fn msvc_link_without_ldflags_omits_link_separator() {
        let action = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("cl.exe"),
            output: Utf8PathBuf::from("C:/build/app.exe"),
            inputs: vec![Utf8PathBuf::from("C:/build/main.obj")],
            implicit_inputs: vec![],
            arguments: vec![],
            link_libs: vec![],
            description: "LINK C:/build/app.exe".to_owned(),
        });
        let lowered = lower(Dialect::Msvc, &action);
        assert_eq!(
            lowered.command,
            strs(&[
                "cl.exe",
                "/nologo",
                "C:/build/main.obj",
                "/FeC:/build/app.exe",
            ])
        );
    }

    #[test]
    fn gnu_link_lowers_link_libs_after_archives() {
        // System libraries are spelled `-l<name>` and placed after the
        // archive inputs and ldflags so GNU `ld` resolves them
        // left-to-right against the archives that reference them.
        let action = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("/usr/bin/cc"),
            output: Utf8PathBuf::from("/abs/build/app"),
            inputs: vec![
                Utf8PathBuf::from("/abs/build/main.o"),
                Utf8PathBuf::from("/abs/build/libsqlite3.a"),
            ],
            implicit_inputs: vec![],
            arguments: vec![],
            link_libs: strs(&["pthread", "m"]),
            description: "LINK /abs/build/app".to_owned(),
        });
        let lowered = lower(Dialect::GnuLike, &action);
        assert_eq!(
            lowered.command,
            strs(&[
                "/usr/bin/cc",
                "/abs/build/main.o",
                "/abs/build/libsqlite3.a",
                "-lpthread",
                "-lm",
                "-o",
                "/abs/build/app",
            ])
        );
    }

    #[test]
    fn msvc_link_lowers_link_libs_as_dot_lib_inputs() {
        // MSVC `link` has no `-l<name>`; system libraries are spelled
        // `<name>.lib` and passed as positional inputs (after the
        // archives, before `/Fe`), not GNU-style flags.
        let action = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("cl.exe"),
            output: Utf8PathBuf::from("C:/build/app.exe"),
            inputs: vec![Utf8PathBuf::from("C:/build/main.obj")],
            implicit_inputs: vec![],
            arguments: vec![],
            link_libs: strs(&["user32"]),
            description: "LINK C:/build/app.exe".to_owned(),
        });
        let lowered = lower(Dialect::Msvc, &action);
        assert_eq!(
            lowered.command,
            strs(&[
                "cl.exe",
                "/nologo",
                "C:/build/main.obj",
                "user32.lib",
                "/FeC:/build/app.exe",
            ])
        );
    }
}
