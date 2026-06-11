# Testing with `cabin test`

`cabin test` is a small wrapper around the existing build
pipeline: it builds the selected `test` targets, runs each
linked executable in deterministic order, and reports a summary.

It is intentionally not a testing framework. It does no test-
case discovery inside binaries and parses no framework-specific
output. The unit of execution is the entire test executable —
its exit status decides pass / fail.

## Declaring a test target

```toml
[target.demo_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]
```

The full `test` syntax is documented in
[`docs/targets.md`](targets.md).

## Running tests

```sh
cabin test                           # every test in the default selection
cabin test --workspace               # every test in every workspace member
cabin test -p demo                   # only demo's tests
cabin test --test demo_test          # only the named test target
cabin test --test a --test b         # several named test targets
cabin test --release                 # compile with the release profile
cabin test --features simd          # forward features to the test build
```

`--test <NAME>` runs individual `test` targets, mirroring
`cargo test --test <name>`. The flag may be repeated; repeated
names are deduplicated. Each requested name must match a `test`
target declared by a selected package — an unknown name (or a
name that matches a target of another kind) is an error, even
under `--allow-no-tests`. Every match across the selected
packages runs, so two workspace members may share a test name.
Package selection composes with `--test`: names are looked up
in the selected packages only.

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
selected packages declare no `test` target, so CI does not
silently pass when tests have not been added yet. Pass
`--allow-no-tests` for cases where empty is expected.

## Output and exit status

The status lines mirror `cargo test`'s shape. A run prints the
`running N tests` header, one result line per executable as it
finishes, and a summary line:

```
running 2 tests
test <pkg>:<target> ... ok
test <pkg>:<target> ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

Cabin has no ignore or benchmark mechanism, so `ignored` and
`measured` are constant zeros — they keep the summary line
shaped exactly like `cargo test`'s. `filtered out` counts the
`test` targets in the selected packages that the invocation
deselected via `--test <NAME>`. `finished in` is the wall-clock
time of the test run (the build is not included).

A test executable's stdout / stderr stream live while it runs,
prefixed by `---- stdout: <pkg>:<target> ----` /
`---- stderr: <pkg>:<target> ----` headers. Unlike
`cargo test`, output is not buffered until the end of the run.

A failed test exits non-zero; Cabin records the exit code and
writes:

```
test <pkg>:<target> ... FAILED (exit 17)
```

If any test fails, `cabin test` itself exits non-zero. A
`failures:` recap lists the failed test names before the
summary, followed by the top-level error on stderr:

```
failures:
    <pkg>:<target>

test result: FAILED. P passed; F failed; 0 ignored; 0 measured; FO filtered out; finished in T.TTs
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
type = "test"
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

- It does not run examples. `example` targets are not
  selectable from the command line — they reach the build
  graph only as transitive deps of another selected target.
- It does not parse GoogleTest / Catch2 / doctest output, nor
  emit XML / JUnit reports.
- It does not provide test filtering inside an executable —
  `--test <NAME>` selects whole `test` targets; individual
  test cases inside a binary are the test framework's concern.

## Contributing tests to Cabin

The rules for writing Cabin's *own* test suite — environment
isolation via the shared `cabin()` helper, tool-availability
gating, host-path and driver-name conventions, and the CI
portability boundary — are contributor guidance, not part of
the `cabin test` user surface. They live in
[`AGENTS.md`](https://github.com/cabinpkg/cabin/blob/main/AGENTS.md).
