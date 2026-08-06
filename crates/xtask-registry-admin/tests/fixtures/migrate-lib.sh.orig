# Shared helpers for the registry scripts; sourced (never executed)
# after each script's cd to the registry root. The wrangler function
# is the single home of the pinned version for the shell; the
# independent pins in .github/workflows/registry.yml (wranglerVersion)
# and crates/xtask-registry-admin (WRANGLER) must move with it - that
# crate's tests read this file and the workflow and assert all three
# agree. The check-*.sh guards stay self-contained on purpose: their
# regression tests copy each guard alone into a scratch tree, where a
# sourced lib would be missing.

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

wrangler() { npx --yes wrangler@4.112.0 "$@"; }
