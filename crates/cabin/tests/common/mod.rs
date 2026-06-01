//! Shared test-support harness for Cabin's integration test
//! binaries (`cli.rs`, `cabin_examples.rs`).
//!
//! This module exists so the `cabin()` command builder — and in
//! particular its environment-scrub list, which is the harness's
//! reproducibility contract — lives in exactly one place. Each test
//! binary declares `mod common;` and the file is compiled as a
//! private submodule of that binary (Cargo does not treat
//! `tests/common/mod.rs` as its own test target).

use std::process::Stdio;

use assert_cmd::Command;

/// `Command` builder pointed at the test-built `cabin` binary, with
/// the full environment scrub every integration test relies on so a
/// test only ever sees the inputs it sets explicitly.
///
/// The env-scrub list below is the harness's correctness contract:
/// toolchain / wrapper / build-flag / cache / pkg-config / color
/// leaks are the most common reason a test that passes locally fails
/// in CI (or vice versa). Tests that exercise env precedence opt back
/// in by calling `.env(KEY, VALUE)` *after* this helper — `assert_cmd`
/// applies env mutations in declaration order, so a later `.env(...)`
/// overrides this `.env_remove(...)`.
pub fn cabin() -> Command {
    let mut cmd = Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
    // Isolate every integration test from a developer's own
    // `~/.config/cabin/config.toml`. Tests that exercise config
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
        // and merged into per-package compile flags. Strip it
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
        // tests that observe cache state. Tests that exercise
        // foundation-port HTTP traffic pass an explicit
        // `--cache-dir` instead.
        "CABIN_CACHE_HOME",
        "CABIN_FMT",
        "CABIN_TIDY",
        // System dependency probing reads `CABIN_PKG_CONFIG`
        // and Cabin passes the rest of the standard pkg-config
        // environment through to its child process. Strip every
        // one of them so an integration test sees only the
        // overrides it sets explicitly.
        "CABIN_PKG_CONFIG",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
        // termcolor's `Auto` decision honors `NO_COLOR`,
        // `CLICOLOR`, and `CLICOLOR_FORCE`. Strip them so a
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
    // ultimately resolves to a terminal. Tests that exercise
    // the color contract (in `mod color_control`) override
    // this with `--color` or `CABIN_TERM_COLOR` explicitly;
    // assert_cmd applies env mutations in declaration order so
    // a later `.env(...)` overrides this default.
    cmd.env("CABIN_TERM_COLOR", "never");
    pin_test_cache_home(&mut cmd);
    cmd
}

/// Pin `CABIN_CACHE_HOME` to a deterministic temp path. Tests
/// routinely strip `HOME` for config isolation, which would
/// otherwise leave the user-global cache fallback
/// (`$CABIN_CACHE_HOME` ▶ `$XDG_CACHE_HOME/cabin` ▶
/// `$HOME/.cache/cabin`) with nothing to resolve to in CI,
/// where `XDG_CACHE_HOME` is unset. The cache is
/// content-addressed, so parallel writers to the same path are
/// safe. Tests that observe cache state still pass an explicit
/// `--cache-dir`, which takes precedence.
pub fn pin_test_cache_home(cmd: &mut Command) {
    cmd.env(
        "CABIN_CACHE_HOME",
        std::env::temp_dir().join("cabin-tests-cache-home"),
    );
}

pub fn command_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Whether Ninja is available on `PATH`. Cabin invokes Ninja
/// directly for every `cabin build` / `cabin test` integration
/// test that produces real artifacts; tests gate on this
/// helper to skip cleanly on environments without it.
pub fn ninja_available() -> bool {
    command_exists("ninja")
}

/// Whether at least one of Cabin's documented C compiler
/// fallbacks is on `PATH` (`cc` / `clang` / `gcc`). Tests that
/// compile `.c` translation units gate on this helper so they
/// do not silently fall through to a `MissingCCompiler` error
/// at planner time on a system that has only a C++ compiler.
pub fn c_compiler_available() -> bool {
    ["cc", "clang", "gcc"]
        .iter()
        .any(|name| command_exists(name))
}

/// Whether at least one of Cabin's documented C++ compiler
/// fallbacks is on `PATH` (`c++` / `clang++` / `g++`).
pub fn cxx_compiler_available() -> bool {
    ["c++", "clang++", "g++"]
        .iter()
        .any(|name| command_exists(name))
}

/// Whether the integration tests that build C++ targets via
/// real Ninja can run. Use this for tests that link only C++
/// translation units. Tests that touch C must use
/// [`c_and_cxx_build_tools_available`] instead.
pub fn build_tools_available() -> bool {
    ninja_available() && cxx_compiler_available()
}

/// Whether the integration tests that build *both* C/C++
/// targets via real Ninja can run. Required by every test that
/// compiles `.c` sources alongside C++ sources, and by pure-C
/// tests (Cabin still requires a C++ compiler at toolchain
/// resolution time even when only C is built).
pub fn c_and_cxx_build_tools_available() -> bool {
    ninja_available() && c_compiler_available() && cxx_compiler_available()
}
