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
- `.cargo/config.toml` - the Cargo aliases that expose the `xtask-*`
  repository tools. See "Repository Automation".
- `RELEASING.md` - maintainer release procedure. Do not infer release rules
  from CI alone, and do not change cargo-dist, binstall, publish, or release
  workflow behavior as part of unrelated work.

## Checks

- `cargo ci` runs the CI gate locally, scoping expensive checks to
  the surfaces changed relative to `origin/main`. Agent stop hooks run
  `cargo ci --hook`, which blocks one attempt to stop while the gate is
  red; a second stop is let through with a warning (`stop_hook_active`).
- The exact per-command shapes are in `CONTRIBUTING.md` "Required checks".
  Mirror the flags verbatim: `--all-features`, `--locked`,
  `RUSTFLAGS="-D warnings"`, `RUSTDOCFLAGS="-D warnings"`, and clippy's
  trailing `-- -D warnings` are intentional.
- Changes under `docs/` or `website/` require, from `website/`:
  `npm ci && npm run lint && npm run build && npm test` (build runs
  typecheck, Astro build, CSP checks, and docs-link checks). For docs-only
  changes, run only the checks matching the touched surface.
- `cargo ci` runs the same four website commands `website.yml` does,
  scoped to changes touching `website/`, `docs/` or `ports/`. Outside that
  scope it skips them, so a change that reaches the site another way still
  needs them by hand.
- Commit subjects follow Conventional Commits, lower-case, at or under 100
  characters (commitlint runs in CI).
- Do not edit `typos.toml` or add allowlist entries unless a reviewer
  explicitly asks. Fix the spelling instead.

## Repository Automation

Repository automation belongs in Rust. Each tool is a private
`crates/xtask-<name>` crate (`publish = false`, never part of the shipped
`cabin` binary), reached through a Cargo alias in `.cargo/config.toml`. The
aliases name root-workspace packages, so run them from the repository root -
`registry/` is a separate workspace and resolves against its own manifest.

The migration is complete: `registry/scripts/` is gone, and no shell
tooling is left in this repository. Do not reintroduce any.

- New automation becomes a subcommand of the `xtask-*` crate that already
  owns that responsibility, or a new crate when the responsibility *and* the
  dependency set are genuinely new. Do not add a shell or Perl script, and do
  not put substantial logic (loops, conditionals, functions, traps,
  heredocs, embedded `node -` / `python3 -` / `perl -e`) in a workflow
  `run:` block - call an alias instead. Plain invocations (`cargo build`,
  `npm ci`, package installation) stay inline.
- The rule covers repository tooling only. Product source (`crates/cabin*`,
  `registry/src/`), website source and its npm scripts, the `Dockerfile`,
  `demo.tape`, and devcontainer provisioning are not repository automation
  and are unaffected.
- Workflows whose jobs back required status checks trigger on `push`
  to `main`, on `pull_request`, and on `merge_group` (the merge
  queue's synthetic commits) with no trigger-level `paths:` filter:
  a filtered-out workflow leaves its required checks pending
  forever, while a job skipped by an `if:` satisfies them. Their
  component scoping therefore happens at the job level - each
  workflow's `changes` job evaluates the shared filter lists in
  `.github/path-filters.yml` and the expensive jobs skip when theirs
  does not match. A workflow with no required check may scope at the
  trigger with `paths:` instead - a filtered-out run there blocks
  nothing (proofs and the workflow audit do; their lists live in
  their own `paths:`). Every other dependency list has its ONE copy
  in the filter file; keep the lists coarse (whole directories,
  shared dependencies) and end each with its consuming workflow file
  plus `.github/path-filters.yml`, so a workflow or filter edit
  re-runs exactly the workflows it affects - never put a broad
  `.github/**` back in a list. No test polices the file - the
  previous per-workflow copies grew a YAML-scraping test that broke
  on an unrelated rename.
  Two carve-outs: a required matrix job reports through a non-matrix
  aggregate (`build-and-test-required`), because skipping a matrix job
  never expands it, so a required per-leg name would never report and
  the merge would hang forever; and a failed (not skipped) `changes`
  job skips its dependents to satisfied - the aggregate fails closed
  on that, and the other required checks accept it with the red gate
  run as the only signal.
- No test may read a file under `.github/` or depend on a workflow living at
  a particular path. Moving or renaming a workflow must never be able to
  fail the test suite.

## Engineering Principles

Apply these pragmatically, not mechanically. When they conflict, prioritize
correctness, readability, simplicity, and ease of change over theoretical
purity. Make the smallest coherent change that solves the current problem,
and do not add speculative flexibility.

- Keep the implementation simple and focused: KISS and YAGNI.
- Avoid duplicating the same knowledge or business rule: DRY.
- Do not introduce abstractions prematurely: follow AHA and the Rule of
  Three.
- Separate distinct concerns and keep each component responsible for one
  coherent purpose. Prefer high cohesion and low coupling.
- Prefer composition over inheritance.
- Keep APIs and behavior unsurprising: follow the Principle of Least
  Astonishment.
- Limit dependencies on internal object structure: follow the Law of
  Demeter.
- Validate assumptions early and fail fast with clear errors.
- Use types and data structures that make invalid states unrepresentable
  where practical.
- Prefer explicit behavior, dependencies, and configuration over hidden or
  implicit mechanisms.
- Apply SOLID principles where they improve clarity, substitutability, and
  maintainability, but avoid unnecessary indirection.
- If an external library can solve the same problem, use it, but weigh the
  full cost first.

## Working Rules

- State assumptions before coding when the request is ambiguous. Ask instead
  of silently picking between incompatible interpretations.
- Make surgical changes: no refactoring adjacent code, reformatting
  unrelated files, or removing pre-existing dead code unless asked. Prefer
  simple, direct Rust and existing local patterns; add abstractions only when
  they remove real duplication or match an established boundary.
- Comments explain constraints and rationale, not mechanics: non-obvious
  invariants, compatibility requirements, workarounds, external constraints,
  or why an obvious alternative is incorrect. Do not write comments that
  restate what the code does. Before keeping or adding a comment, first
  consider whether clearer naming, extracting a function, introducing a
  type, or restructuring would make it unnecessary.
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
- A scripted or repeated edit must assert, not assume: check that the
  pattern matched and that the old identifier is gone afterwards. A
  silently-skipped edit passes every check that does not compile the file.
- Green checks only cover the surfaces they touch. Before calling a change
  verified, name which check would have failed had the change been wrong;
  if none would, the change is unverified.
- A **port** directory under a ports tree is published **verbatim**: editing
  it - a comment included - changes the published archive bytes.
  A byte-only correction is a packaging revision; an edit that changes what
  resolution consumes (`dependencies`, `features`, `standards`) is not a
  revision at all and needs a new upstream version, with `links` stamping
  the one-way exception - see `docs/foundation-ports.md` "Correcting a
  published port". The preflight dry-run publishes into a fresh temporary
  registry, so it cannot catch that distinction for you. Never fold such an
  edit into an unrelated change, and check the published digest either way.
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
- A change that alters what is *true* rather than what runs (a migration, a
  removal) silently invalidates prose. Sweep every surface that describes the
  thing: `docs/`, `CONTRIBUTING.md`, the per-example `README.md`s **and the
  aggregate `examples/README.md`**, the ports tree `README.md`, and website
  copy that names specific packages or features. Separate claims
  about a thing's *shape* ("published from the curated recipe", "reachable as
  `port = true`"), which go stale, from durable facts about upstream or
  policy, which do not - and fix only the former.

## Done Criteria

- The diff is limited to the requested behavior or documentation change, and
  new or changed behavior has tests at the right layer.
- Required checks for the touched surface were run, or skipped checks are
  called out with a reason.
- Docs, examples, website, and `AGENTS.md` pointers are updated when the
  user-visible surface changes; generated output remains deterministic.

## Git and pull request workflow

All implementation work must use branches and pull requests. Never implement changes directly on the default branch.

### Core principle

Each pull request must be the **smallest cohesive change that can be meaningfully reviewed, tested, and squash-merged**.

Default to splitting work into smaller pull requests. Keep changes together only when separating them would make an intermediate pull request:

* broken or non-buildable;
* untestable;
* misleading or semantically incomplete;
* unsafe to merge;
* or not meaningfully reviewable on its own.

Do not combine changes merely because they belong to the same issue, feature, or overall task.

### Before editing

Before making changes:

1. Inspect the repository status and preserve all unrelated local changes.
2. Update the local default branch from its remote.
3. Determine whether the requested work fits into one minimal, cohesive pull request.
4. If multiple pull requests are needed, briefly state the proposed sequence and the responsibility of each pull request.
5. Create a fresh branch from the latest default branch for the first pull request.

Do not discard, reset, overwrite, commit, or otherwise modify unrelated local changes.

### Pull request scope

A pull request may include only:

* the code required for its single cohesive change;
* tests required to verify that change;
* documentation required to accurately describe that change;
* and strictly necessary mechanical updates caused by that change.

Do not include:

* unrelated refactoring;
* opportunistic cleanup;
* optional improvements;
* formatting changes outside the affected area;
* preparatory work that is needed only by a later pull request;
* or additional features that can be reviewed and merged separately.

When uncertain whether two changes belong together, default to separate pull requests.

Do not split work into artificial or meaningless fragments. Every merged pull request must leave the repository in a valid, tested, and usable state.

### Branches and commits

Use one branch per pull request.

The initial implementation should normally be committed as one cohesive commit. During the pull request lifecycle, additional fixup commits are allowed for:

* review feedback;
* CI failures;
* test corrections;
* small omissions;
* and other changes required to make the current pull request acceptable.

Do not add unrelated work through fixup commits.

Use clear branch names and commit messages that describe the specific change.

### Large or dependent tasks

If the overall task is too large for one minimal, cohesive pull request, split it into an ordered sequence of smaller pull requests.

Later pull requests may depend on earlier pull requests. Dependent work must be processed strictly sequentially:

1. Implement, test, commit, push, and open the current pull request.
2. Address review feedback with fixup commits as needed.
3. Wait until the pull request is approved and all required checks pass.
4. Do autosquash and rebase the current branch onto the updated default branch, resolving any conflicts.
5. Wait until the pull request is approved and all required checks pass again.
6. If you get new review feedback, repeat steps 2–5 until the pull request is ready to merge.
7. Squash-merge the pull request into the default branch.
8. Update the local default branch from the remote.
9. Create a fresh branch from the updated default branch.
10. Begin the next dependent pull request.

Do not:

* create stacked branches;
* create a later branch from an unmerged feature branch;
* keep multiple dependent pull requests open simultaneously;
* target one feature branch from another pull request;
* or place several sequential review steps into one multi-commit pull request.

Independent pull requests may be open concurrently only when they do not depend on or overlap with each other.

If a task cannot be decomposed into valid sequential pull requests without leaving broken or unusable intermediate states, keep the inseparable portion together and explain why it cannot be split further.

### Testing

Before opening or updating a pull request:

1. Run the relevant formatting, linting, build, and test commands.
2. Fix failures caused by the current change.
3. Report which checks were run and their results.
4. Clearly disclose any checks that could not be run and why.

Do not claim that a change is complete or tested when the relevant verification has not been performed.

### Review lifecycle

After opening a pull request:

* keep all further changes limited to the current pull request’s stated purpose;
* use fixup commits for review feedback and CI corrections;
* rerun relevant checks after material changes;

Do not begin the next dependent pull request while the current one remains unmerged.

### Merge policy

Use **squash merge only**.

Never use:

* merge commits;
* rebase merge;
* direct pushes to the default branch;
* or manual merging outside the pull request workflow.

Merge only after:

* the pull request has received the required approval;
* all required checks have passed;
* and there are no unresolved review comments or requested changes.

If authorized to perform the merge, use the repository’s squash-merge mechanism and delete the merged branch. Otherwise, stop after the pull request is ready and wait for an authorized maintainer to squash-merge it.

After merging, update the local default branch before starting any subsequent work.

### Decision rule

When choosing between one larger pull request and several smaller sequential pull requests, prefer the smaller sequence unless the split would create a broken, untestable, misleading, unsafe, or meaningless intermediate state.
