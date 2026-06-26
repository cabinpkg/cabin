# AGENTS.md - Rust workspace crates

These rules apply under `crates/`. The detailed architecture is
`../docs/architecture.md`; keep this file short and update the doc when a
boundary moves.

## Crate Boundaries

- `cabin-core` owns format-agnostic domain models, build configuration,
  source-language helpers, profiles, toolchain/build-flag types, compiler
  capability data, and language-standard logic. It must not parse TOML, use
  clap, invoke processes, know Ninja, or read/write indexes, registries, or
  lockfiles.
- `cabin-manifest` owns `cabin.toml` parsing. Raw serde structs stay private.
  It must not load workspaces, resolve dependencies, write Ninja, or touch
  `cabin.lock`.
- `cabin-workspace` owns workspace discovery, member expansion, selection,
  local/path dependency graph loading, workspace inheritance, and patch
  stitching. Graph algorithms stay here, not in the CLI.
- `cabin-resolver` owns resolution and lockfile-aware modes. PubGrub is a
  private implementation detail.
- `cabin-lockfile` owns the `cabin.lock` model, deterministic formatting, and
  I/O. It must not run the resolver or parse manifests.
- `cabin-artifact` owns archive checksum verification, cache layout, and safe
  extraction. It must stay network-free.
- `cabin-package` owns deterministic source archives and canonical package
  metadata. It must not mutate registries or invoke compilers.
- `cabin-publish` orchestrates local publish; `cabin-registry-file` owns the
  local file-registry layout and atomic mutation.
- `cabin-index` owns local JSON indexes; `cabin-index-http` owns the read-only
  sparse HTTP path. Neither publishes or authenticates.
- `cabin-toolchain` owns tool and compiler-cache wrapper resolution,
  subprocess version probing, Ninja lookup, and MSVC discovery. It must not
  parse TOML, resolve packages, or compile probe sources.
- `cabin-build` owns backend-independent build planning and planned-standard
  validation. It must not write Ninja or parse manifests.
- `cabin-driver` owns GCC/Clang vs. MSVC command-line lowering.
- `cabin-ninja` owns `build.ninja` and `compile_commands.json` generation.
- `cabin-test` owns test-target discovery from a finished build graph and the
  sequential test runner. It must not plan builds or parse manifests.
- `cabin-explain` owns typed `metadata`/`tree`/`explain` render models and
  deterministic renderers. It should not perform I/O.
- `cabin-env` is the only home for `CABIN_*` names and run/test env overlays.
- `cabin-source-discovery` owns the shared C/C++ source walker for `fmt` and
  `tidy`; the runner crates own command construction.
- `cabin-fmt`, `cabin-tidy`, and `cabin-system-deps` own their external tool
  argv and typed request/report boundaries.
- `cabin-fs` owns narrow filesystem helpers only. Domain-specific path,
  archive, registry, config, and diagnostic policy stays with consumers.
- `cabin-diagnostics` owns stable user-facing diagnostic rendering and the
  annotate-snippets boundary.

## Cross-Crate Rules

- Add new behavior to the owning crate first, expose a typed API, then thread
  it through the CLI.
- Keep manifest, config, package, index, registry, and lockfile metadata free
  of local machine state such as detected tools, env-derived selections,
  effective config, pkg-config results, and CLI-only choices.
- Keep C and C++ separate: source classification, `CFLAGS`/`CXXFLAGS`,
  standards, compiler capabilities, and link-driver choice must not collapse
  into C++-only assumptions.
- Keep `cabin_core::BuildConfiguration::fingerprint` complete for every
  build-affecting input. Add a focused fingerprint test with new fields.
- Keep machine-readable stdout clean. Diagnostics go to stderr through
  `cabin-diagnostics`.
- Treat public diagnostic codes and serialized JSON/TOML field names as stable
  user-facing API.

## Test Portability

- Tests that compile real C/C++ sources must explicitly require tools through
  helpers in `crates/cabin/tests/common/mod.rs`.
- Use `require_cxx_build_tools` for C++-only builds.
- Use `require_c_and_cxx_build_tools` for any test that compiles `.c` sources;
  Cabin still resolves both CC and CXX.
- Pure data-model tests do not need external tool gating.
- CLI tests should invoke Cabin through the shared `cabin()` helper so config,
  toolchain, cache, color, and tool override env vars are scrubbed.
- Tests that intentionally exercise env precedence should opt back in with
  `.env(KEY, VALUE)` after calling `cabin()`.
- Config-discovery tests should use the existing `cabin_with_config()` pattern.
- Do not hardcode host-specific absolute paths such as `/tmp/...` or
  `/usr/bin/...` in integration tests. Use `assert_fs::TempDir`.
- Fake POSIX absolute paths are acceptable only in pure planner/model tests
  that never execute them.
- Prefer structural assertions for generated Ninja and link-driver selection;
  compare resolved driver paths from metadata only when the actual path matters.
- Normalize temp paths before comparing generated output or snapshots.
- Use `assert_fs` for filesystem fixtures, `assert_cmd` for command execution,
  and `predicates` for stdout/stderr/path assertions.
- Tests must not require external internet access. Use local `tiny_http` on
  `127.0.0.1:0` for sparse-HTTP coverage.

## Platform Notes

- Linux and macOS CI exercise the main Rust workspace. Keep paths and tool
  assumptions portable across both.
- Windows/MSVC is supported. `cabin-driver` and `cabin-toolchain` own dialect
  and discovery differences; avoid scattering `cfg(windows)` command policy in
  higher layers.
- `pkg-config` and `run-clang-tidy` smoke tests are ignored on Windows because
  those tools are unavailable on the CI runners.
