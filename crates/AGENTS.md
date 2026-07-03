# AGENTS.md - Rust workspace crates

These rules apply under `crates/`. Crate ownership and the per-crate "must
not" rules are canonical in `../docs/architecture.md` ("Crate
responsibilities and rules"); read that section for every crate you touch,
and update the doc in the same change when a boundary moves. Cross-crate
rules that are easy to violate:

- New behavior lands in the owning crate behind a typed API, then threads
  through the CLI. Keep public APIs small: raw serde structs stay private to
  parser crates; PubGrub
  never appears in `cabin-resolver`'s public API; `clap` appears only in
  `cabin`; workspace graph algorithms stay in `cabin-workspace`.
- Keep manifest, config, package, index, registry, and lockfile metadata
  free of local machine state: detected tools, env-derived selections,
  effective config, pkg-config results, and CLI-only choices.
- Keep C and C++ separate: source classification, `CFLAGS`/`CXXFLAGS`,
  standards, compiler capabilities, and link-driver choice must not collapse
  into C++-only assumptions.
- Keep `cabin_core::BuildConfiguration::fingerprint` complete for every
  build-affecting input. Add a focused fingerprint test with new fields.
- Keep machine-readable stdout clean; diagnostics stay in typed domain
  errors and render to stderr through `cabin-diagnostics`.
- Every `CABIN_*` read-side env-var name belongs in `cabin-env`; no string
  literals elsewhere.
- Treat public diagnostic codes and serialized JSON/TOML field names as
  stable user-facing API.

## Test Portability

- Tests that compile real C/C++ sources must gate on tools via helpers in
  `crates/cabin/tests/common/mod.rs`: `require_cxx_build_tools` for C++-only
  builds, `require_c_and_cxx_build_tools` when any `.c` source is compiled
  (Cabin still resolves both CC and CXX). Pure data-model tests need no
  gating.
- CLI tests invoke Cabin through the shared `cabin()` helper so config,
  toolchain, cache, color, and tool-override env vars are scrubbed. Tests
  that exercise env precedence opt back in with `.env(KEY, VALUE)` after
  calling `cabin()`; config-discovery tests use `cabin_with_config()`.
- No host-specific absolute paths (`/tmp/...`, `/usr/bin/...`) in
  integration tests; use `assert_fs::TempDir`. Fake POSIX absolute paths are
  acceptable only in pure planner/model tests that never execute them.
- Prefer structural assertions for generated Ninja and link-driver
  selection; compare resolved driver paths only when the actual path
  matters. Normalize temp paths before comparing output or snapshots.
- Use `assert_fs` for filesystem fixtures, `assert_cmd` for command
  execution, and `predicates` for stdout/stderr/path assertions.
- No external internet access; protocol tests use a local `tiny_http`
  server on `127.0.0.1:0`.

## Platform Notes

- Linux and macOS CI exercise the main Rust workspace; keep paths and tool
  assumptions portable across both.
- Windows/MSVC is supported. MSVC/GNU command-dialect and discovery
  differences live in `cabin-driver` / `cabin-toolchain` (see
  `../docs/toolchains.md`); avoid scattering `cfg(windows)` command policy
  in higher layers.
- `pkg-config` and `run-clang-tidy` smoke tests are ignored on Windows
  because those tools are unavailable on the CI runners.
