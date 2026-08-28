# AGENTS.md - GitHub Actions and workflows

These rules apply under `.github/`. The repository-root `AGENTS.md` also
applies. Repository automation logic lives in private Rust `crates/xtask-*`
crates (see `crates/AGENTS.md`); workflows invoke their cargo aliases.

- Do not put substantial logic in workflow `run:` blocks: loops,
  conditionals, functions, traps, heredocs, or embedded `node`, Python, or
  Perl. Plain command invocations remain inline.
- A workflow that backs a required check must trigger on pushes to `main`,
  pull requests, and `merge_group`, without trigger-level `paths:` filters.
  Scope expensive work at the job level with `.github/path-filters.yml`.
  Non-required workflows may use trigger-level path filters.
- Keep each job-level component dependency list only in
  `.github/path-filters.yml`. Keep it coarse and end it with the consuming
  workflow, the shared `changes` gate workflow, plus the filter file; do
  not add a broad `.github/**` entry.
- Required contexts for gated work must be non-matrix `if: always()`
  aggregates that fail closed when the change gate or work job fails or is
  cancelled. Protect the aggregate, never a skippable work or matrix job.
- Workflow moves or renames must not break the test suite; tests must not
  read `.github/` files or depend on a workflow path (root `AGENTS.md`
  rule).
