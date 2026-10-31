use std::fmt::Write as _;
use std::path::Path;

use cabin_build::BuildGraph;
use cabin_driver::{
    BuildAction, CompileMode, Dialect, LoweredAction, LoweredActionKind, NinjaDeps, lower,
    windows_quote_arg,
};
use cabin_fs::write_atomic;

use crate::error::NinjaError;

/// The file beside `build.ninja` that records the detected toolchain.
/// Every edge depends on it, so a compiler replaced at the same path
/// rebuilds everything.
pub const TOOLCHAIN_STAMP: &str = "cabin-toolchain.json";

/// The `msvc_deps_prefix` of a compile whose depfile `cabin depfile`
/// reports: it prints each dependency as a line of this prefix and the
/// path, the format Ninja's `deps = msvc` reads.
pub const DEPFILE_DEPS_PREFIX: &str = "cabin-depfile:";

/// Longest archive or link command an edge spells inline; a longer one
/// passes its arguments through a response file.  Windows'
/// `CreateProcess` takes at most 32,767 characters, and on Linux Ninja's
/// `/bin/sh -c <command>` cannot exec a single argument over 128 KiB;
/// both limits keep headroom for the `cabin recreate` wrapper.  Other
/// hosts cap only the total argument size, and the `ar` of macOS and
/// the BSDs reads no response files, so commands there stay inline.
const MAX_INLINE_COMMAND: usize = if cfg!(windows) {
    24 * 1024
} else if cfg!(target_os = "linux") {
    120 * 1024
} else {
    usize::MAX
};

/// Write a `build.ninja` describing `graph` to `path`, and
/// `toolchain_identity` to [`TOOLCHAIN_STAMP`] beside it.  Returns
/// whether `build.ninja` changed.
///
/// Each file is written only when its contents change, and then
/// atomically: the new bytes land in a sibling temporary file and only
/// rename onto `path` after a successful write, so an interrupted run
/// leaves the previous `build.ninja` in place.  The parent directory
/// must already exist.
///
/// `check_stamp_runner` is the path to the `cabin` executable; the
/// syntax-check rule invokes it as `cabin stamp` to run the
/// compiler and stamp the check output without a shell, the
/// GNU-dialect archive rule as `cabin recreate`, and a compile whose
/// depfile Ninja cannot read as `cabin depfile` (see
/// [`render_build_ninja`]).
///
/// The stamp's modification time changes only with
/// `toolchain_identity`, and that is what makes Ninja rebuild every
/// edge.
///
/// # Errors
/// Propagates rendering failures from [`render_build_ninja`]
/// ([`NinjaError::PathHasNewline`], [`NinjaError::ValueHasNewline`], or
/// [`NinjaError::UnquotableArgument`]), and returns [`NinjaError::Io`]
/// when the atomic write to `path` fails (for example, when the parent
/// directory is missing).
#[tracing::instrument(level = "info", skip_all)]
pub fn write_build_ninja(
    path: &Path,
    graph: &BuildGraph,
    check_stamp_runner: &Path,
    toolchain_identity: &str,
) -> Result<bool, NinjaError> {
    let body = render_build_ninja(graph, check_stamp_runner)?;
    let changed = write_if_changed(path, body.as_bytes())?;
    write_if_changed(
        &path.with_file_name(TOOLCHAIN_STAMP),
        toolchain_identity.as_bytes(),
    )?;
    Ok(changed)
}

/// Atomically replace `path` with `body` unless it already holds exactly
/// `body`, and return whether it wrote.  Leaving an identical file alone
/// keeps its modification time for the tools watching it and skips the
/// atomic write's fsyncs, which dominate an unchanged build's write cost.
/// Any IO error is tagged with `path` so callers can point users at the
/// destination that failed.
pub(crate) fn write_if_changed(path: &Path, body: &[u8]) -> Result<bool, NinjaError> {
    if std::fs::read(path).ok().as_deref() == Some(body) {
        return Ok(false);
    }
    write_atomic(path, body).map_err(|source| NinjaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(true)
}

/// Render `graph` as a Ninja build file.
///
/// Pulled out so unit tests can exercise the formatter without touching the
/// filesystem.
///
/// Each semantic [`cabin_build::BuildAction`] is lowered to a concrete
/// command via [`cabin_driver::lower()`] immediately before its edge is
/// rendered - the single point where compile/archive/link intent
/// becomes a command line.  The [`cabin_driver::Dialect`] selects the spelling, so the
/// same actions render as GNU/Clang or MSVC without this writer changing.
///
/// # Errors
/// Returns [`NinjaError::PathHasNewline`] when a path token contains a
/// newline or carriage return, [`NinjaError::ValueHasNewline`] when a command or
/// description value contains a newline or carriage return, and
/// [`NinjaError::UnquotableArgument`] when a command argument cannot be
/// shell-quoted.
pub fn render_build_ninja(
    graph: &BuildGraph,
    check_stamp_runner: &Path,
) -> Result<String, NinjaError> {
    render_with_inline_limit(graph, check_stamp_runner, MAX_INLINE_COMMAND)
}

fn render_with_inline_limit(
    graph: &BuildGraph,
    check_stamp_runner: &Path,
    inline_limit: usize,
) -> Result<String, NinjaError> {
    let mut out = String::new();
    out.push_str("# Generated by cabin. Do not edit by hand.\n");
    out.push_str("ninja_required_version = 1.10\n");
    // Ninja has no escape for `|` in a path; `escape_path` spells it as
    // a reference to this variable instead.
    out.push_str("pipe = |\n\n");

    // Header-dependency discovery is dialect-specific: the GNU/Clang
    // dialect pairs a Makefile `depfile` with `deps = gcc`; MSVC parses
    // `cl /showIncludes` stdout with `deps = msvc` and an
    // `msvc_deps_prefix`.
    let deps = deps_lines(graph.dialect.ninja_deps());
    // Syntax-only checks (`cabin check`) produce no object, so the rule
    // stamps an output to record success and keep Ninja's incrementality
    // working.  The compiler argv is bound to `$checkcmd` (not
    // `$command`): an edge-level `command` binding would shadow the
    // rule's `command` line entirely, dropping the stamp tail.
    let check_command = check_command_line(check_stamp_runner)?;

    out.push_str("rule c_compile\n");
    out.push_str("  command = $command\n");
    out.push_str(&deps);
    out.push_str("  description = $description\n\n");

    out.push_str("rule cxx_compile\n");
    out.push_str("  command = $command\n");
    out.push_str(&deps);
    out.push_str("  description = $description\n\n");

    out.push_str("rule c_check\n  command = ");
    out.push_str(&check_command);
    out.push('\n');
    out.push_str(&deps);
    out.push_str("  description = $description\n\n");

    out.push_str("rule cxx_check\n  command = ");
    out.push_str(&check_command);
    out.push('\n');
    out.push_str(&deps);
    out.push_str("  description = $description\n\n");

    // Emitted only when used, so builds without assembly keep their
    // existing `build.ninja` bytes.
    if graph
        .actions
        .iter()
        .any(|action| matches!(action, BuildAction::Assemble(_)))
    {
        out.push_str("rule assemble\n");
        out.push_str("  command = $command\n");
        if let Some(deps) = graph.dialect.assemble_deps() {
            out.push_str(&deps_lines(deps));
        }
        out.push_str("  description = $description\n\n");
    }

    // Ninja expands `$in_newline` only in a rule's bindings, and a rule
    // with `rspfile_content` must name an `rspfile`; an edge that sets
    // no `rsp_file` writes none.
    let input_lines = if cfg!(windows) && graph.dialect.response_file_input_lines() {
        "  rspfile = $rsp_file\n  rspfile_content = $rsp_head $in_newline $rsp_tail\n"
    } else {
        ""
    };

    out.push_str("rule cxx_archive\n  command = ");
    out.push_str(&archive_command_line(check_stamp_runner, graph.dialect)?);
    out.push('\n');
    out.push_str(input_lines);
    out.push_str("  description = $description\n\n");

    out.push_str("rule link_executable\n");
    out.push_str("  command = $command\n");
    out.push_str(input_lines);
    out.push_str("  description = $description\n\n");

    for action in &graph.actions {
        let lowered = lower(graph.dialect, action);
        let depfile_runner = depfile_needs_translation(action).then_some(check_stamp_runner);
        write_edge(
            &mut out,
            &lowered,
            graph.dialect,
            depfile_runner,
            inline_limit,
        )?;
    }

    if !graph.default_outputs.is_empty() {
        out.push_str("default");
        for output in &graph.default_outputs {
            out.push(' ');
            out.push_str(&escape_path(output.as_str())?);
        }
        out.push('\n');
    }

    Ok(out)
}

/// The `depfile` / `deps` (and, for MSVC, `msvc_deps_prefix`) lines a
/// rule needs for header-dependency discovery in `deps` mode.
/// Returned with trailing newlines, ready to splice into a rule body.
fn deps_lines(deps: NinjaDeps) -> String {
    match deps {
        NinjaDeps::Gcc => "  depfile = $depfile\n  deps = gcc\n".to_owned(),
        NinjaDeps::Msvc { prefix } => format!("  deps = msvc\n  msvc_deps_prefix = {prefix}\n"),
    }
}

/// The `command` line for a syntax-check rule.  A syntax-only compile
/// produces no object, so the rule must run the compiler (`$checkcmd`)
/// and, on success, stamp `$out` to record it for Ninja's
/// incrementality.
///
/// Rather than chain `compiler && stamp` through a shell - which differs
/// per host (`&&` + `touch` under POSIX `/bin/sh`, `cmd /c "… && type
/// nul"` under Windows `CreateProcess`) and forces shell-metacharacter
/// escaping the per-edge `$checkcmd` quoting cannot provide - Cabin runs
/// the compiler through its own `cabin stamp <stamp> -- <argv…>` witness
/// writer.  That spawns the compiler directly (no shell), and writes the
/// stamp only on a zero exit.  One host-independent rule, and no second
/// parsing layer to escape for - this removes the cmd.exe round-trip that
/// mangled paths with `&` / `|` / `()`.
///
/// Only the `cabin` runner token is quoted, using the same platform-aware
/// quoting (`command_line`: POSIX `/bin/sh` vs Windows `CreateProcess`)
/// every other edge command uses, then `$`-escaped for Ninja.  A runner
/// path containing `$` is therefore single-quoted by `shlex` so POSIX
/// `/bin/sh` does not expand it. `$out` and `$checkcmd` stay raw so Ninja
/// expands them as the edge's stamp output and per-edge compiler argv.
fn check_command_line(runner: &Path) -> Result<String, NinjaError> {
    Ok(format!("{} stamp $out -- $checkcmd", runner_token(runner)?))
}

/// The `command` line for the archive rule.  An archiver that updates
/// the library in place runs through the shell-free `cabin recreate
/// $out -- $archivecmd`, which removes `$out` first.
fn archive_command_line(runner: &Path, dialect: Dialect) -> Result<String, NinjaError> {
    if dialect.archive_updates_in_place() {
        Ok(format!(
            "{} recreate $out -- $archivecmd",
            runner_token(runner)?
        ))
    } else {
        Ok("$archivecmd".to_owned())
    }
}

/// The `cabin` runner path as one host-quoted, Ninja-escaped token.
fn runner_token(runner: &Path) -> Result<String, NinjaError> {
    let runner_str = runner.to_string_lossy().into_owned();
    escape_value(&command_line(std::slice::from_ref(&runner_str))?)
}

/// Whether the depfile a GCC-style compile writes could name a path
/// Ninja's depfile parser misreads.  That parser ends a path at any byte
/// outside a fixed set - `;`, `|`, `*`, `<`, `>`, `^`, `` ` ``, control
/// characters, and before Ninja 1.13 also `'`, `"`, `&`, `?` - and has
/// no escape for them, so such a compile reports its dependencies
/// through `cabin depfile` and Ninja's `deps = msvc` line format
/// instead.  The paths checked are the ones a depfile is spelled from:
/// the source, the outputs, the compiler (whose built-in include
/// directories sit beside it), the sysroot, the include directories, and
/// the escape-hatch flags other than `-D` defines.  A header whose own name has such a byte below an
/// ordinary include directory is not foreseen: Ninja then records paths
/// that do not exist, so the compile reruns on every build rather than
/// going stale.
fn depfile_needs_translation(action: &BuildAction) -> bool {
    let unreadable = |path: &str| {
        path.chars().any(|c| {
            !(c.is_ascii_alphanumeric() || !c.is_ascii() || "+,/_:.~(){}%=@[]!- #$\\".contains(c))
        })
    };
    match action {
        BuildAction::Compile(compile) => {
            let stamp = match &compile.mode {
                CompileMode::SyntaxOnly { stamp } => Some(stamp),
                CompileMode::Object => None,
            };
            [&compile.source, &compile.object, &compile.compiler]
                .into_iter()
                .chain(&compile.depfile)
                .chain(stamp)
                .chain(compile.sysroot.as_ref().map(|sysroot| &sysroot.path))
                .chain(&compile.arguments.include_dirs)
                .chain(&compile.arguments.system_include_dirs)
                .any(|path| unreadable(path.as_str()))
                || compile
                    .arguments
                    .extra_flags
                    .iter()
                    .any(|flag| !flag.starts_with("-D") && unreadable(flag))
        }
        BuildAction::Assemble(assemble) => {
            [&assemble.source, &assemble.object, &assemble.assembler]
                .into_iter()
                .chain(&assemble.depfile)
                .chain(assemble.sysroot.as_ref().map(|sysroot| &sysroot.path))
                .chain(&assemble.include_dirs)
                .any(|path| unreadable(path.as_str()))
        }
        BuildAction::Archive(_) | BuildAction::Link(_) => false,
    }
}

/// Write one edge.  `depfile_runner` is the `cabin` executable when the
/// action's depfile must reach Ninja through `cabin depfile` (see
/// [`depfile_needs_translation`]).  An archive or link command longer
/// than `inline_limit` passes its arguments through a response file.
fn write_edge(
    out: &mut String,
    action: &LoweredAction,
    dialect: Dialect,
    depfile_runner: Option<&Path>,
    inline_limit: usize,
) -> Result<(), NinjaError> {
    let rule = match action.kind {
        LoweredActionKind::CompileC => "c_compile",
        LoweredActionKind::CompileCpp => "cxx_compile",
        LoweredActionKind::SyntaxCheckC => "c_check",
        LoweredActionKind::SyntaxCheckCpp => "cxx_check",
        LoweredActionKind::Assemble => "assemble",
        LoweredActionKind::ArchiveStaticLibrary => "cxx_archive",
        LoweredActionKind::LinkExecutable => "link_executable",
    };

    out.push_str("build ");
    let mut first = true;
    for output in &action.outputs {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(&escape_path(output.as_str())?);
    }
    out.push_str(": ");
    out.push_str(rule);
    for input in &action.inputs {
        out.push(' ');
        out.push_str(&escape_path(input.as_str())?);
    }
    // Relative to the build root Ninja runs in (`-C`).
    out.push_str(" | ");
    out.push_str(TOOLCHAIN_STAMP);
    out.push('\n');

    // `cabin depfile <depfile> -- <argv…>` runs the compile, then prints
    // the depfile's dependencies for the edge's `deps = msvc` below.
    let translated_depfile = action.depfile.as_ref().zip(depfile_runner);
    let wrapped: Vec<String>;
    let argv = match translated_depfile {
        Some((depfile, runner)) => {
            wrapped = [
                runner.to_string_lossy().into_owned(),
                "depfile".to_owned(),
                depfile.to_string(),
                "--".to_owned(),
            ]
            .into_iter()
            .chain(action.command.iter().cloned())
            .collect();
            &wrapped
        }
        None => &action.command,
    };
    // Ninja launches each edge's command itself, so the argv is quoted
    // for the platform that will run it (POSIX shell vs.  Windows
    // `CreateProcess`) - distinct from `compile_commands.json`, whose
    // `command` field is always Bash-quoted per the Clang spec.
    let mut command_value = command_line(argv)?;
    // Archive and link commands grow with the number of objects; past the
    // inline limit the arguments move into a response file that Ninja
    // writes before the command runs and removes after it succeeds.
    let mut response_file = Vec::new();
    if matches!(
        action.kind,
        LoweredActionKind::ArchiveStaticLibrary | LoweredActionKind::LinkExecutable
    ) && command_value.len() > inline_limit
        && let Some((program, args)) = action.command.split_first()
    {
        // `+` is outside the target-name grammar, so no other output
        // can have this name.
        let path = format!("{}+.rsp", action.outputs[0]);
        command_value = command_line(&[program.clone(), format!("@{path}")])?;
        response_file = response_file_bindings(action, args, dialect, path);
    }
    // Check edges bind the argv to `$checkcmd` so the `c_check` /
    // `cxx_check` rule can run it through the shell-free
    // `cabin stamp $out -- $checkcmd` witness writer (which stamps
    // `$out` only on a zero exit), and archive edges to `$archivecmd`
    // so the archive rule can wrap it in `cabin recreate`; every other
    // edge binds the plain `command` the rule runs verbatim.
    let command_var = match action.kind {
        LoweredActionKind::SyntaxCheckC | LoweredActionKind::SyntaxCheckCpp => "checkcmd",
        LoweredActionKind::ArchiveStaticLibrary => "archivecmd",
        LoweredActionKind::CompileC
        | LoweredActionKind::CompileCpp
        | LoweredActionKind::Assemble
        | LoweredActionKind::LinkExecutable => "command",
    };
    write_var(out, command_var, &command_value)?;
    // Bound even when `cabin depfile` reads it: the rule's
    // `depfile = $depfile` would otherwise refer to itself.
    if let Some(depfile) = &action.depfile {
        write_var(out, "depfile", depfile.as_str())?;
    }
    if translated_depfile.is_some() {
        write_var(out, "deps", "msvc")?;
        write_var(out, "msvc_deps_prefix", DEPFILE_DEPS_PREFIX)?;
    }
    for (key, value) in &response_file {
        write_var(out, key, value)?;
    }
    write_var(out, "description", &action.description)?;
    out.push('\n');

    Ok(())
}

/// The edge bindings that hand `args` to the tool through the response
/// file at `path`.
fn response_file_bindings(
    action: &LoweredAction,
    args: &[String],
    dialect: Dialect,
    path: String,
) -> Vec<(&'static str, String)> {
    let header = dialect.response_file_header();
    // The rule's `$in_newline` writes the edge's inputs one per line,
    // quoted for the host as `$in` is, which is how MSVC tools read
    // them on Windows; the arguments around them keep their places.
    let inputs = action.inputs.len();
    match (cfg!(windows) && dialect.response_file_input_lines() && inputs > 0)
        .then(|| {
            args.windows(inputs).position(|run| {
                run.iter()
                    .map(String::as_str)
                    .eq(action.inputs.iter().map(|input| input.as_str()))
            })
        })
        .flatten()
    {
        Some(start) => {
            let (before, rest) = args.split_at(start);
            vec![
                ("rsp_file", path),
                (
                    "rsp_head",
                    format!("{header}{}", dialect.response_file_args(before)),
                ),
                ("rsp_tail", dialect.response_file_args(&rest[inputs..])),
            ]
        }
        None => vec![
            ("rspfile", path),
            (
                "rspfile_content",
                format!("{header}{}", dialect.response_file_args(args)),
            ),
        ],
    }
}

fn write_var(out: &mut String, key: &str, value: &str) -> Result<(), NinjaError> {
    // Ninja indents variable definitions inside a build edge with two
    // spaces.  Within a value, `$` is meaningful (variable references
    // and line continuation) and a literal newline ends the value;
    // `escape_value` doubles `$` and rejects newlines so a manifest
    // string cannot inject a fresh statement.
    let _ = writeln!(out, "  {key} = {}", escape_value(value)?);
    Ok(())
}

/// Escape a path for use as a token inside a Ninja `build` edge.
///
/// In token positions, Ninja recognizes `$$` for a literal `$`, `$ ` for a
/// literal space, and `$:` for a literal colon.  A `|` would separate
/// implicit inputs and `$|` is no escape, so it is spelled `${pipe}`, a
/// reference to the file-level `pipe = |` [`render_build_ninja`] writes.
/// A literal newline cannot be represented and is rejected; a lone
/// carriage return has no escape either and trips Ninja's lexer ("lexing
/// error"), so it is rejected the same way.
pub(crate) fn escape_path(s: &str) -> Result<String, NinjaError> {
    if s.contains('\n') || s.contains('\r') {
        return Err(NinjaError::PathHasNewline(s.into()));
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("$ "),
            ':' => out.push_str("$:"),
            '$' => out.push_str("$$"),
            '|' => out.push_str("${pipe}"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Escape a value for the right-hand side of a Ninja variable assignment.
///
/// In value positions `$` is special (used for variable references and
/// line continuation).  Spaces are preserved verbatim.  A literal `\n` or
/// `\r` would terminate the variable line and is rejected so a
/// manifest-derived value cannot inject sibling Ninja declarations.
pub(crate) fn escape_value(s: &str) -> Result<String, NinjaError> {
    if s.contains('\n') || s.contains('\r') {
        return Err(NinjaError::ValueHasNewline(s.to_owned()));
    }
    Ok(s.replace('$', "$$"))
}

/// Join an argv into the single command string Ninja will hand to the
/// platform's process launcher for a `build.ninja` edge.
///
/// The quoting convention is dictated by *how Ninja runs the command*,
/// which is platform- (not dialect-) specific: on POSIX it runs each
/// command through `/bin/sh`, so arguments are POSIX-shell-quoted; on
/// Windows it calls `CreateProcess` with no shell, so arguments follow
/// the `CommandLineToArgvW` convention `CreateProcess` uses to split
/// them back apart.  A MinGW (GCC-dialect) build on Windows still goes
/// through the same `CreateProcess`, so the axis is the host OS.
///
/// This is deliberately separate from [`shell_join`]: the
/// `compile_commands.json` `command` field is specified to use Bash
/// quoting on every platform, so it stays on [`shell_join`].
fn command_line(args: &[String]) -> Result<String, NinjaError> {
    if cfg!(windows) {
        windows_join(args)
    } else {
        shell_join(args)
    }
}

/// Join an argv into a single POSIX-shell command line (also the Bash
/// quoting the Clang compilation-database `command` field requires on
/// every platform).
///
/// Each argument is quoted via [`shlex::try_join`] so a value
/// containing spaces, quotes, dollar signs, backslashes, or other shell
/// metacharacters round-trips back through `sh` (or Clang's Bash-rules
/// unescaper) to the same argv.  Returns
/// [`NinjaError::UnquotableArgument`] when `shlex` refuses an argument
/// (in practice, a NUL byte).
pub(crate) fn shell_join(args: &[String]) -> Result<String, NinjaError> {
    shlex::try_join(args.iter().map(String::as_str)).map_err(|_| {
        let bad = args
            .iter()
            .find(|a| a.contains('\0'))
            .cloned()
            .unwrap_or_default();
        NinjaError::UnquotableArgument(bad)
    })
}

/// Join an argv into a single Windows command line, following the
/// `CommandLineToArgvW` convention `CreateProcess` uses to split it
/// back into arguments.  This is the quoting Ninja needs on Windows,
/// where it launches commands with no shell.
///
/// Returns [`NinjaError::UnquotableArgument`] for an argument
/// containing a NUL byte (which cannot appear in a Windows command
/// line), matching [`shell_join`]'s contract.
fn windows_join(args: &[String]) -> Result<String, NinjaError> {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if arg.contains('\0') {
            return Err(NinjaError::UnquotableArgument(arg.clone()));
        }
        if i > 0 {
            out.push(' ');
        }
        windows_quote_arg(arg, &mut out);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_build::{
        ArchiveAction, BuildAction, BuildGraph, CompileAction, CompileArguments, CompileCommand,
        CompileMode, LinkAction,
    };
    use cabin_core::OptLevel;
    use cabin_driver::Dialect;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;

    // These fixtures are *semantic* actions; the writer lowers them to
    // concrete commands as it renders, so the assertions below check
    // the GNU/Clang lowering as it appears in `build.ninja`.

    fn compile_action() -> BuildAction {
        BuildAction::Compile(CompileAction {
            standard: cabin_core::LanguageStandard::Cxx(cabin_core::CxxStandard::Cxx17),
            gnu_extensions: false,
            source: Utf8PathBuf::from("/abs/src/main.cc"),
            object: Utf8PathBuf::from("/abs/build/main.o"),
            mode: CompileMode::Object,
            depfile: Some(Utf8PathBuf::from("/abs/build/main.o.d")),
            compiler: Utf8PathBuf::from("/usr/bin/g++"),
            target: None,
            sysroot: None,
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: vec![],
                system_include_dirs: vec![],
                defines: vec![],
                instruction_sets: vec![],
                extra_flags: vec![],
            },
            description: "CXX /abs/build/main.o".into(),
        })
    }

    /// Clone the compile fixture and tweak its inner action.  Keeps the
    /// edge-mutation tests legible now that the field lives inside a
    /// [`BuildAction::Compile`].
    fn compile_with(f: impl FnOnce(&mut CompileAction)) -> BuildAction {
        let BuildAction::Compile(mut c) = compile_action() else {
            unreachable!("compile_action builds a compile");
        };
        f(&mut c);
        BuildAction::Compile(c)
    }

    fn archive_action() -> BuildAction {
        BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("/usr/bin/ar"),
            output: Utf8PathBuf::from("/abs/build/libfoo.a"),
            inputs: vec![Utf8PathBuf::from("/abs/build/main.o")],
            description: "AR /abs/build/libfoo.a".into(),
        })
    }

    fn link_action() -> BuildAction {
        BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("/usr/bin/g++"),
            target: None,
            sysroot: None,
            output: Utf8PathBuf::from("/abs/build/hello"),
            inputs: vec![Utf8PathBuf::from("/abs/build/main.o")],
            arguments: vec![],
            link_libs: vec![],
            frameworks: vec![],
            description: "LINK /abs/build/hello".into(),
        })
    }

    fn syntax_check_action() -> BuildAction {
        BuildAction::Compile(CompileAction {
            standard: cabin_core::LanguageStandard::Cxx(cabin_core::CxxStandard::Cxx17),
            gnu_extensions: false,
            source: Utf8PathBuf::from("/abs/src/main.cc"),
            object: Utf8PathBuf::from("/abs/build/main.o"),
            mode: CompileMode::SyntaxOnly {
                stamp: Utf8PathBuf::from("/abs/build/main.o.check"),
            },
            depfile: Some(Utf8PathBuf::from("/abs/build/main.o.d")),
            compiler: Utf8PathBuf::from("/usr/bin/g++"),
            target: None,
            sysroot: None,
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: vec![],
                system_include_dirs: vec![],
                defines: vec![],
                instruction_sets: vec![],
                extra_flags: vec![],
            },
            description: "CHECK /abs/build/main.o".into(),
        })
    }

    fn graph_with(actions: Vec<BuildAction>, defaults: Vec<Utf8PathBuf>) -> BuildGraph {
        BuildGraph {
            actions,
            dialect: Dialect::GnuLike,
            default_outputs: defaults,
            compile_commands: Vec::<CompileCommand>::new(),
            standard_violations: Vec::new(),
            standard_compat_violations: Vec::new(),
            planned_packages: BTreeSet::default(),
        }
    }

    /// Render with a fixed `cabin` runner path so the syntax-check rule
    /// is deterministic across hosts (the real path comes from
    /// `std::env::current_exe()` at build time).
    fn render(graph: &BuildGraph) -> Result<String, NinjaError> {
        render_build_ninja(graph, Path::new("/opt/cabin/bin/cabin"))
    }

    #[test]
    fn renders_all_three_rule_kinds() {
        let body = render(&graph_with(
            vec![compile_action(), archive_action(), link_action()],
            vec![Utf8PathBuf::from("/abs/build/hello")],
        ))
        .unwrap();
        // Two compile rules - one per language - plus archive
        // and the language-neutral link rule.  The link rule name
        // does not encode a language because the link driver is
        // selected per target by the planner and threaded into
        // the action's command, not into the rule name.
        assert!(body.contains("rule c_compile"));
        assert!(body.contains("rule cxx_compile"));
        assert!(body.contains("rule cxx_archive"));
        assert!(body.contains("rule link_executable"));
    }

    #[test]
    fn includes_default_targets() {
        let body = render(&graph_with(
            vec![compile_action()],
            vec![Utf8PathBuf::from("/abs/build/main.o")],
        ))
        .unwrap();
        assert!(body.contains("default /abs/build/main.o"));
    }

    #[test]
    fn compile_edge_carries_depfile() {
        let body = render(&graph_with(vec![compile_action()], vec![])).unwrap();
        assert!(body.contains("build /abs/build/main.o: cxx_compile /abs/src/main.cc"));
        assert!(body.contains("depfile = /abs/build/main.o.d"));
        assert!(body.contains("deps = gcc"));
    }

    /// The `assemble` rule exists only when a graph assembles, and its
    /// header tracking follows the dialect: a GNU depfile, nothing for
    /// `ml64`.
    #[test]
    fn assemble_rule_tracks_headers_on_gnu_only() {
        let assemble = BuildAction::Assemble(cabin_driver::AssembleAction {
            assembler: Utf8PathBuf::from("/usr/bin/g++"),
            target: None,
            sysroot: None,
            source: Utf8PathBuf::from("/abs/src/jump.S"),
            object: Utf8PathBuf::from("/abs/build/jump.o"),
            depfile: Some(Utf8PathBuf::from("/abs/build/jump.o.d")),
            include_dirs: vec![],
            defines: vec![],
            description: "AS /abs/build/jump.o".into(),
        });
        let gnu = render(&graph_with(vec![assemble.clone()], vec![])).unwrap();
        assert!(
            gnu.contains(
                "rule assemble\n  command = $command\n  depfile = $depfile\n  deps = gcc\n"
            )
        );
        assert!(gnu.contains("build /abs/build/jump.o: assemble /abs/src/jump.S"));
        assert!(gnu.contains("depfile = /abs/build/jump.o.d"));
        let msvc = render(&msvc_graph_with(vec![assemble])).unwrap();
        assert!(msvc.contains("rule assemble\n  command = $command\n  description"));
        assert!(!msvc.contains("depfile = /abs/build/jump.o.d"));
        let plain = render(&graph_with(vec![compile_action()], vec![])).unwrap();
        assert!(!plain.contains("rule assemble"));
    }

    /// Build a graph that renders for the MSVC dialect.
    fn msvc_graph_with(actions: Vec<BuildAction>) -> BuildGraph {
        BuildGraph {
            actions,
            dialect: Dialect::Msvc,
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
            standard_violations: Vec::new(),
            standard_compat_violations: Vec::new(),
            planned_packages: BTreeSet::default(),
        }
    }

    #[test]
    fn msvc_dialect_uses_showincludes_deps_not_depfile() {
        let body = render(&msvc_graph_with(vec![compile_action()])).unwrap();
        // MSVC compiles discover headers through `cl /showIncludes`,
        // parsed by Ninja's `deps = msvc` + `msvc_deps_prefix` - never a
        // Makefile depfile or `deps = gcc`.
        assert!(body.contains("deps = msvc"));
        assert!(body.contains("msvc_deps_prefix = Note: including file:"));
        assert!(!body.contains("deps = gcc"));
        assert!(!body.contains("depfile ="));
        assert!(body.contains("/showIncludes"));
    }

    /// The host-independent check command the rule must carry: the
    /// `cabin stamp` witness writer (no shell, no `cfg!(windows)` split),
    /// spelled with the fixed test runner from [`render`].  The runner
    /// path has no shell metacharacters, so it is unquoted on both POSIX
    /// and Windows.
    fn expected_check_command() -> &'static str {
        "command = /opt/cabin/bin/cabin stamp $out -- $checkcmd"
    }

    #[test]
    fn msvc_check_rule_uses_zs_with_shellfree_stamp() {
        let body = render(&msvc_graph_with(vec![syntax_check_action()])).unwrap();
        // The compile content is MSVC-specific (`/Zs`), but the stamp
        // runner is the same host-independent `cabin stamp` invocation
        // for either dialect.
        assert!(body.contains("/Zs"));
        assert!(body.contains(expected_check_command()));
    }

    #[test]
    fn check_runner_path_is_shell_quoted_and_ninja_escaped() {
        // Adversarial runner path: a `$` plus a space.  The old quoting
        // wrapped the token in *double* quotes, so POSIX `/bin/sh` would
        // expand `$h` and invoke the wrong path.  The runner must instead
        // be quoted with the platform's real argv quoting (so the shell
        // treats `$` as a literal) and then `$`-escaped for Ninja, while
        // `$out` / `$checkcmd` stay raw Ninja variables.
        let line = check_command_line(Path::new("/tmp/ca$h dir/cabin")).unwrap();
        // `$out` and `$checkcmd` survive verbatim as single-`$` vars.
        assert!(line.ends_with(" stamp $out -- $checkcmd"), "{line}");
        // The runner path's literal `$` is doubled for Ninja (Ninja then
        // hands a single `$` to the shell) - never a bare `$` that Ninja
        // would read as a variable reference.
        assert!(line.contains("ca$$h"), "{line}");
        // POSIX: `shlex` single-quotes the whole token so `/bin/sh`
        // performs no `$` / backtick expansion on it.
        #[cfg(unix)]
        assert!(line.starts_with("'/tmp/ca$$h dir/cabin'"), "{line}");
    }

    #[test]
    fn link_edge_uses_link_rule() {
        let body = render(&graph_with(vec![link_action()], vec![])).unwrap();
        assert!(body.contains("build /abs/build/hello: link_executable /abs/build/main.o"));
    }

    #[test]
    fn archive_edge_uses_archive_rule() {
        let body = render(&graph_with(vec![archive_action()], vec![])).unwrap();
        assert!(body.contains("build /abs/build/libfoo.a: cxx_archive /abs/build/main.o"));
        assert!(
            body.contains("command = /opt/cabin/bin/cabin recreate $out -- $archivecmd\n"),
            "{body}"
        );
        let msvc = render(&msvc_graph_with(vec![archive_action()])).unwrap();
        assert!(
            msvc.contains("rule cxx_archive\n  command = $archivecmd\n"),
            "{msvc}"
        );
    }

    #[test]
    fn renders_syntax_check_rule_and_edge() {
        let body = render(&graph_with(vec![syntax_check_action()], vec![])).unwrap();
        // The check rule stamps the output after a successful syntax-only
        // compile (via the shell-free `cabin stamp` witness writer) so
        // Ninja records completion, and keeps depfile + `deps = gcc` for
        // header-edit incrementality.
        assert!(body.contains("rule cxx_check"));
        assert!(body.contains(expected_check_command()));
        assert!(body.contains("build /abs/build/main.o.check: cxx_check /abs/src/main.cc"));
        // The argv binds to `$checkcmd` (not `command`) so the rule's
        // stamp tail is not shadowed by the edge binding.
        assert!(body.contains("checkcmd = /usr/bin/g++"));
        assert!(body.contains("-fsyntax-only"));
        assert!(body.contains("depfile = /abs/build/main.o.d"));
        // The C variant rule is always emitted alongside the C++ one.
        assert!(body.contains("rule c_check"));
    }

    #[test]
    fn escape_path_handles_specials() {
        assert_eq!(escape_path("simple/path").unwrap(), "simple/path");
        assert_eq!(escape_path("with space").unwrap(), "with$ space");
        assert_eq!(escape_path("with$dollar").unwrap(), "with$$dollar");
        assert_eq!(escape_path("foo:bar").unwrap(), "foo$:bar");
        assert_eq!(escape_path("pi|pe").unwrap(), "pi${pipe}pe");
        assert!(escape_path("with\nnewline").is_err());
        assert!(escape_path("with\rcarriage-return").is_err());
    }

    #[test]
    fn escape_value_only_doubles_dollars() {
        assert_eq!(escape_value("hello world").unwrap(), "hello world");
        assert_eq!(escape_value("$VAR").unwrap(), "$$VAR");
    }

    #[test]
    fn escape_value_rejects_newlines_and_carriage_returns() {
        // A literal newline or CR inside a value would terminate the
        // Ninja variable line and let the next-line content be
        // re-interpreted as a fresh top-level declaration.  Reject so
        // a manifest-controlled string cannot smuggle in `rule` or
        // `command` overrides.
        assert!(matches!(
            escape_value("foo\nbar"),
            Err(NinjaError::ValueHasNewline(_))
        ));
        assert!(matches!(
            escape_value("foo\rbar"),
            Err(NinjaError::ValueHasNewline(_))
        ));
    }

    #[test]
    fn newline_in_description_aborts_render() {
        let act = compile_with(|c| c.description = "CXX foo.o\n  rule injected".into());
        let err = render(&graph_with(vec![act], vec![])).unwrap_err();
        assert!(
            matches!(err, NinjaError::ValueHasNewline(_)),
            "expected ValueHasNewline, got {err:?}"
        );
    }

    #[test]
    fn newline_in_command_aborts_render() {
        // The newline rides in on a compile flag, which lowering copies
        // verbatim into the command argv.
        let act = compile_with(|c| {
            c.arguments
                .extra_flags
                .push("payload\n  command = /bin/rm".into());
        });
        let err = render(&graph_with(vec![act], vec![])).unwrap_err();
        assert!(
            matches!(err, NinjaError::ValueHasNewline(_)),
            "expected ValueHasNewline, got {err:?}"
        );
    }

    #[test]
    fn shell_join_passes_safe_args_through() {
        // shlex emits POSIX-safe tokens (letters, digits, and a
        // narrow punctuation set including `-`, `.`, `/`, `+`)
        // verbatim.  Args within that set must not gain any
        // quoting.
        assert_eq!(
            shell_join(&["simple".into(), "/abs/path/foo.cc".into(), "-c".into()]).unwrap(),
            "simple /abs/path/foo.cc -c",
        );
    }

    #[test]
    fn shell_join_quotes_argument_with_space() {
        let joined = shell_join(&["with space".into()]).unwrap();
        // Round-trip back through `shlex::split` recovers the
        // exact argv, regardless of whether the formatter chose
        // single or double quotes.
        assert_eq!(shlex::split(&joined).unwrap(), vec!["with space"]);
    }

    #[test]
    fn shell_join_quotes_argument_with_dollar_or_metacharacters() {
        for arg in ["$VAR", "a;b", "a|b", "a&b", "*.cc", "`echo`"] {
            let joined = shell_join(&[arg.into()]).unwrap();
            assert_eq!(
                shlex::split(&joined).unwrap(),
                vec![arg.to_owned()],
                "{arg:?} must round-trip through shell quoting",
            );
        }
    }

    #[test]
    fn shell_join_quotes_argument_with_single_quote() {
        let joined = shell_join(&["it's".into()]).unwrap();
        assert_eq!(shlex::split(&joined).unwrap(), vec!["it's"]);
    }

    #[test]
    fn shell_join_handles_empty_argument() {
        let joined = shell_join(&[String::new()]).unwrap();
        assert_eq!(shlex::split(&joined).unwrap(), vec![""]);
    }

    #[test]
    fn shell_join_rejects_nul_byte() {
        let err = shell_join(&["a\0b".into()]).unwrap_err();
        assert!(
            matches!(err, NinjaError::UnquotableArgument(ref s) if s == "a\0b"),
            "expected UnquotableArgument(a\\0b), got {err:?}",
        );
    }

    // ---- Windows command-line quoting (CommandLineToArgvW). ----
    // These exercise the pure joiner directly so the Windows quoting is
    // validated on every host, including non-Windows runners.

    #[test]
    fn windows_join_leaves_backslash_paths_and_flags_verbatim() {
        // The common MSVC argv: a bare compiler name, slash flags, and
        // backslash-laden object/source paths - none contain a space,
        // tab, or quote, so all pass through untouched. (POSIX `shlex`
        // would single-quote the backslash paths, which Windows
        // `CreateProcess` cannot launch - the whole reason this exists.)
        let joined = windows_join(&[
            "cl.exe".into(),
            "/nologo".into(),
            "/c".into(),
            r"/TpC:\build\src\main.cc".into(),
            r"/FoC:\build\obj\main.obj".into(),
        ])
        .unwrap();
        assert_eq!(
            joined,
            r"cl.exe /nologo /c /TpC:\build\src\main.cc /FoC:\build\obj\main.obj"
        );
    }

    #[test]
    fn windows_join_double_quotes_compiler_path_with_spaces() {
        // The exact failure the quoter fixes: a PATH-resolved cl under
        // `Program Files`.  Backslashes that don't precede a quote stay
        // single; the argument is wrapped so `CreateProcess` reads the
        // whole path as argv[0].
        let joined = windows_join(&[
            r"C:\Program Files\Microsoft Visual Studio\VC\cl.exe".into(),
            "/nologo".into(),
        ])
        .unwrap();
        assert_eq!(
            joined,
            "\"C:\\Program Files\\Microsoft Visual Studio\\VC\\cl.exe\" /nologo"
        );
    }

    #[test]
    fn windows_join_quotes_empty_argument() {
        // An empty argv element must survive as an empty token, which
        // requires explicit quotes.
        assert_eq!(windows_join(&[String::new()]).unwrap(), "\"\"");
    }

    #[test]
    fn windows_join_rejects_nul_byte() {
        let err = windows_join(&["a\0b".into()]).unwrap_err();
        assert!(
            matches!(err, NinjaError::UnquotableArgument(ref s) if s == "a\0b"),
            "expected UnquotableArgument(a\\0b), got {err:?}",
        );
    }

    #[test]
    fn render_includes_full_command_in_value() {
        let body = render(&graph_with(vec![compile_action()], vec![])).unwrap();
        // The build edge's `command = ` line round-trips back
        // through POSIX shell-style splitting to the exact argv
        // the action declared.  Tying the assertion to the
        // round-trip rather than a fixed quoted form keeps the
        // test stable when shlex tweaks its formatter.  The
        // Rule-template `command =` lines reference Ninja variables
        // (`$command`, `$checkcmd`, `$out`); the edge's command is the
        // literal argv with no `$`, so pick the line without one.
        let command_line = body
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("command = "))
            .find(|l| !l.contains('$'))
            .expect("edge command = line present");
        let split = shlex::split(command_line).expect("command must round-trip");
        // The edge carries the *lowered* GNU/Clang argv, including the
        // `-MD -MF <depfile>` dependency block the writer's lowering
        // injects for a depfile-bearing compile.
        assert_eq!(
            split,
            vec![
                "/usr/bin/g++",
                "-std=c++17",
                "-O0",
                "-MD",
                "-MF",
                "/abs/build/main.o.d",
                "-c",
                "/abs/src/main.cc",
                "-o",
                "/abs/build/main.o",
            ],
        );
    }

    #[test]
    fn render_propagates_unquotable_argument_error() {
        let act = compile_with(|c| c.arguments.extra_flags.push("evil\0arg".into()));
        let err = render(&graph_with(vec![act], vec![])).unwrap_err();
        assert!(
            matches!(err, NinjaError::UnquotableArgument(_)),
            "expected UnquotableArgument, got {err:?}",
        );
    }

    #[test]
    fn write_build_ninja_creates_file_with_rendered_body() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("build.ninja");
        let graph = graph_with(
            vec![compile_action()],
            vec![Utf8PathBuf::from("/abs/build/main.o")],
        );
        write_build_ninja(&path, &graph, Path::new("/opt/cabin/bin/cabin"), "{}").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, render(&graph).unwrap());
    }

    #[test]
    fn write_build_ninja_replaces_existing_contents() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("build.ninja");
        std::fs::write(&path, "stale\n").unwrap();
        let graph = graph_with(vec![compile_action()], vec![]);
        write_build_ninja(&path, &graph, Path::new("/opt/cabin/bin/cabin"), "{}").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, render(&graph).unwrap());
    }

    #[test]
    fn every_edge_depends_on_the_toolchain_stamp() {
        let c_compile = compile_with(|c| {
            c.standard = cabin_core::LanguageStandard::C(cabin_core::CStandard::C11);
            c.source = Utf8PathBuf::from("/abs/src/util.c");
            c.object = Utf8PathBuf::from("/abs/build/util.o");
        });
        let assemble = BuildAction::Assemble(cabin_driver::AssembleAction {
            assembler: Utf8PathBuf::from("/usr/bin/g++"),
            target: None,
            sysroot: None,
            source: Utf8PathBuf::from("/abs/src/jump.S"),
            object: Utf8PathBuf::from("/abs/build/jump.o"),
            depfile: None,
            include_dirs: vec![],
            defines: vec![],
            description: "AS /abs/build/jump.o".into(),
        });
        let graph = graph_with(
            vec![
                compile_action(),
                c_compile,
                syntax_check_action(),
                assemble,
                archive_action(),
                link_action(),
            ],
            vec![],
        );
        let body = render(&graph).unwrap();
        let edges: Vec<&str> = body.lines().filter(|l| l.starts_with("build ")).collect();
        assert_eq!(edges.len(), 6, "{body}");
        assert!(body.contains(": c_compile /abs/src/util.c"), "{body}");
        for edge in edges {
            assert!(edge.ends_with(" | cabin-toolchain.json"), "{edge}");
        }
    }

    #[test]
    fn toolchain_stamp_is_rewritten_only_when_the_identity_changes() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("build.ninja");
        let stamp = dir.path().join(TOOLCHAIN_STAMP);
        let graph = graph_with(vec![compile_action()], vec![]);
        let runner = Path::new("/opt/cabin/bin/cabin");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let modified = || std::fs::metadata(&stamp).unwrap().modified().unwrap();

        write_build_ninja(&path, &graph, runner, "gcc 13").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stamp)
            .unwrap()
            .set_modified(old)
            .unwrap();
        write_build_ninja(&path, &graph, runner, "gcc 13").unwrap();
        assert_eq!(modified(), old);

        write_build_ninja(&path, &graph, runner, "gcc 14").unwrap();
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), "gcc 14");
        assert!(modified() > old);
    }

    #[test]
    fn write_build_ninja_reports_destination_when_parent_directory_missing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let missing_parent = dir.path().join("nonexistent").join("build.ninja");
        let graph = graph_with(vec![compile_action()], vec![]);
        let err = write_build_ninja(
            &missing_parent,
            &graph,
            Path::new("/opt/cabin/bin/cabin"),
            "{}",
        )
        .unwrap_err();
        match err {
            NinjaError::Io { path, .. } => assert_eq!(path, missing_parent),
            other => panic!("expected NinjaError::Io, got {other:?}"),
        }
    }

    #[test]
    fn unchanged_build_ninja_keeps_its_modification_time() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("build.ninja");
        let runner = Path::new("/opt/cabin/bin/cabin");
        let graph = graph_with(vec![compile_action()], vec![]);
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let modified = || std::fs::metadata(&path).unwrap().modified().unwrap();

        assert!(write_build_ninja(&path, &graph, runner, "{}").unwrap());
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!write_build_ninja(&path, &graph, runner, "{}").unwrap());
        assert_eq!(modified(), old);

        let changed = graph_with(vec![compile_action(), link_action()], vec![]);
        assert!(write_build_ninja(&path, &changed, runner, "{}").unwrap());
        assert!(modified() > old);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            render(&changed).unwrap()
        );
    }

    #[test]
    fn pipe_in_a_path_expands_from_the_file_level_variable() {
        let body = render(&graph_with(vec![], vec![Utf8PathBuf::from("/abs/a|b")])).unwrap();
        assert!(body.contains("\npipe = |\n"), "{body}");
        assert!(body.contains("default /abs/a${pipe}b\n"), "{body}");
    }

    #[test]
    fn depfile_ninja_cannot_read_is_reported_through_cabin_depfile() {
        let plain = render(&graph_with(vec![compile_action()], vec![])).unwrap();
        assert!(!plain.contains("deps = msvc"), "{plain}");
        let edges = [
            compile_with(|c| c.source = Utf8PathBuf::from("/abs/a;b/main.cc")),
            compile_with(|c| c.object = Utf8PathBuf::from("/abs/it's/main.o")),
            compile_with(|c| c.arguments.include_dirs = vec!["/abs/a|b/include".into()]),
            compile_with(|c| c.arguments.system_include_dirs = vec!["/abs/a*b".into()]),
            compile_with(|c| c.arguments.extra_flags = vec!["-I/abs/a&b".into()]),
            compile_with(|c| {
                c.sysroot = Some(cabin_core::Sysroot {
                    path: "/abs/a;b/sdk".into(),
                    apple: false,
                });
            }),
            BuildAction::Assemble(cabin_driver::AssembleAction {
                assembler: Utf8PathBuf::from("/usr/bin/g++"),
                target: None,
                sysroot: Some(cabin_core::Sysroot {
                    path: "/abs/a;b/sdk".into(),
                    apple: false,
                }),
                source: Utf8PathBuf::from("/abs/src/main.S"),
                object: Utf8PathBuf::from("/abs/build/main.o"),
                depfile: Some(Utf8PathBuf::from("/abs/build/main.o.d")),
                include_dirs: vec![],
                defines: vec![],
                description: "AS /abs/build/main.o".into(),
            }),
        ];
        for edge in edges {
            let body = render(&graph_with(vec![edge], vec![])).unwrap();
            assert!(
                body.contains("  command = /opt/cabin/bin/cabin depfile /abs/build/main.o.d -- "),
                "{body}"
            );
            assert!(
                body.contains(
                    "  depfile = /abs/build/main.o.d\n  deps = msvc\n  msvc_deps_prefix = cabin-depfile:\n"
                ),
                "{body}"
            );
        }
        // A define never names a file, so it keeps the depfile as is.
        let defines = [
            compile_with(|c| c.arguments.defines = vec!["MSG=\"it's\"".into()]),
            compile_with(|c| c.arguments.extra_flags = vec!["-DMSG=\"it's\"".into()]),
        ];
        for define in defines {
            let body = render(&graph_with(vec![define], vec![])).unwrap();
            assert!(!body.contains("deps = msvc"), "{body}");
        }
    }

    #[test]
    fn long_archive_and_link_commands_pass_arguments_in_a_response_file() {
        let archive = BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("/usr/bin/ar"),
            output: Utf8PathBuf::from("/abs/build/libfoo.a"),
            inputs: vec![
                Utf8PathBuf::from("/abs/build/a b.o"),
                Utf8PathBuf::from("/abs/build/it's\\\"$x.o"),
            ],
            description: "AR /abs/build/libfoo.a".into(),
        });
        let graph = graph_with(vec![archive, link_action()], vec![]);
        assert!(!render(&graph).unwrap().contains("rspfile"));

        let body = render_with_inline_limit(&graph, Path::new("/opt/cabin/bin/cabin"), 0).unwrap();
        assert!(
            body.contains(
                "  archivecmd = /usr/bin/ar @/abs/build/libfoo.a+.rsp\n  \
                 rspfile = /abs/build/libfoo.a+.rsp\n  \
                 rspfile_content = crs /abs/build/libfoo.a /abs/build/a\\ b.o \
                 /abs/build/it\\'s\\\\\\\"$$x.o\n"
            ),
            "{body}"
        );
        assert!(
            body.contains(
                "  command = /usr/bin/g++ @/abs/build/hello+.rsp\n  \
                 rspfile = /abs/build/hello+.rsp\n  \
                 rspfile_content = /abs/build/main.o -o /abs/build/hello\n"
            ),
            "{body}"
        );
    }

    #[test]
    fn msvc_response_files_list_inputs_one_per_line_after_a_byte_order_mark() {
        let archive = BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("C:/VC/lib.exe"),
            output: Utf8PathBuf::from("C:/b/foo.lib"),
            inputs: vec![
                Utf8PathBuf::from("C:/b/a b.obj"),
                Utf8PathBuf::from("C:/b/c.obj"),
            ],
            description: "LIB C:/b/foo.lib".into(),
        });
        let link = BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("C:/VC/cl.exe"),
            target: None,
            sysroot: None,
            output: Utf8PathBuf::from("C:/b/$app.exe"),
            inputs: vec![
                Utf8PathBuf::from("C:/b/main.obj"),
                Utf8PathBuf::from("C:/b/foo.lib"),
            ],
            arguments: vec!["/DEBUG".into()],
            link_libs: vec!["ws2_32".into()],
            frameworks: vec![],
            description: "LINK C:/b/$app.exe".into(),
        });
        let body = render_with_inline_limit(
            &msvc_graph_with(vec![archive, link]),
            Path::new("/opt/cabin/bin/cabin"),
            0,
        )
        .unwrap();
        // Ninja quotes `$in_newline` for the host, which MSVC tools read
        // only on Windows; `/link` and its options share the last line.
        let expected: &[&str] = if cfg!(windows) {
            &[
                "  rspfile = $rsp_file\n  rspfile_content = $rsp_head $in_newline $rsp_tail\n",
                "  rsp_file = C:/b/foo.lib+.rsp\n  rsp_head = \u{feff}/nologo /OUT:C:/b/foo.lib\n  \
                 rsp_tail = \n",
                "  rsp_file = C:/b/$$app.exe+.rsp\n  rsp_head = \u{feff}/nologo\n  rsp_tail = ws2_32.lib /FeC:/b/$$app.exe /link \
                 /SUBSYSTEM:CONSOLE /DEBUG\n",
            ]
        } else {
            &[
                "  rspfile_content = \u{feff}/nologo /OUT:C:/b/foo.lib \"C:/b/a b.obj\" C:/b/c.obj\n",
                "  rspfile_content = \u{feff}/nologo C:/b/main.obj C:/b/foo.lib ws2_32.lib \
                 /FeC:/b/$$app.exe /link /SUBSYSTEM:CONSOLE /DEBUG\n",
            ]
        };
        for line in expected {
            assert!(body.contains(line), "{body}");
        }
    }
}
