# AGENTS.md - GitHub Actions and workflows

These rules apply under `.github/`. The repository-root `AGENTS.md` also
applies. Cabin-specific repository automation and orchestration lives in
private Rust `crates/xtask-*` crates (see `crates/AGENTS.md`). Workflows invoke
their cargo aliases or established external tools directly.

- Do not put substantial logic in workflow `run:` blocks: loops,
  conditionals, functions, traps, heredocs, or embedded `node`, Python, or
  Perl. Plain command invocations remain inline.
- When the same configuration or command is repeated across workflow steps,
  prefer the narrowest native workflow- or job-level mechanism that preserves
  the same semantics. Do not introduce a custom abstraction merely to remove
  incidental duplication.
- A workflow that backs a required check must trigger on pull requests
  and `merge_group`, without trigger-level `paths:` filters. Scope
  expensive work at the job level with `.github/path-filters.yml`.
  Non-required workflows may use trigger-level path filters.
- Merge Queue's `merge_group` run is the authoritative pre-merge
  validation; never add `push: main` to repeat validation it already
  performed. `push: main` is reserved for genuine post-merge semantics:
  production side effects (the registry deploy), GitHub features that
  require a default-branch push analysis (currently CodeQL), and
  path-filtered final-state checks in advisory workflows with no
  `merge_group` run (proofs, zizmor).
- Keep each job-level component dependency list only in
  `.github/path-filters.yml`. Keep it coarse and end it with the consuming
  workflow, the shared `changes` gate workflow, any local action it uses,
  plus the filter file; do not add a broad `.github/**` entry.
- Required contexts for gated work must be non-matrix `if: always()`
  aggregates that fail closed when the change gate or work job fails or is
  cancelled. Protect the aggregate, never a skippable work or matrix job.
- Workflow moves or renames must not break the test suite; tests must not
  read `.github/` files or depend on a workflow path (root `AGENTS.md`
  rule).
