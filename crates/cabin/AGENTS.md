# AGENTS.md - CLI crate

`crates/cabin` owns clap parsing and command orchestration. It may call any
workspace crate; business logic belongs in the typed owning crate.

- `src/cli/mod.rs` must not grow new business logic. New top-level commands
  or non-trivial command code go in a focused `src/cli/<command>.rs` module
  or, preferably, the owning library crate. CLI code translates clap inputs
  into typed requests, calls the owning crate, and renders the result.
- Do not parse manifests, config files, package metadata, lockfiles, or tool
  output in ad hoc CLI helpers when a lower crate owns the format.
- Reuse `cli::config` helpers for build-dir and offline/env precedence.
- `cabin metadata`, `cabin tree --format json`, and
  `cabin explain --format json` stdout stays machine-readable; errors go to
  stderr through `cabin-diagnostics`.
- `compgen` and `mangen` must consume `Cli::command()` directly; never
  duplicate command names, flags, or help text.
- `--target` stays reserved for future platform/toolchain triples; do not
  add a manifest-target selector with that name.
