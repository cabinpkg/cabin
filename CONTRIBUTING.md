# Contributing to Cabin

Thanks for your interest in contributing to Cabin. This document
covers local setup, required checks, and PR workflow. The canonical
crate-boundary, scope, and ownership rules live in
[`docs/architecture.md`](docs/architecture.md); do not duplicate
them here.

## Prerequisites

- A recent stable Rust toolchain.
- `rustfmt` and `clippy` components installed.
- [`taplo`](https://taplo.tamasfe.dev/) for TOML formatting.
- For end-to-end build coverage: **Ninja** 1.10+, a **C++
  compiler** (`g++`, `clang++`, or `c++`), and a **C compiler**
  (`gcc`, `clang`, or `cc`) for tests that exercise `.c` sources.

The unit tests in every crate, plus the resolution / lockfile
integration tests, do not require Ninja or C/C++ compilers. The CLI
build integration tests skip themselves gracefully when those tools
are missing.

## Setup

```sh
git clone https://github.com/cabinpkg/cabin.git
cd cabin
cargo build --workspace
```

## Required checks

```sh
cargo fmt --all --verbose -- --check
taplo fmt --check
typos
cargo clippy --workspace --all-targets --all-features --locked --verbose -- -D warnings
cargo check --workspace --all-targets --locked --verbose
cargo test --workspace --all-targets --all-features --locked --verbose -- --show-output
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --verbose

# Conventional-commit lint of the commits this branch adds.
# Mirrors CI's @commitlint/config-conventional gate; every commit
# header must be a valid conventional commit and stay <= 100 chars.
npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
  commitlint --extends @commitlint/config-conventional --from origin/main --to HEAD --verbose
```

The Rust CI workflow runs the Rust commands above and treats
warnings as errors; a separate CI job runs the `commitlint`
command above against the PR's commits. Mirror the flags verbatim
when running locally, including:

- `--all-features` on both `cargo clippy` and `cargo doc` —
  cabin gates several modules behind features, and dropping the
  flag hides lints and broken intra-doc links that CI still
  fires on;
- the trailing `-- -D warnings` on `cargo clippy` (the
  `clippy::pedantic` group is denied workspace-wide via
  `[workspace.lints]` in the root `Cargo.toml`, so it no longer
  needs a command-line flag);
- the `RUSTDOCFLAGS="-D warnings"` environment variable on
  `cargo doc`, so broken or redundant docs links fail locally
  rather than only in CI;
- `--locked`, which pins the resolution to the committed
  `Cargo.lock`. Reviewers will reject PRs that silently bump
  transitive dependency versions.

The repository's `typos.toml` pins the project locale to American
English; do not modify it (including adding new `extend-words`
entries) unless a reviewer explicitly asks for the change. If
`typos` flags a spelling, fix the offending occurrence instead of
allowlisting it.

The separate CI workflow also runs workflow linting and
commit-message linting. Commit subjects are validated with
`commitlint` against `@commitlint/config-conventional`, so every
subject must follow [Conventional Commits](https://www.conventionalcommits.org/)
(`<type>(<scope>)?: <subject>`, lower-case subject, ≤ 100
characters). Body and footer lines, if present, must also stay
≤ 100 characters per line. See the "Commit messages" section of
[`AGENTS.md`](AGENTS.md) for the full rule set.

The test suite includes external-tool smoke tests for `ninja`,
`clang-format`, `run-clang-tidy`, and `pkg-config`.
Those tests fail by default when the real tools are missing.  For
local environments that intentionally lack the tools, set
`CABIN_SKIP_EXTERNAL_TOOL_TESTS=1` to route only those smoke tests
through the bundled fake-tool binaries.

## Code style

- Idiomatic Rust.  Prefer simple, direct code over clever
  abstractions.
- Follow the diagnostic and crate-boundary rules in
  [`docs/architecture.md`](docs/architecture.md).
- Avoid `unwrap()` / `expect()` outside of tests except where
  invariants are obvious and locally proven.
- Public APIs stay small.  Add a doc comment when the reason a type
  or function exists is not obvious from its signature.
- Tests live next to the code they exercise.  CLI integration tests
  live in `crates/cabin/tests/cli.rs` and exercise the compiled
  `cabin` binary via `assert_cmd`.  The user-facing example projects
  under `examples/` are exercised by
  `crates/cabin/tests/cabin_examples.rs` using the same pattern.

## Architectural rules

Read [`docs/architecture.md`](docs/architecture.md) before changing
crate boundaries, command ownership, scope, diagnostics, generated
formats, or build / registry / resolver behavior. When in doubt, the
architecture document wins.

## Pull requests

- **Keep PRs focused.**  One change per PR is easier to review and
  to revert.
- **Add tests for behavior changes.**  New workspace, resolver,
  or build logic should land with unit tests in the owning crate
  plus a CLI integration test in `crates/cabin/tests/cli.rs`.
- **Update documentation when architecture or behavior changes.**
  Update the relevant [`docs/`](docs/) page. If you move code across
  crates, update [`docs/architecture.md`](docs/architecture.md) and
  [`AGENTS.md`](AGENTS.md).
- **Update the website when user-facing positioning changes.**
  Copy on [`website/`](website/) (taglines, supported languages,
  supported platforms, top-level command surface, package-page
  install snippet) does not auto-regenerate from the Rust crates.
  A change that adjusts what Cabin is, what it builds, or how a
  user installs / declares / publishes a package must update
  `website/` in the same PR. See the
  "Keeping docs, AGENTS.md, and the website in sync" section of
  [`AGENTS.md`](AGENTS.md) for the per-area checklist.

If you are unsure whether something belongs to the current scope,
open an issue first or ask in the PR description rather than
implementing it.
