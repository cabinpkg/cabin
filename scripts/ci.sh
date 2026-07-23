#!/usr/bin/env bash
#
# Local mirror of the CI gate (rust.yml, ci.yml, website.yml). Expensive
# checks are scoped to the surfaces changed relative to origin/main, per
# AGENTS.md ("run only the checks that match the touched surface").
#
#   scripts/ci.sh          run the checks; exits non-zero on the first failure
#   scripts/ci.sh --hook   agent Stop-hook adapter (Claude Code / Codex):
#                          reads the hook JSON on stdin, always exits 0, and
#                          prints {} on success or a "block" decision naming
#                          the failed step

set -euo pipefail

cd "$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"

if [[ "${1:-}" == "--hook" ]]; then
  input="$(cat || true)"
  log="${TMPDIR:-/tmp}/cabin-ci-hook.$$.log"
  trap 'rm -f "$log"' EXIT
  status=0
  bash scripts/ci.sh 2>&1 | tee "$log" >&2 || status=$?
  if [[ "$status" -eq 0 ]]; then
    printf '{}\n'
    exit 0
  fi
  # The reason stays a fixed ASCII template on purpose: embedding compiler
  # output would require JSON-escaping arbitrary text, and one bad escape
  # makes the whole hook output invalid.
  step="$(grep '^==> ' "$log" | tail -n 1 | cut -c5- || true)"
  step="${step:-scripts/ci.sh (failed before the first step)}"
  # One blocked stop per natural stop: stop_hook_active means we already
  # blocked once, and blocking again on an unfixable failure would loop the
  # agent through the full gate forever.
  if grep -q '"stop_hook_active"[[:space:]]*:[[:space:]]*true' <<<"$input"; then
    printf '{"systemMessage":"scripts/ci.sh is still failing at: %s (stop allowed to avoid a hook loop; rerun it manually)"}\n' "$step"
  else
    printf '{"decision":"block","reason":"Local CI failed at: %s. Run bash scripts/ci.sh, fix the failures, and rerun it until it passes before stopping."}\n' "$step"
  fi
  exit 0
fi

step() {
  printf '==> %s\n' "$*"
  "$@"
}

base="$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || true)"
rust_changed=1
web_changed=1
docs_changed=1
if [[ -n "$base" ]]; then
  changed="$(git diff --name-only "$base" --; git ls-files --others --exclude-standard)"
  commits="$(git rev-list "$base..HEAD")"
  if [[ -z "$changed" && -z "$commits" ]]; then
    echo "no changes since $(git rev-parse --short "$base"); nothing to check"
    exit 0
  fi
  grep -qE '^(crates/|examples/|Cargo\.|\.cargo/|rust-toolchain)' <<<"$changed" || rust_changed=0
  # The website build also loads the foundation-port recipes
  # (website/src/lib/ports.ts reads crates/cabin-port/ports/).
  grep -qE '^(website/|docs/|crates/cabin-port/ports/)' <<<"$changed" || web_changed=0
  grep -qE '^(docs/|CONTRIBUTING\.md|INSTALL\.md)' <<<"$changed" || docs_changed=0
fi

step cargo fmt --all --verbose -- --check
step taplo fmt --check
step typos

if [[ -n "$base" && -n "$commits" ]]; then
  step npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
    commitlint --extends @commitlint/config-conventional --from "$base" --to HEAD --verbose
fi

if [[ "$rust_changed" -eq 1 ]]; then
  step cargo clippy --workspace --all-targets --all-features --locked --verbose -- -D warnings
  step env RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --locked --verbose
  step env RUSTFLAGS="-D warnings" cargo test --workspace --all-targets --all-features --locked --verbose -- --show-output
  step env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked --verbose
else
  echo "skipping clippy/check/test/doc: no Rust changes since main"
  if [[ "$docs_changed" -eq 1 ]]; then
    # The cli integration tests embed doc pages via include_str! (the
    # crates/cabin/tests/cli/*_docs.rs convention) and assert on their
    # contents, so doc edits can fail Rust CI.
    step env RUSTFLAGS="-D warnings" cargo test -p cabinpkg --test cli --all-features --locked --verbose -- --show-output docs
  fi
fi

if [[ "$web_changed" -eq 1 ]]; then
  (cd website &&
    step npm ci &&
    step npm run lint &&
    step npm run build)
else
  echo "skipping website lint/build: no website/ or docs/ changes since main"
fi

echo "local CI green"
