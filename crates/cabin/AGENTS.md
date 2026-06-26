# AGENTS.md - CLI crate

`crates/cabin` owns clap parsing and command orchestration. It may call any
workspace crate, but business logic belongs in the typed owning crate.

## CLI Boundaries

- Keep `src/cli/mod.rs` from growing new business logic. New top-level
  commands or non-trivial command code should live in a focused
  `src/cli/<command>.rs` module or, preferably, in the owning library crate.
- CLI code should translate clap inputs into typed requests, call the owning
  crate, and render the result.
- Do not parse manifests, config files, package metadata, lockfiles, or tool
  output in ad hoc CLI helpers when a lower crate owns the format.
- Reuse `cli::config` helpers for build-dir and offline/env precedence.
- Keep `cabin metadata`, `cabin tree --format json`, and
  `cabin explain --format json` stdout machine-readable. Errors go to stderr
  through `cabin-diagnostics`.
- `compgen` and `mangen` must consume `Cli::command()` directly; do not
  duplicate command names, flags, or help text.
- Keep `--target` reserved for future platform/toolchain triples. Do not add a
  manifest-target selector with that name.
