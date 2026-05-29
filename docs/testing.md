# Testing with `cabin test`

`cabin test` is a small wrapper around the existing build
pipeline: it builds the selected `cpp_test` targets, runs each
linked executable in deterministic order, and reports a summary.

It is intentionally not a testing framework. It does no test-
case discovery inside binaries and parses no framework-specific
output. The unit of execution is the entire test executable —
its exit status decides pass / fail.

## Declaring a test target

```toml
[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]
```

The full `cpp_test` syntax is documented in
[`docs/targets.md`](targets.md).

## Running tests

```sh
cabin test                           # every cpp_test in the default selection
cabin test --workspace               # every cpp_test in every workspace member
cabin test -p demo                   # only demo's tests
cabin test --release                 # compile with the release profile
cabin test --features simd          # forward features to the test build
```

`cabin test` does not offer a single-test selector flag.
Narrow the run by narrowing the package selection (`--package`
/ `--workspace` / `--exclude`).

`cabin test` shares its core flags with `cabin build`:
`--profile`, `--release`, `--features`, `--no-default-features`,
`--all-features`, `--locked`, `--frozen`, `--no-patches`, the
toolchain
overrides (`--cc` / `--cxx` / `--ar` / `--compiler-wrapper`),
the workspace-selection bundle (`--workspace` / `-p` /
`--default-members` / `--exclude`), and the index/cache
locations (`--index-path` / `--index-url` / `--cache-dir`).

`--allow-no-tests` opts the user out of the empty-selection
error: by default, `cabin test` exits with an error when the
selected packages declare no `cpp_test` target, so CI does not
silently pass when tests have not been added yet. Pass
`--allow-no-tests` for cases where empty is expected.

## Output and exit status

For each test executable Cabin prints:

```
running test <pkg>:<target>
... (the executable's stdout, prefixed by a "stdout:" header)
... (the executable's stderr, prefixed by a "stderr:" header)
test <pkg>:<target> ... ok
```

A failed test exits non-zero; Cabin records the exit code and
writes:

```
test <pkg>:<target> ... FAILED (exit 17)
```

If any test fails, `cabin test` itself exits non-zero and
writes the rendered test summary to stdout, followed by the
top-level error on stderr:

```
test result: FAILED. P passed; F failed (of N)
error: test failures: F of N test executables failed
```

A test killed by a signal renders as `FAILED (terminated by
signal)`.

## Working directory and environment

Each test executable runs with its working directory set to the
**owning package's manifest directory**. Tests that read fixture
data from paths relative to the package root therefore see the
same files in CI and on a developer's machine.

The environment the test inherits is the same as `cabin test`'s
own — Cabin does not set test-framework-specific variables. If
you need a particular variable, set it in the parent process.

## Determinism

Test ordering is `(package_name, target_name)` ascending,
regardless of the order targets appear in `cabin.toml`. The
runner is sequential: each test runs to completion before the
next starts.

## Dev-dependencies

`cabin test` activates `[dev-dependencies]` for the selected
packages as real graph edges. A test target may therefore depend
on packages declared only under `[dev-dependencies]`:

```toml
[dependencies]
demo = { path = "../demo" }

[dev-dependencies]
gtest = "^1.14"

[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo", "gtest"]
```

`cabin build` continues to ignore `[dev-dependencies]`, so
ordinary production builds remain unaffected. Dev-dep
activation never propagates: a transitive package's own
`[dev-dependencies]` stay declaration-only even under
`cabin test`.

## Lockfile behavior

`--locked` and `--frozen` apply to `cabin test` exactly as they
do to `cabin build`:

- `--locked` requires the lockfile to satisfy the resolver's
  pick (and the patch / source-replacement state to match);
- `--frozen` additionally forbids state-writing side effects
  (cache population, lockfile mutation).

Because `cabin test` includes `[dev-dependencies]` in
resolution, projects that have never run `cabin test` may need
one un-`--locked` invocation to add dev-deps to the lockfile.

## What `cabin test` is *not*

- It does not run examples. `cpp_example` targets are not
  selectable from the command line — they reach the build
  graph only as transitive deps of another selected target.
- It does not parse GoogleTest / Catch2 / doctest output, nor
  emit XML / JUnit reports.
- It does not provide test filtering inside an executable, and
  there is no single-test selector flag; narrow the run by
  narrowing the package selection (`-p <package>` /
  `--workspace` / `--exclude`).

## Test portability rules

These rules apply to every test that lives under `cabin`,
the planner, the toolchain layer, and any future crate that
exercises the build / test pipeline. They are normative —
adding a test that violates them is a review-blocking change.

### 1. Tool gating must be explicit

Tests that compile real C or C++ sources gate on a tool
availability helper from `cabin/tests/cli.rs`:

| Helper                              | Required tools                          |
| ----------------------------------- | --------------------------------------- |
| `ninja_available`                   | `ninja`                                 |
| `c_compiler_available`              | one of `cc` / `clang` / `gcc`           |
| `cxx_compiler_available`            | one of `c++` / `clang++` / `g++`        |
| `build_tools_available`             | `ninja` + a C++ compiler                |
| `c_and_cxx_build_tools_available`   | `ninja` + a C compiler + a C++ compiler |

A test that compiles `.c` sources **must** gate on
`c_and_cxx_build_tools_available`, not on
`build_tools_available`. Without that, the test would silently
fall through to a planner-time `MissingCCompiler` error on a
runner that has only `c++` / `clang++` / `g++` installed.

Pure data-model tests that never spawn a compiler (planner
unit tests, lockfile / metadata round-trips) do not need to
gate on tool availability.

The CLI suite also has external-tool smoke tests for the tools
Cabin shells out to directly: `ninja`, `clang-format`,
`run-clang-tidy`, and `pkg-config`. These tests intentionally
fail by default when the real tools are absent, so CI catches a
missing package instead of silently exercising only fake helpers.
Set `CABIN_SKIP_EXTERNAL_TOOL_TESTS=1` only when you deliberately
want those smoke tests to use the bundled fake-tool binaries.

### 2. Environment isolation

Integration tests use the shared `cabin()` helper, which clears
or pins the read-side environment that commonly affects test
output and tool selection:

- `CABIN_NO_CONFIG`, `CABIN_CONFIG`, `CABIN_CONFIG_HOME`
  (config discovery);
- `CC`, `CXX`, `AR` (toolchain selection);
- `NINJA` (backend lookup);
- `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, `LDFLAGS` (build-flag
  ingestion);
- `CABIN_NET_OFFLINE` (offline override);
- `CABIN_COMPILER_WRAPPER` (compiler-cache wrapper);
- `CABIN_CACHE_DIR` (artifact cache override);
- `CABIN_FMT`, `CABIN_TIDY`, `CABIN_PKG_CONFIG`
  (developer-tool and system-dependency executable overrides);
- standard pkg-config lookup variables (`PKG_CONFIG_PATH`,
  `PKG_CONFIG_LIBDIR`, `PKG_CONFIG_SYSROOT_DIR`);
- terminal-color controls (`NO_COLOR`, `CLICOLOR`,
  `CLICOLOR_FORCE`), while pinning `CABIN_TERM_COLOR=never`.

The helper does not remove every read-side Cabin variable. Tests
whose assertions depend on build directory, job count, or
verbosity must set or remove `CABIN_BUILD_DIR`,
`CABIN_BUILD_JOBS`, `CABIN_TERM_VERBOSE`, and
`CABIN_TERM_QUIET` explicitly.

Tests that intentionally exercise env precedence (e.g. "CXX
env wins over the manifest's `[toolchain]`") opt back in with a
plain `.env(KEY, VALUE)` after `cabin()` returns — `assert_cmd`
applies env mutations in declaration order, so a later
`.env(...)` overrides the earlier `.env_remove(...)`.

The shared `cabin_with_config()` helper in the patches module
keeps the same scrubbing rules but additionally re-enables
config discovery for tests that exercise config files; consult
that module for the documented opt-in pattern.

### 3. No host-specific absolute paths

Integration tests must not use hardcoded host-specific
absolute paths (`/tmp/...`, `/usr/bin/...`, `/this/path/does/not/exist/...`).
Construct paths under `assert_fs::TempDir` instead —
`dir.child("missing-cc").path()` is the canonical
"non-existent path" idiom for tests that need a path that will
fail to resolve.

The planner unit tests use fake POSIX-shaped paths (`/abs/proj`,
`/usr/bin/g++`) but never *execute* them — those tests are pure
data-model assertions on the build graph that happens to take a
`PathBuf` as input. That is the only place absolute fake paths
are acceptable.

### 4. Driver-name assertions

Tests that need to verify the link-driver pick should prefer
*structural* assertions over driver-name substring matching:

- assert on the rule name (`c_compile`, `cxx_compile`,
  `link_executable`) by walking generated `build.ninja` edges
  rather than grepping for `c++` / `g++` / `clang++`;
- when the test must check the actual driver path, ask
  `cabin metadata --format json` for the resolved
  `toolchain.tools.cc.path` / `toolchain.tools.cxx.path` and
  compare the link command's first argument against that.

Substring checks are acceptable as a *belt-and-suspenders*
sanity check, never as the primary assertion.

### 5. Generated output normalization

Golden / fixture tests compare generated output (Ninja file,
metadata JSON, package archive contents) against a snapshot.
Output that contains absolute paths must be normalized before
comparison so the snapshot does not bake in the developer's
temp-directory prefix. The lockfile renderer is the canonical
example: every value sorts deterministically, paths are stored
relative to a documented anchor, and the tests assert on
byte-equal output.

### 6. Test filesystem fixtures

Use [`assert_fs`](https://docs.rs/assert_fs) for temporary
filesystem fixtures and filesystem assertions in Rust tests.
The canonical pattern is:

```rust
use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;

let dir = TempDir::new().unwrap();
dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
dir.child("src/main.cc").write_str(MAIN_CC).unwrap();
let out = dir.child("dist");

// Pass `&Path` across the production boundary:
cabin().args(["build", "--manifest-path"])
    .arg(dir.child("cabin.toml").path())
    .arg("--build-dir").arg(out.path())
    .assert().success();

// Predicate-based filesystem assertions:
out.child("dev/build.ninja").assert(predicate::path::is_file());
```

`ChildPath` is a test fixture type — never expose it from
production crates. Pass `child.path()` or `child.to_path_buf()`
across the production API boundary so Cabin's library code
keeps accepting `&Path` / `PathBuf` / `OsStr`.

Keep command execution through the shared `cabin()` helper so
environment isolation remains consistent (see § 2). Pair
`assert_fs` for fixture setup with `assert_cmd` for command
invocation and `predicates` for stdout / stderr / path
assertions.

Normalize absolute temp paths before comparing generated output
against a golden snapshot (see § 5). The fixture path printed
by `assert_fs::TempDir` is a host-specific temp directory and
must not leak into expected text.

## CI portability boundary

The Rust CI job in `.github/workflows/rust.yml` runs on both
`ubuntu-latest` and `macos-latest`. Linux installs
`ninja-build`, `gcc`, and `g++`; macOS installs Ninja and LLVM
through Homebrew and uses the platform Clang drivers. The
`cabin()` env scrubbing and the
`c_and_cxx_build_tools_available` gating keep tests portable
across both runners without silently masking C coverage.
