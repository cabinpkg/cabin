# AGENTS.md

## Project Overview

- Cabin is a pre-1.0 package manager and build system for C/C++,
  implemented in Rust.
- Cabin is Cargo-inspired, not Cargo-compatible. Reuse Cargo vocabulary
  only where the C/C++ semantics really line up.
- This repository is the public local OSS core. Do not add network publish,
  account, ownership, quota, policy, control-plane, registry auth, or remote
  cache behavior here unless `docs/architecture.md` explicitly moves it into
  scope.
- The canonical architecture and scope document is
  `docs/architecture.md`. If it disagrees with this file, update both in the
  same change and treat the architecture doc as authoritative.

## Repository Layout

- `crates/` - Rust workspace crates. Read `crates/AGENTS.md` before changing
  crate boundaries, tests, diagnostics, build planning, package/index logic,
  or CLI orchestration.
- `crates/cabin/` - the `cabin` binary and thin command orchestration. Read
  `crates/cabin/AGENTS.md` before changing CLI code.
- `docs/` - canonical Markdown docs rendered by the website. The website
  docs pipeline is described in `website/AGENTS.md`.
- `website/` - Astro site for `cabinpkg.com`; it also renders `docs/`.
  Read `website/AGENTS.md` before changing website code or docs rendering.
- `examples/` - runnable Cabin packages covered by CLI integration tests.
- `.github/workflows/` - CI, release, dist, website, Docker, CodeQL, and
  foundation-port smoke workflows.
- `RELEASING.md` - maintainer release procedure. Do not infer release rules
  from CI alone.

## Common Commands

Run from the repository root unless noted.

```sh
cargo build --workspace
cargo fmt --all --verbose -- --check
taplo fmt --check
typos
cargo clippy --workspace --all-targets --all-features --locked --verbose -- -D warnings
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --locked --verbose
RUSTFLAGS="-D warnings" cargo test --workspace --all-targets --all-features --locked --verbose -- --show-output
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --verbose

npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
  commitlint --extends @commitlint/config-conventional --from origin/main --to HEAD --verbose

cd website
yarn build
```

- Use the full command shapes above when mirroring CI. `--all-features`,
  `--locked`, `RUSTFLAGS="-D warnings"`, `RUSTDOCFLAGS="-D warnings"`, and
  clippy's trailing `-- -D warnings` are intentional.
- `yarn build` is required for changes under `docs/` or `website/`; it runs
  typecheck, Astro build, CSP checks, and docs-link checks.
- Do not edit `typos.toml` or add allowlist entries unless a reviewer
  explicitly asks. Fix the spelling instead.

## Testing And Verification

- Add focused tests for behavior changes. Prefer unit tests in the owning
  crate; add CLI integration coverage when behavior is user-facing.
- Follow `crates/AGENTS.md` for test portability: explicit tool gating,
  shared `cabin()` environment isolation, no host-specific absolute paths,
  structural assertions for generated build output, and normalized snapshots.
- Tests must not require external internet access. Protocol tests should use
  an in-process server on `127.0.0.1:0`.
- When touching C/C++ build planning, manifest parsing, flags, toolchains,
  generated Ninja, packaging, lockfiles, artifacts, metadata, or docs for
  those areas, keep C support first-class and add/update C fixtures alongside
  C++ fixtures when relevant.
- For docs-only changes, run only the checks that match the touched surface
  unless the change also affects generated code, examples, or CI behavior.

## Coding Conventions

- State assumptions before coding when the request is ambiguous. Ask instead
  of silently picking between incompatible interpretations.
- Make surgical changes. Do not refactor adjacent code, reformat unrelated
  files, or remove pre-existing dead code unless asked.
- Prefer simple, direct Rust and existing local patterns. Add abstractions
  only when they remove real duplication or match an established boundary.
- Keep public APIs small. Raw serde structs stay private to parser crates.
- Keep generated output deterministic: sort or normalize workspace
  selections, Ninja files, `compile_commands.json`, metadata/tree/explain
  JSON, archives, lockfiles, registry files, and snapshots.
- Keep user-facing diagnostics in typed domain errors and render them through
  `cabin-diagnostics`. Do not hand-roll presentation in higher layers.
- Every `CABIN_*` read-side env var name belongs in `cabin-env`; do not add
  string literals elsewhere.
- Do not implement "not implemented" features. Unknown future syntax should
  usually fall through generic `deny_unknown_fields` or clap unknown-flag
  diagnostics, not feature-specific rejection arms.

## Architecture Guardrails

- Business logic belongs in the owning crate, not in `crates/cabin`.
  CLI code parses flags, calls typed APIs, and renders results.
- Workspace graph algorithms stay in `cabin-workspace`.
- Manifest parsing stays in `cabin-manifest`.
- Resolver behavior stays in `cabin-resolver`; do not expose PubGrub types.
- Build planning stays in `cabin-build`; Ninja emission stays in
  `cabin-ninja`; command dialect lowering stays in `cabin-driver`.
- Package archives stay in `cabin-package`; publish orchestration stays in
  `cabin-publish`; file-registry mutation stays in `cabin-registry-file`.
- Toolchain and wrapper resolution stay in `cabin-toolchain`; compiler and
  standard capability policy lives in typed core/toolchain surfaces.
- Config discovery and merge stay in `cabin-config`. Local config is never
  serialized into package, index, registry, or lockfile metadata.
- Patch/source-replacement policy stays in typed core/config/workspace
  surfaces. Git sources, registry auth, HTTP publish, and new registry
  protocols remain out of scope.

## Platform, CI, And Release Gotchas

- Linux and macOS Rust CI run the main workspace checks. Windows/MSVC support
  is real; keep path handling, generated commands, and tests portable.
- MSVC/GNU dialect differences belong in the driver/toolchain/build layers,
  not in ad hoc CLI conditionals. See `docs/toolchains.md`.
- `--target` is reserved for future platform/toolchain triples. Do not use it
  for manifest-target selection.
- `--build-dir` is the build-output flag; `--target-dir` is not a Cabin alias.
- Commit subjects must follow Conventional Commits, be lower-case, and stay
  at or under 100 characters. CI runs commitlint.
- Release automation is documented in `RELEASING.md`; do not change cargo-dist,
  binstall, publish, or release workflow behavior as part of unrelated work.

## Docs And Website Sync

- Detailed behavior belongs in `docs/`, not in root `AGENTS.md`.
- If a behavior or architecture change affects users, update the matching
  docs page in the same change.
- If positioning, supported languages/platforms, install instructions, top
  level command surface, or package-page snippets change, update `website/`
  in the same change or call out the required website follow-up.
- New `docs/*.md` pages must be added to `website/src/lib/docsNav.ts`.

## Done Criteria

- The diff is limited to the requested behavior or documentation change.
- New or changed behavior has tests at the right layer.
- Required checks for the touched surface were run, or skipped checks are
  called out with a reason.
- Docs, examples, website, and `AGENTS.md` pointers are updated when the user
  visible surface changes.
- Generated or machine-readable output remains deterministic.

## Detailed References

- `docs/architecture.md` - crate ownership, data flow, scope, and seams.
- `docs/cargo-inspired-interface.md` - Cargo-inspired interface contract.
- `docs/environment-variables.md` - `CABIN_*` variables and precedence.
- `docs/toolchains.md` - toolchain, build flags, compiler detection, MSVC.
- `docs/language-standards.md` - C/C++ standard policy and validation.
- `docs/testing.md` - user-facing `cabin test` behavior.
- `docs/package-format.md`, `docs/package-index.md`,
  `docs/registry-design.md` - package and registry formats.
- `docs/vendoring-offline.md`, `docs/patch-overrides.md`,
  `docs/config.md` - local override and offline behavior.
