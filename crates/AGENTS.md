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
  `cabin` and the `xtask-*` binaries; workspace graph algorithms stay in
  `cabin-workspace`.
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

## Repository automation (xtasks)

- Repository-owned automation and orchestration belongs in private,
  `publish = false` `crates/xtask-*` crates exposed through aliases in
  `.cargo/config.toml`. Extend the owning xtask; add one only for a new
  responsibility with a new dependency set. Run aliases from the repository
  root because `registry/` is a separate workspace.

## Test portability

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

## Platform portability

- Keep paths and tool assumptions portable across Linux and macOS.
- Windows/MSVC is supported. MSVC/GNU command-dialect and discovery
  differences live in `cabin-driver` / `cabin-toolchain` (see
  `../docs/toolchains.md`); avoid scattering `cfg(windows)` command policy
  in higher layers.
