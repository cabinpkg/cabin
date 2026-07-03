//! Shared test-support harness for Cabin's integration test
//! binaries (`cli.rs`, `cabin_examples.rs`).
//!
//! This module exists so the `cabin()` command builder - and in
//! particular its environment-scrub list, which is the harness's
//! reproducibility contract - lives in exactly one place.  Each test
//! binary declares `mod common;` and the file is compiled as a
//! private submodule of that binary (Cargo does not treat
//! `tests/common/mod.rs` as its own test target).
//!
//! Each test binary uses a different subset of these helpers, so the
//! ones a given binary does not reach are not dead code in any
//! meaningful sense - silence the per-binary `dead_code` lint here.
#![allow(dead_code)]

use assert_cmd::Command;
use cabin_build::Dialect;

mod fake_ports;
mod foundation_port_smoke;
mod port_schema;

#[allow(unused_imports)]
pub use fake_ports::{FakeArchiveServer, FakePortRepo};
#[allow(unused_imports)]
pub use foundation_port_smoke::{
    PortBuildRun, PortCacheLifecycle, run_port_build_then_run, run_port_cache_lifecycle,
};
#[allow(unused_imports)]
pub use port_schema::{
    assert_builtin_port_bundled_and_parses, assert_tar_gz_source, builtin_overlay,
    load_real_port_and_assert_schema,
};

/// File name of the executable built from `stem` in the host's
/// build dialect (`app` on Unix, `app.exe` on Windows).  Single
/// source of truth so path/artifact assertions speak the host's
/// dialect instead of hardcoding the POSIX spelling.
pub fn host_exe(stem: &str) -> String {
    Dialect::host_default().executable_name(stem)
}

/// File name of the static library built from `stem` in the
/// host's build dialect (`libfmt.a` on Unix, `fmt.lib` on
/// Windows).
pub fn host_static_lib(stem: &str) -> String {
    Dialect::host_default().static_library_name(stem)
}

/// Object-file extension (no leading dot) in the host's build
/// dialect (`o` on Unix, `obj` on Windows).
pub fn host_obj_ext() -> &'static str {
    Dialect::host_default().object_extension()
}

/// Rewrite a `/`-separated expected path substring to use the
/// host path separator so substring assertions over `str` output
/// match Windows backslash paths.  Leaves Unix output unchanged.
///
/// Only for `str`-based assertions (`contains` / `ends_with` over
/// `String`).  Paths compared as `&Path` are already separator
/// agnostic and must not be wrapped.
pub fn host_path(unix_relpath: &str) -> String {
    unix_relpath.replace('/', std::path::MAIN_SEPARATOR_STR)
}

/// Extract argv[0] (the program) from a serialized command line.
///
/// Ninja command lines quote a program path that contains spaces,
/// which the Windows MSVC compiler path always does
/// (`"C:\Program Files\...\cl.exe" /nologo ...`).  On Unix the
/// program is an unquoted leading token.  Splitting on whitespace
/// and taking `[0]` breaks on the quoted Windows form, so link
/// tests use this instead.
pub fn program_from_command(cmd: &str) -> String {
    let cmd = cmd.trim_start();
    if let Some(rest) = cmd.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return rest[..end].to_owned();
    }
    cmd.split_whitespace().next().unwrap_or("").to_owned()
}

/// Compiler flag the host dialect emits to select the C++
/// standard (`-std=c++17` on GCC/Clang, `/std:c++17` on MSVC).
pub fn host_std_cxx_flag() -> &'static str {
    match Dialect::host_default() {
        Dialect::GnuLike => "-std=c++17",
        Dialect::Msvc => "/std:c++17",
    }
}

/// Compiler flag the host dialect emits for a release-optimized
/// build (`-O3` on GCC/Clang, `/O2` on MSVC).
pub fn host_release_opt_flag() -> &'static str {
    match Dialect::host_default() {
        Dialect::GnuLike => "-O3",
        Dialect::Msvc => "/O2",
    }
}

/// Compiler flag the host dialect emits for an unoptimized build
/// (`-O0` on GCC/Clang, `/Od` on MSVC).  Used by negative
/// assertions so they stay meaningful on every host.
pub fn host_no_opt_flag() -> &'static str {
    match Dialect::host_default() {
        Dialect::GnuLike => "-O0",
        Dialect::Msvc => "/Od",
    }
}

/// Compiler flag the host dialect emits to define `NDEBUG`
/// (`-DNDEBUG` on GCC/Clang, `/DNDEBUG` on MSVC).
pub fn host_define_ndebug_flag() -> &'static str {
    match Dialect::host_default() {
        Dialect::GnuLike => "-DNDEBUG",
        Dialect::Msvc => "/DNDEBUG",
    }
}

/// Compiler flag the host dialect emits to embed debug info
/// (`-g` on GCC/Clang, `/Z7` on MSVC).
pub fn host_debug_info_flag() -> &'static str {
    match Dialect::host_default() {
        Dialect::GnuLike => "-g",
        Dialect::Msvc => "/Z7",
    }
}

/// Substring of the OS error a host emits when a manifest read
/// targets a directory (`Is a directory` on Unix,
/// `Access is denied` on Windows).  Diagnostics tests assert the
/// real platform string rather than a tautology.
pub fn manifest_dir_read_error() -> &'static str {
    if cfg!(windows) {
        "Access is denied"
    } else {
        "Is a directory"
    }
}

/// `Command` builder pointed at the test-built `cabin` binary, with
/// the full environment scrub every integration test relies on so a
/// test only ever sees the inputs it sets explicitly.
///
/// The env-scrub list below is the harness's correctness contract:
/// toolchain / wrapper / build-flag / cache / pkg-config / color
/// leaks are the most common reason a test that passes locally fails
/// in CI (or vice versa).  Tests that exercise env precedence opt back
/// in by calling `.env(KEY, VALUE)` *after* this helper - `assert_cmd`
/// applies env mutations in declaration order, so a later `.env(...)`
/// overrides this `.env_remove(...)`.
pub fn cabin() -> Command {
    let mut cmd = Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
    // Isolate every integration test from a developer's own
    // `~/.config/cabin/config.toml`.  Tests that exercise config
    // discovery on purpose explicitly re-enable it via
    // `.env_remove("CABIN_NO_CONFIG")` or a custom
    // `CABIN_CONFIG_HOME`.
    cmd.env("CABIN_NO_CONFIG", "1")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME");
    for key in [
        "CC",
        "CXX",
        "AR",
        "NINJA",
        "CFLAGS",
        "CXXFLAGS",
        // CPPFLAGS is read by the build orchestration layer
        // and merged into per-package compile flags.  Strip it
        // so a developer's `CPPFLAGS=-I/opt/...` shell state
        // cannot bleed into golden output or verbose-on-stderr
        // assertions.
        "CPPFLAGS",
        "LDFLAGS",
        "CABIN_NET_OFFLINE",
        "CABIN_COMPILER_WRAPPER",
        "CABIN_CACHE_DIR",
        // `CABIN_CACHE_HOME` redirects the per-user cache home;
        // strip it so a developer's environment can't bleed into
        // tests that observe cache state.  Tests that exercise
        // foundation-port HTTP traffic pass an explicit
        // `--cache-dir` instead.
        "CABIN_CACHE_HOME",
        "CABIN_FMT",
        "CABIN_TIDY",
        // System dependency probing reads `CABIN_PKG_CONFIG`
        // and Cabin passes the rest of the standard pkg-config
        // environment through to its child process.  Strip every
        // one of them so an integration test sees only the
        // overrides it sets explicitly.
        "CABIN_PKG_CONFIG",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
        // termcolor's `Auto` decision honors `NO_COLOR`,
        // `CLICOLOR`, and `CLICOLOR_FORCE`.  Strip them so a
        // developer's shell configuration does not flip the
        // default away from "no color".
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
    ] {
        cmd.env_remove(key);
    }
    // Force the default test binary to emit no color so
    // existing substring-based assertions stay byte-stable
    // regardless of whether the test harness's stderr
    // ultimately resolves to a terminal.  Tests that exercise
    // the color contract (in `mod color_control`) override
    // this with `--color` or `CABIN_TERM_COLOR` explicitly;
    // assert_cmd applies env mutations in declaration order so
    // a later `.env(...)` overrides this default.
    cmd.env("CABIN_TERM_COLOR", "never");
    pin_test_cache_home(&mut cmd);
    cmd
}

/// Pin `CABIN_CACHE_HOME` to a deterministic temp path.  Tests
/// routinely strip `HOME` for config isolation, which would
/// otherwise leave the user-global cache fallback
/// (`$CABIN_CACHE_HOME` ▶ `$XDG_CACHE_HOME/cabin` ▶
/// `$HOME/.cache/cabin`) with nothing to resolve to in CI,
/// where `XDG_CACHE_HOME` is unset.  The cache is
/// content-addressed, so parallel writers to the same path are
/// safe.  Tests that observe cache state still pass an explicit
/// `--cache-dir`, which takes precedence.
pub fn pin_test_cache_home(cmd: &mut Command) {
    cmd.env(
        "CABIN_CACHE_HOME",
        std::env::temp_dir().join("cabin-tests-cache-home"),
    );
}

pub fn command_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

/// Whether Ninja is available on `PATH`.  Cabin invokes Ninja
/// directly for every `cabin build` / `cabin test` integration
/// test that produces real artifacts; tests gate on this
/// helper to skip cleanly on environments without it.
pub fn ninja_available() -> bool {
    command_exists("ninja")
}

/// Whether a C compiler Cabin can drive is available - `cc` /
/// `clang` / `gcc` on `PATH`, or MSVC `cl` on Windows (see
/// [`msvc_cl_available`]).  Tests that compile `.c` translation
/// units gate on this so they do not silently fall through to a
/// `MissingCCompiler` error at planner time on a system that has
/// only a C++ compiler.
pub fn c_compiler_available() -> bool {
    ["cc", "clang", "gcc"]
        .iter()
        .any(|name| command_exists(name))
        || msvc_cl_available()
}

/// Whether a C++ compiler Cabin can drive is available - `c++` /
/// `clang++` / `g++` on `PATH`, or MSVC `cl` on Windows (which
/// compiles both C and C++; see [`msvc_cl_available`]).
pub fn cxx_compiler_available() -> bool {
    ["c++", "clang++", "g++"]
        .iter()
        .any(|name| command_exists(name))
        || msvc_cl_available()
}

/// Whether the MSVC `cl.exe` compiler is usable on Windows -
/// either on `PATH` (an activated Developer environment) or
/// auto-discoverable the same way Cabin finds it (via
/// `find-msvc-tools`). `cl` drives both C and C++, so it counts
/// for both compiler probes.  Always `false` off Windows.
///
/// This mirrors the resolver's `cl` lookup exactly
/// (`resolve.rs`: `search_path("cl")` then `msvc_tool_path("cl")`)
/// so the probe reports availability precisely when the resolver
/// can resolve a compiler. `clang-cl` is intentionally *not*
/// counted: the resolver's fallback list never tries it, so
/// counting it here would let `require_*` pass and then fail
/// later at toolchain resolution.
fn msvc_cl_available() -> bool {
    cfg!(windows) && (command_exists("cl") || cabin_toolchain::msvc::msvc_tool_path("cl").is_some())
}

/// Whether the integration tests that build C++ targets via
/// real Ninja can run.  Use this for tests that link only C++
/// translation units.  Tests that touch C must use
/// [`c_and_cxx_build_tools_available`] instead.
pub fn cxx_build_tools_available() -> bool {
    ninja_available() && cxx_compiler_available()
}

/// Whether the integration tests that build *both* C/C++
/// targets via real Ninja can run.  Required by every test that
/// compiles `.c` sources alongside C++ sources, and by pure-C
/// tests (Cabin still requires a C++ compiler at toolchain
/// resolution time even when only C is built).
pub fn c_and_cxx_build_tools_available() -> bool {
    ninja_available() && c_compiler_available() && cxx_compiler_available()
}

/// Assert that the full C/C++ build toolchain (Ninja + a C
/// compiler + a C++ compiler) is on `PATH`, failing the test if
/// any of it is missing.  Tests that build `.c` sources call this
/// instead of skipping on missing tools, so a host with a broken
/// toolchain reds the suite rather than silently going green.  See
/// [`c_and_cxx_build_tools_available`].
pub fn require_c_and_cxx_build_tools() {
    let mut missing = Vec::new();
    if !ninja_available() {
        missing.push("ninja");
    }
    if !c_compiler_available() {
        missing.push("a C compiler (cc/clang/gcc)");
    }
    if !cxx_compiler_available() {
        missing.push("a C++ compiler (c++/clang++/g++)");
    }
    assert!(
        c_and_cxx_build_tools_available(),
        "C/C++ build tools required for this test are missing on PATH: {}; install them",
        missing.join(", "),
    );
}

/// Assert that the C++ build toolchain (Ninja + a C++ compiler) is
/// available, failing the test if either is missing.  Tests that
/// build C++ targets call this instead of skipping on missing
/// tools, so a host with a broken toolchain reds the suite rather
/// than silently going green.  See [`cxx_build_tools_available`].
pub fn require_cxx_build_tools() {
    let mut missing = Vec::new();
    if !ninja_available() {
        missing.push("ninja");
    }
    if !cxx_compiler_available() {
        missing.push("a C++ compiler (c++/clang++/g++)");
    }
    assert!(
        cxx_build_tools_available(),
        "C++ build tools required for this test are missing on PATH: {}; install them",
        missing.join(", "),
    );
}
