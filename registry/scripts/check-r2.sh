#!/usr/bin/env bash
#
# Every billable R2 call the Worker makes must be admitted by the cost
# governor before the call (docs/architecture.md, "The cost governor").
# A guard cannot prove admission lexically, so it pins the seam
# instead: every acquisition of an R2 bucket handle must sit in a
# function scripts/check-r2.pl allowlists with its exact acquisition
# count, and a new site fails CI until a reviewer confirms the
# governor admission and re-pins it. The scan is lexical
# (scripts/lexical.pm blanks comments and strings first), so no
# comment can hide an acquisition or fake one and no multi-line
# spelling slips through; tests/check_r2_guard.rs pins every case.
#
# ponytail: pins where handles are acquired, not that every use is
# admitted - a new call inside an already-pinned function passes, and
# a call assembled by a macro would pass. It is a regression tripwire
# that forces diff review at the seam; make it syntax-aware only if
# that stops holding.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

if ! find src -name '*.rs' -print0 | xargs -0 perl scripts/check-r2.pl; then
  echo "error: R2 bucket acquisition outside the pinned governor-admitting functions" >&2
  exit 1
fi
