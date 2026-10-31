# AGENTS.md

Cabin is a pre-1.0, Cargo-inspired, but not Cargo-compatible, package manager
and build system for C/C++, implemented in Rust. Use Cargo vocabulary only
when the C/C++ semantics match.

`docs/architecture.md` is authoritative for architecture, crate ownership,
data flow, and scope exclusions. If it conflicts with this file, update both
in the same change and follow the architecture document.

## Scoped instructions and canonical docs

- Read `crates/AGENTS.md` before changing `crates/`. Before changing
  `crates/cabin/`, also read `crates/cabin/AGENTS.md`.
- Read `registry/AGENTS.md` before changing `registry/`; it routes registry
  work to the owning module and names the registry-specific checks, which
  `cargo ci` does not cover.
- Read `website/AGENTS.md` before changing `website/`, `docs/`, or `ports/`;
  those paths share the website verification gate. `docs/` contains the
  canonical Markdown rendered by the website.
- Read `.github/AGENTS.md` before changing GitHub Actions workflows or other
  `.github/` configuration.
- Use `RELEASING.md` for release procedure. Do not infer release policy from
  CI or change binstall, publishing, or release workflows during
  unrelated work.

## Working rules

- Resolve incompatible interpretations before coding. State any assumption
  that materially affects the result.
- Do not modify `AGENTS.md` unless explicitly requested, or unless the current
  change would otherwise make an existing instruction factually incorrect.
- Make the smallest coherent change required. Do not refactor, reformat,
  clean up, or remove unrelated code, including pre-existing dead code.
- Prefer existing repository mechanisms and maintained upstream tools,
  libraries, and platform features over repository-owned replacements. Do not
  reimplement or compatibility-port third-party semantics merely to remove a
  dependency, runtime, or implementation language.
- If exact behavioral parity with an external tool is required, use that tool
  as the source of truth rather than cloning its behavior. Owning a replacement
  parser, linter, formatter, protocol implementation, or compatibility layer
  requires explicit maintainer approval.
- Reuse existing code and patterns. Within Cabin-owned Rust code, prefer
  direct implementations over speculative abstraction, configuration, or
  flexibility.
- When documentation and existing code disagree about implementation
  conventions, follow the established patterns in the relevant code unless
  doing so violates an explicit requirement, architectural decision, or
  security invariant. Report each discrepancy you encounter in your final
  reply.
- Before finalizing a change, review the complete diff for code,
  configuration, helpers, files, or dependencies that can be removed or
  simplified without changing the intended behavior.
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
- Do not implement features listed as deferred or "not implemented" in
  `docs/architecture.md`, except work it marks *planned*, which lands only
  through its stage in `docs/design/capability-roadmap/ledger.md` and
  against that roadmap's contracts. Unknown future syntax must use generic
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
- Tests must not read `.github/` files or depend on a workflow path. Workflow
  moves or renames must not break the test suite.
- Scripted or repeated edits must verify that the expected pattern matched
  and the old form is gone.
- Claim verification only when an executed check could have detected the
  relevant failure. Report checks run and any relevant checks skipped.
- Do not edit `typos.toml` or add allowlist entries unless a reviewer asks;
  fix the spelling.

## Cabin invariants

- `--target` selects the platform triple to build for. Do not use it for
  manifest-target selection. The build-output flag is `--build-dir`;
  `--target-dir` is not an alias.
- A port directory, except its unpublished `tests/`, is published verbatim, so
  every edit, including a comment, changes archive bytes. Compare its published
  digest before editing and do not fold a port correction into unrelated work.
  Follow
  `docs/foundation-ports.md` "Packaging revisions" for how a correction
  respins as a packaging revision of the same upstream version.

## Repository automation

- Cabin-specific repository automation and orchestration belongs in private
  Rust `crates/xtask-*` crates run through cargo aliases; see
  `crates/AGENTS.md` "Repository automation (xtasks)".
- Do not reintroduce shell or Perl repository tooling. This restriction does
  not cover product or website source, npm scripts, `Dockerfile`, `demo.tape`,
  devcontainer provisioning, or normal invocation/configuration of external
  tools.

## Checks

- Run `cargo ci` from the repository root. It scopes expensive checks to
  changes relative to `origin/main`.
- Changes under `docs/`, `website/`, or `ports/` require the website gate
  defined in `website/AGENTS.md`. `cargo ci` runs it for those paths; run it
  manually if site output changes through another path.
- Changes under `registry/` require the registry checks listed in
  `registry/AGENTS.md`; `cargo ci` does not run them.
- Commit-message policy follows `@commitlint/config-conventional`; treat
  upstream commitlint behavior as authoritative. The generated squash-merge
  header must remain within its 100-character limit.

## Documentation sync

- Update the matching `docs/` page with user-visible behavior or architecture
  changes.
- Update `website/` in the same change, or identify the required follow-up,
  when changing product positioning, supported languages or platforms,
  installation, top-level commands, or package-page snippets.
- Migrations and removals can stale descriptions outside the code path.
  Check `docs/`, `CONTRIBUTING.md`, each affected example README and
  `examples/README.md`, `ports/README.md`, and website copy. Update claims
  about the repository's current shape; leave durable upstream facts and
  policy alone.
- Keep documentation focused on current behavior, contracts, design
  rationale, and unresolved work. Update existing explanations instead of
  appending implementation diaries, completed-task reports, or one-time
  validation transcripts. Keep task history in commits and pull requests.
- When a change resolves a documented gap or supersedes a plan, update or
  remove the affected blocker entries, TODOs, and handoffs in the same
  change. Preserve remaining requirements, active dependencies, and useful
  distinctions between implemented, published, and verified behavior.
- Preserve information that still serves readers, including design
  rationale, compatibility boundaries, reproducible measurements, release
  notes, and migration guidance. Prefer one authoritative explanation with
  links over duplicated content. Do not create archive pages merely to
  relocate obsolete task history.

## Git and pull requests

- All implementation work, including documentation changes, uses a dedicated
  temporary worktree, a branch, and a PR; never implement directly on the
  default branch. At task start, inspect existing worktrees and their status.
  Reuse a worktree only if it is already dedicated to the current task;
  otherwise fetch origin and create a new worktree with a fresh branch from
  origin/main. Do not switch, reset, or update a branch checked out in
  another worktree. Preserve unrelated changes: do not discard, overwrite,
  or commit them. Read-only investigation does not require a new worktree.
- Use one branch per PR. Each PR must be the smallest cohesive change that is
  independently buildable, testable, and reviewable. Split independent
  changes by default; keep changes together when a split would leave a
  broken, untestable, unsafe, misleading, or meaningless intermediate state.
- If work needs multiple PRs, state their order and responsibility. Handle
  dependent PRs sequentially: do not start or open a later dependent branch
  or PR until the current PR has no blocking review findings, is green,
  squash-merged, and origin/main is refreshed by fetching. Independent PRs
  may overlap in time only when they do not overlap in scope.
- Before starting implementation and before opening or enqueueing a PR,
  inspect all open PRs, including those in the merge queue, for potential
  conflicts in files, behavior, interfaces, and dependencies. Account for
  the latest default branch and queued PR order. Resolve or sequence
  conflicting work rather than allowing incompatible changes to proceed.
- Recheck affected open and queued PRs after relevant merges. For PRs under
  your control, proactively resolve confirmed conflicts, rerun relevant
  checks, and obtain a fresh review before re-enqueueing. Do not modify
  PRs owned by other sessions or contributors without coordination.
- Track the merge-queue outcome of PRs you manage. If a PR is removed
  because of conflicts or failed checks, investigate and fix the cause
  before retrying. Do not update queued branches solely because the
  default branch advanced; the merge queue validates the combined result.
  Never treat an enqueued PR as merged.
- Before opening or updating a PR, run the relevant checks and report their
  results. Fix failures caused by the change.
- Open a PR as one cohesive commit; its subject becomes the PR title. Once a
  PR is open, land review feedback, CI failures, test corrections, and small
  omissions as ordinary commits on top, within the PR's scope. Do not rewrite
  pushed commit history to prepare a PR for squash merge.
- After any content change, obtain a fresh review of the updated PR before
  merge.
- If you have permission, squash merge once the latest review reports no
  blocking findings, required checks pass, and all review comments and
  requested changes are resolved. Resolving a review comment does not imply
  implementing it: reject findings that are incorrect, speculative, out of
  scope, or disproportionately complex, with a concise rationale. Remove its
  temporary worktree before deleting the merged branch.
- Clean up temporary worktrees you created when their work is complete.
  Opening or enqueueing a PR, or ending a session, is not completion.
  Before removal, check for uncommitted and untracked files and unpushed
  commits; preserve anything needed and confirm the task's changes are
  merged or otherwise recoverable. Never remove another session's worktree
  or force removal over unpreserved work. If a worktree must remain, report
  its path and why.
