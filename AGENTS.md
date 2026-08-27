# AGENTS.md

Cabin is a pre-1.0, Cargo-inspired, but not Cargo-compatible, package manager
and build system for C/C++, implemented in Rust. Use Cargo vocabulary only
when the C/C++ semantics match.

`docs/architecture.md` is authoritative for architecture, crate ownership,
data flow, and scope exclusions. If it conflicts with this file, update both
in the same change and follow the architecture document.

## Scoped instructions and canonical docs

- Read `crates/AGENTS.md` before changing `crates/`. For CLI work, also read
  `crates/cabin/AGENTS.md`.
- Read `website/AGENTS.md` before changing `website/` or docs rendering.
  `docs/` contains the canonical Markdown rendered by the website.
- Use `RELEASING.md` for release procedure. Do not infer release policy from
  CI or change cargo-dist, binstall, publishing, or release workflows during
  unrelated work.

## Working rules

- Resolve incompatible interpretations before coding. State any assumption
  that materially affects the result.
- Do not modify `AGENTS.md` unless explicitly requested, or unless the current
  change would otherwise make an existing instruction factually incorrect.
- Make the smallest coherent change required. Do not refactor, reformat,
  clean up, or remove unrelated code, including pre-existing dead code.
- Reuse existing code and patterns. Prefer direct Rust; add no speculative
  abstraction, configuration, or flexibility.
- Treat review comments as findings to evaluate, not requirements to
  implement. Fix security issues, supported-path correctness bugs, and
  documented contract violations. Do not expand the design solely to handle
  speculative edge cases or make behavior theoretically complete.
- Complexity must be proportional to the failure being prevented. Do not add
  state, reconciliation, lifecycle tracking, or cleanup machinery for rare
  combinations of transient failures unless they can cause a security issue,
  data loss, or a realistic user-visible correctness failure.
- Prefer reducing the state space or simplifying an invariant over adding
  logic to make every possible state behave perfectly. Once supported
  behavior is correct and secure, stop.
- Comments explain non-obvious constraints, compatibility requirements, or
  rationale. Do not restate mechanics that clearer code can express.
- Business logic belongs in its owning crate. `crates/cabin` parses flags,
  calls typed APIs, and renders results.
- Do not implement features listed as deferred or "not implemented" in
  `docs/architecture.md`. Unknown future syntax must use generic
  `deny_unknown_fields` or clap unknown-flag diagnostics, not tailored
  rejection arms.
- Keep C first-class alongside C++. Changes to planning, manifests, flags,
  toolchains, Ninja output, packaging, lockfiles, metadata, and related docs
  must cover both languages, including fixtures.
- Keep generated and machine-readable output sorted or normalized. See
  `docs/architecture.md` "Contributor-facing architecture guardrails" for
  the affected outputs.
- Add focused tests that detect a concrete behavioral regression. Prefer the
  lowest useful layer; add CLI integration coverage when end-to-end behavior
  itself is the contract, not to duplicate unit coverage. Do not test
  implementation text/layout, trivial derived behavior, or library/framework
  guarantees. Follow the portability rules in `crates/AGENTS.md`.
- Scripted or repeated edits must verify that the expected pattern matched
  and the old form is gone.
- Claim verification only when an executed check could have detected the
  relevant failure. Report checks run and any relevant checks skipped.
- Do not edit `typos.toml` or add allowlist entries unless a reviewer asks;
  fix the spelling.

## Cabin invariants

- `--target` is reserved for future platform/toolchain triples. Do not use it
  for manifest-target selection. The build-output flag is `--build-dir`;
  `--target-dir` is not an alias.
- A port directory is published verbatim, so every edit, including a comment,
  changes archive bytes. Compare its published digest before editing and do
  not fold a port correction into unrelated work. A byte-only correction is
  a packaging revision. Changes to `dependencies`, `features`, or `standards`
  require a new upstream version; `links` may only be added when absent. The
  temporary-registry preflight cannot validate this distinction. See
  `docs/foundation-ports.md` "Packaging revisions".

## Repository automation and GitHub Actions

- Repository automation belongs in private, `publish = false`
  `crates/xtask-*` crates exposed through aliases in `.cargo/config.toml`.
  Extend the owning xtask; add one only for a new responsibility with a new
  dependency set. Run aliases from the repository root because `registry/`
  is a separate workspace.
- Do not reintroduce shell or Perl repository tooling, or substantial logic
  in workflow `run:` blocks: loops, conditionals, functions, traps, heredocs,
  or embedded `node`, Python, or Perl. Plain command invocations remain
  inline. This restriction does not cover product or website source, npm
  scripts, `Dockerfile`, `demo.tape`, or devcontainer provisioning.
- A workflow that backs a required check must trigger on pushes to `main`,
  pull requests, and `merge_group`, without trigger-level `paths:` filters.
  Scope expensive work at the job level with `.github/path-filters.yml`.
  Non-required workflows may use trigger-level path filters.
- Keep each job-level component dependency list only in
  `.github/path-filters.yml`.
  Keep it coarse and end it with the consuming workflow plus the filter file;
  do not add a broad `.github/**` entry.
- Required contexts for gated work must be non-matrix `if: always()`
  aggregates that fail closed when the change gate or work job fails or is
  cancelled. Protect the aggregate, never a skippable work or matrix job.
- Tests must not read `.github/` files or depend on a workflow path. Workflow
  moves or renames must not break the test suite.

## Checks

- Run `cargo ci` from the repository root. It scopes expensive checks to
  changes relative to `origin/main`.
- Changes under `docs/`, `website/`, or `ports/` require the website gate:
  from `website/`, run `npm ci`, `npm run lint`, `npm test`, and
  `npm run build`. `cargo ci` runs these for those paths; run them manually if
  site output changes through another path.
- Commit subjects use Conventional Commits, lower case, at most 100
  characters.

## Documentation sync

- Update the matching `docs/` page with user-visible behavior or architecture
  changes. Add new `docs/*.md` pages to `website/src/lib/docsNav.ts`.
- Update `website/` in the same change, or identify the required follow-up,
  when changing product positioning, supported languages or platforms,
  installation, top-level commands, or package-page snippets.
- Migrations and removals can stale descriptions outside the code path.
  Check `docs/`, `CONTRIBUTING.md`, each affected example README and
  `examples/README.md`, `ports/README.md`, and website copy. Update claims
  about the repository's current shape; leave durable upstream facts and
  policy alone.

## Git and pull requests

- All implementation work uses a branch and PR; never implement directly on
  the default branch. Before editing, inspect status, preserve all unrelated
  local changes, update the default branch from its remote, and create a fresh
  branch. Do not discard, reset, overwrite, or commit unrelated changes.
- Use one branch per PR. Each PR must be the smallest cohesive change that is
  independently buildable, testable, and reviewable. Split independent
  changes by default; keep changes together when a split would leave a
  broken, untestable, unsafe, misleading, or meaningless intermediate state.
- If work needs multiple PRs, state their order and responsibility. Handle
  dependent PRs sequentially: do not start or open a later dependent branch
  or PR until the current PR is approved, green, rebased or autosquashed as
  needed, squash-merged, and the local default branch is updated. Independent
  PRs may overlap in time only when they do not overlap in scope.
- Normally keep the initial implementation in one cohesive commit. Fixup
  commits for review feedback, CI failures, test corrections, or small
  omissions are allowed when they remain within the PR's scope.
- Before opening or updating a PR, run the relevant checks and report their
  results. Fix failures caused by the change.
- Squash merge only. Merge after required approval, passing required checks,
  and resolution of all review comments and requested changes. Resolving a
  review comment does not imply implementing it: reject findings that are
  incorrect, speculative, out of scope, or disproportionately complex, with
  a concise rationale. Delete the merged branch when authorized; otherwise
  stop when the PR is ready for an authorized maintainer.
