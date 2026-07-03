# AGENTS.md

Cabin is a pre-1.0, Cargo-inspired (not Cargo-compatible) package manager and
build system for C/C++, implemented in Rust. Reuse Cargo vocabulary only
where the C/C++ semantics really line up. `docs/architecture.md` is the
canonical architecture and scope document (crate ownership, boundaries, data
flow, scope exclusions); if it disagrees with this file, update both in the
same change and treat the architecture doc as authoritative.

## Repository Layout

- `crates/` - Rust workspace crates. Read `crates/AGENTS.md` before changing
  anything under it.
- `crates/cabin/` - the `cabin` binary. Read `crates/cabin/AGENTS.md` before
  changing CLI code.
- `docs/` - canonical Markdown docs, rendered by the website. Per-page
  summaries are in the "Repository shape today" section of
  `docs/architecture.md`.
- `website/` - Astro site for `cabinpkg.com`; also renders `docs/`. Read
  `website/AGENTS.md` before changing website code or docs rendering.
- `examples/` - runnable Cabin packages covered by CLI integration tests.
- `RELEASING.md` - maintainer release procedure. Do not infer release rules
  from CI alone, and do not change cargo-dist, binstall, publish, or release
  workflow behavior as part of unrelated work.

## Checks

- `bash scripts/ci.sh` runs the CI gate locally, scoping expensive checks to
  the surfaces changed relative to `origin/main`. Agent stop hooks run
  `scripts/ci.sh --hook`, which blocks one attempt to stop while the gate is
  red; a second stop is let through with a warning (`stop_hook_active`).
- The exact per-command shapes are in `CONTRIBUTING.md` "Required checks".
  Mirror the flags verbatim: `--all-features`, `--locked`,
  `RUSTFLAGS="-D warnings"`, `RUSTDOCFLAGS="-D warnings"`, and clippy's
  trailing `-- -D warnings` are intentional.
- Changes under `docs/` or `website/` require, from `website/`:
  `yarn install --frozen-lockfile && yarn lint && yarn build` (build runs
  typecheck, Astro build, CSP checks, and docs-link checks). For docs-only
  changes, run only the checks matching the touched surface.
- Commit subjects follow Conventional Commits, lower-case, at or under 100
  characters (commitlint runs in CI).
- Do not edit `typos.toml` or add allowlist entries unless a reviewer
  explicitly asks. Fix the spelling instead.

## Working Rules

- State assumptions before coding when the request is ambiguous. Ask instead
  of silently picking between incompatible interpretations.
- Make surgical changes: no refactoring adjacent code, reformatting
  unrelated files, or removing pre-existing dead code unless asked. Prefer
  simple, direct Rust and existing local patterns; add abstractions only when
  they remove real duplication or match an established boundary.
- Business logic belongs in the owning crate; `crates/cabin` parses flags,
  calls typed APIs, and renders results. Boundary or scope questions:
  `docs/architecture.md` ("Scope and limitations" lists what is deliberately
  deferred - do not implement deferred features).
- Do not implement "not implemented" features. Unknown future syntax should
  fall through generic `deny_unknown_fields` or clap unknown-flag
  diagnostics, not feature-specific rejection arms.
- Keep C support first-class: when touching build planning, manifests,
  flags, toolchains, generated Ninja, packaging, lockfiles, metadata, or
  docs for those areas, cover C alongside C++ (fixtures included).
- Keep generated and machine-readable output deterministic (sorted or
  normalized); the full list is in `docs/architecture.md`
  ("Contributor-facing architecture guardrails").
- Add focused tests for behavior changes: unit tests in the owning crate,
  plus CLI integration coverage when user-facing. Test portability rules
  live in `crates/AGENTS.md`.
- `--target` is reserved for future platform/toolchain triples; never use it
  for manifest-target selection. `--build-dir` is the build-output flag;
  `--target-dir` is not a Cabin alias.

## Docs And Website Sync

- Detailed behavior belongs in `docs/`, not here. If a behavior or
  architecture change affects users, update the matching docs page in the
  same change.
- New `docs/*.md` pages must be added to `website/src/lib/docsNav.ts`.
- If positioning, supported languages/platforms, install instructions, the
  top-level command surface, or package-page snippets change, update
  `website/` in the same change or call out the required follow-up.

## Done Criteria

- The diff is limited to the requested behavior or documentation change, and
  new or changed behavior has tests at the right layer.
- Required checks for the touched surface were run, or skipped checks are
  called out with a reason.
- Docs, examples, website, and `AGENTS.md` pointers are updated when the
  user-visible surface changes; generated output remains deterministic.
