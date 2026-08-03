#!/usr/bin/env bash
#
# Local mirror of the CI gate (rust.yml, ci.yml, website.yml). Expensive
# checks are scoped to the surfaces changed relative to origin/main, per
# AGENTS.md ("run only the checks that match the touched surface").
#
# The cargo phases and the website block are independent and share no
# build artifacts (clippy compiles under its own driver; check, test,
# and doc differ in RUSTFLAGS, feature set, or compiler), so they run
# concurrently, each cargo phase in its own target directory - cargo's
# target-dir lock is exclusive, so a shared directory would serialize
# them right back. The first run in a checkout pays a cold build in
# each directory; after that every run is incremental, and keeping the
# gate's `-D warnings` builds out of `target/` also stops them from
# invalidating ordinary iteration builds (and vice versa).
#
#   scripts/ci.sh          run the checks; exits non-zero on failure
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
  # Job control gives the inner gate its own process group, and the
  # forwarding trap tears that group down when the hook itself is
  # cancelled - without it, a killed hook leaves the gate and every
  # build under it running.  The gate runs as a single background
  # child (not a pipeline: a pipeline's group id is its FIRST
  # process's pid while `$!` names the LAST, so a group kill through
  # `$!` would miss); its output replays to stderr afterwards.
  set -m
  # Armed BEFORE the fork so no signal window exists, and enumerating
  # the job table instead of a recorded pid for the same reason the
  # inner gate's teardown does.
  hook_teardown() {
    local pid
    for pid in $(jobs -p); do
      kill -- "-$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    exit 0
  }
  trap hook_teardown TERM INT HUP
  bash scripts/ci.sh >"$log" 2>&1 &
  wait "$!" || status=$?
  trap - TERM INT HUP
  cat "$log" >&2
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

# One concurrent phase. Output goes to a log replayed only on failure,
# under a final `==> <name>` marker, so the --hook adapter's "last ==>
# line names the failed step" contract holds in both modes.
phase_names=()
phase_logs=()
phase_pids=()
phase() {
  local name="$1"
  shift
  local log
  log="$(mktemp "${TMPDIR:-/tmp}/cabin-ci-phase.XXXXXX")"
  printf '==> %s (started)\n' "$name"
  "$@" >"$log" 2>&1 &
  # Recorded before anything else so the teardown trap's window
  # without this pid is as small as bash allows.
  phase_pids+=($!)
  phase_names+=("$name")
  phase_logs+=("$log")
}

# A cancelled gate must not orphan its phases: bash exits on a
# process-directed TERM/INT/HUP without signaling background jobs,
# leaving cargo/npm running.  Phases are enumerated from bash's own
# job table, not the phase_pids array: the table is populated at
# fork time (so a signal landing before the array append still sees
# the new phase) and reaped jobs leave it (so a recycled pid is
# never signaled).  Each live phase is a process group (set -m), so
# the group kill reaps grandchildren too.  The exit code is the
# conventional 128+signal for whichever signal arrived.
terminate_phases() {
  local code="$1" pid
  for pid in $(jobs -p); do
    kill -- "-$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -f "${phase_logs[@]:-}" 2>/dev/null || true
  exit "$code"
}
trap 'terminate_phases 143' TERM
trap 'terminate_phases 130' INT
trap 'terminate_phases 129' HUP

await_phases() {
  local failed=0 i
  for i in "${!phase_pids[@]}"; do
    if wait "${phase_pids[$i]}"; then
      printf '    ok: %s\n' "${phase_names[$i]}"
      rm -f "${phase_logs[$i]}"
    else
      failed=1
      printf '\n--- failed phase output: %s ---\n' "${phase_names[$i]}"
      cat "${phase_logs[$i]}"
      rm -f "${phase_logs[$i]}"
      printf '\n==> %s\n' "${phase_names[$i]}"
    fi
  done
  return "$failed"
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
  # `ports/` counts as a Rust surface: the publisher's
  # `committed_ports_all_load` and the CLI's registry fixtures stage the
  # committed tree, so a ports-only change still has to run the Rust gate.
  grep -qE '^(crates/|examples/|ports/|Cargo\.|\.cargo/|rust-toolchain)' <<<"$changed" || rust_changed=0
  # The website build also loads the foundation ports
  # (website/src/lib/ports.ts reads ports/).
  grep -qE '^(website/|docs/|ports/)' <<<"$changed" || web_changed=0
  grep -qE '^(docs/|CONTRIBUTING\.md|INSTALL\.md)' <<<"$changed" || docs_changed=0
fi

step cargo fmt --all --verbose -- --check
step taplo fmt --check
step typos
step cargo check-scripts

if [[ -n "$base" && -n "$commits" ]]; then
  step npx --yes --package @commitlint/cli --package @commitlint/config-conventional \
    commitlint --extends @commitlint/config-conventional --from "$base" --to HEAD --verbose
fi

# Concurrent cargo invocations each default --jobs to the CPU count,
# so four unbounded phases could peak at 4N compiler jobs and swap or
# OOM a smaller host. Split the cores statically instead: the test
# phase gets half (it is the longest, and --jobs caps only its
# compilation - test-running parallelism is libtest's own), the rest a
# quarter each. The ~1.25N aggregate peak is deliberate mild
# oversubscription; phases finish at different times, and a dynamic
# jobserver shared across invocations is not worth the machinery here.
# CPUs actually available to this process, not merely online: agent
# and CI containers often run under an affinity mask (`nproc` honors
# it) or a cgroup v2 CPU quota, and a cap derived from the online
# count would overshoot exactly on the constrained hosts it exists
# for.  The quota applies at the process's own cgroup or any
# ancestor (`/proc/self/cgroup` names the path; the files live under
# the ancestors too when there is no private cgroup namespace), so
# the walk takes the tightest `cpu.max` on the way to the root.
effective_cores() {
  local cores cgroup_path node cpu_quota cpu_period quota_cores
  local mount_root mount_point rel
  cores="$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
  # Ceiling, stated once: these probes handle the common container
  # shapes - v2 or v1 hierarchies found through /proc/self/mountinfo
  # (whose root field maps a non-root hierarchy mount onto its
  # mountpoint), quotas on the process's cgroup or an ancestor.
  # Shapes beyond that (relocated roots under private cgroup
  # namespaces, escaped mount paths) degrade to the nproc count:
  # this is a local scheduling hint for a dev-machine gate, not a
  # security boundary, and nproc is the fail-safe it falls back to.
  #
  # cgroup v2: `cpu.max` at the process's cgroup or any ancestor,
  # resolved relative to the hierarchy root the mount exposes.
  read -r mount_root mount_point <<<"$(awk '{
      sep = index($0, " - "); if (sep == 0) next
      split(substr($0, sep + 3), post, " ")
      if (post[1] == "cgroup2") { print $4, $5; exit }
    }' /proc/self/mountinfo 2>/dev/null || true)"
  cgroup_path="$(sed -n 's/^0:://p' /proc/self/cgroup 2>/dev/null | head -n 1 || true)"
  if [[ -n "${mount_point:-}" ]]; then
    # Component-aware: root `/tenant` must not strip from
    # `/tenant2/job` (a textual prefix but a sibling cgroup).  A path
    # outside the mount's root degrades to the mountpoint itself.
    if [[ "$mount_root" == "/" ]]; then
      rel="$cgroup_path"
    elif [[ "$cgroup_path" == "$mount_root" || "$cgroup_path" == "$mount_root"/* ]]; then
      rel="${cgroup_path#"$mount_root"}"
    else
      rel=""
    fi
    node="$mount_point$rel"
    while [[ "$node" == "$mount_point"* ]]; do
      if [[ -r "$node/cpu.max" ]]; then
        read -r cpu_quota cpu_period < "$node/cpu.max" || true
        clamp_cores "$cpu_quota" "$cpu_period"
      fi
      [[ "$node" == "$mount_point" ]] && break
      node="${node%/*}"
    done
  fi
  # cgroup v1: the `cpu` controller's `cpu.cfs_quota_us` (-1 means
  # unlimited and fails the numeric guard), same ancestral walk and
  # the same mountinfo resolution (the controller list rides the
  # super options after the ` - ` separator).  The cgroup path keeps
  # everything after the second colon: controller names never
  # contain `:`, but a cgroup path legally can.
  read -r mount_root mount_point <<<"$(awk '{
      sep = index($0, " - "); if (sep == 0) next
      split(substr($0, sep + 3), post, " ")
      if (post[1] == "cgroup" && post[3] ~ /(^|,)cpu(,|$)/) { print $4, $5; exit }
    }' /proc/self/mountinfo 2>/dev/null || true)"
  cgroup_path="$(awk -F: 'NF >= 3 && $2 ~ /(^|,)cpu(,|$)/ { sub(/^[^:]*:[^:]*:/, ""); print; exit }' /proc/self/cgroup 2>/dev/null || true)"
  if [[ -n "${mount_point:-}" && -d "$mount_point" ]]; then
    # Component-aware: root `/tenant` must not strip from
    # `/tenant2/job` (a textual prefix but a sibling cgroup).  A path
    # outside the mount's root degrades to the mountpoint itself.
    if [[ "$mount_root" == "/" ]]; then
      rel="$cgroup_path"
    elif [[ "$cgroup_path" == "$mount_root" || "$cgroup_path" == "$mount_root"/* ]]; then
      rel="${cgroup_path#"$mount_root"}"
    else
      rel=""
    fi
    node="$mount_point$rel"
    while [[ "$node" == "$mount_point"* ]]; do
      if [[ -r "$node/cpu.cfs_quota_us" && -r "$node/cpu.cfs_period_us" ]]; then
        cpu_quota="$(cat "$node/cpu.cfs_quota_us" 2>/dev/null || true)"
        cpu_period="$(cat "$node/cpu.cfs_period_us" 2>/dev/null || true)"
        clamp_cores "$cpu_quota" "$cpu_period"
      fi
      [[ "$node" == "$mount_point" ]] && break
      node="${node%/*}"
    done
  fi
  printf '%s
' "$cores"
}
# Clamp the surrounding `cores` by ceil(quota/period) when both are
# plain numerals (v2 spells "no limit" as `max`, v1 as `-1`; both
# fail the guard and leave `cores` alone).
clamp_cores() {
  local cpu_quota="$1" cpu_period="$2" quota_cores
  if [[ "$cpu_quota" =~ ^[0-9]+$ && "$cpu_period" =~ ^[0-9]+$ && "$cpu_period" -gt 0 ]]; then
    quota_cores="$(((cpu_quota + cpu_period - 1) / cpu_period))"
    [[ "$quota_cores" -lt 1 ]] && quota_cores=1
    [[ "$quota_cores" -lt "$cores" ]] && cores="$quota_cores"
  fi
}

cores="$(effective_cores)"

# Below four cores there is nothing to split: the rounded-up shares
# would sum right back to the oversubscription the cap exists to
# prevent, so constrained hosts keep the original serial gate (which
# was already the right shape there) and every phase gets the whole
# machine.  With four or more, the split is exact: the test phase
# (the longest) takes half, and the remainder is distributed - never
# independently rounded up, which compounds (2+2+3+2 = 9 jobs on a
# 5-core host).  The shares sum to the detected capacity, except the
# 4-core floor where keeping every phase alive costs one extra.
# `launch` picks the mode so each command is written once.
if [[ "$cores" -ge 4 ]]; then
  parallel_gate=1
  test_jobs="$((cores / 2))"
  jobs_left="$((cores - test_jobs))"
  share="$((jobs_left / 3))"
  spare="$((jobs_left % 3))"
  clippy_jobs="$((share + (spare >= 1)))"
  check_jobs="$((share + (spare >= 2)))"
  doc_jobs="$share"
  [[ "$doc_jobs" -lt 1 ]] && doc_jobs=1
else
  parallel_gate=0
  test_jobs="$cores"
  clippy_jobs="$cores"
  check_jobs="$cores"
  doc_jobs="$cores"
fi

launch() {
  local name="$1"
  shift
  if [[ "$parallel_gate" -eq 1 ]]; then
    phase "$name" "$@"
  else
    step "$@"
  fi
}

# The test phases add `--test-threads` on top of CI's command shape:
# libtest's default worker count is the full CPU count, which would
# overlap the other phases (and each CLI test's spawned fixture
# build) unbounded.  A harness scheduling flag changes how fast the
# same tests run locally, never what is tested, so the CI mirror
# stays faithful where it matters; serial mode passes the full core
# count, which is libtest's default - i.e. CI's exact behavior.
# Job control from here on: every backgrounded phase becomes its own
# process group, so teardown can kill a phase's whole tree (npm's
# node children included).  Deliberately NOT enabled for the serial
# steps above - under job control even foreground commands get their
# own group (escaping a group-directed kill), and bash defers trap
# handling until the foreground command finishes.  Serial mode never
# backgrounds (`launch` degrades to foreground `step`s), so job
# control stays off there for the same reason.
[[ "$parallel_gate" -eq 1 ]] && set -m

if [[ "$rust_changed" -eq 1 ]]; then
  launch "cargo clippy (workspace, all targets, all features)" \
    env CARGO_TARGET_DIR="$PWD/target/ci-clippy" \
    cargo clippy --workspace --all-targets --all-features --locked --verbose \
    --jobs "$clippy_jobs" -- -D warnings
  launch "cargo check (workspace, all targets, -D warnings)" \
    env CARGO_TARGET_DIR="$PWD/target/ci-check" RUSTFLAGS="-D warnings" \
    cargo check --workspace --all-targets --locked --verbose --jobs "$check_jobs"
  # `cargo-nextest` runs the same test set (the phase excludes
  # doctests either way, via `--all-targets`) but schedules each test
  # in its own process instead of one binary at a time, which is the
  # bulk of this phase's wall clock.  It is an optional accelerator:
  # without it installed the phase runs CI's exact command.
  if command -v cargo-nextest >/dev/null 2>&1; then
    launch "cargo nextest (workspace, all targets, all features)" \
      env CARGO_TARGET_DIR="$PWD/target/ci-test" RUSTFLAGS="-D warnings" \
      cargo nextest run --workspace --all-targets --all-features --locked \
      --no-fail-fast --cargo-verbose --build-jobs "$test_jobs" \
      --test-threads "$test_jobs"
  else
    launch "cargo test (workspace, all targets, all features)" \
      env CARGO_TARGET_DIR="$PWD/target/ci-test" RUSTFLAGS="-D warnings" \
      cargo test --workspace --all-targets --all-features --locked --no-fail-fast \
      --verbose --jobs "$test_jobs" -- --show-output --test-threads "$test_jobs"
  fi
  launch "cargo doc (workspace, no deps, -D warnings)" \
    env CARGO_TARGET_DIR="$PWD/target/ci-doc" RUSTDOCFLAGS="-D warnings" \
    cargo doc --workspace --all-features --no-deps --locked --verbose --jobs "$doc_jobs"
else
  echo "skipping clippy/check/test/doc: no Rust changes since main"
  if [[ "$docs_changed" -eq 1 ]]; then
    # The cli integration tests embed doc pages via include_str! (the
    # crates/cabin/tests/cli/*_docs.rs convention) and assert on their
    # contents, so doc edits can fail Rust CI.
    if command -v cargo-nextest >/dev/null 2>&1; then
      launch "cargo nextest -p cabinpkg --test cli (docs)" \
        env CARGO_TARGET_DIR="$PWD/target/ci-test" RUSTFLAGS="-D warnings" \
        cargo nextest run -p cabinpkg --test cli --all-features --locked \
        --no-fail-fast --cargo-verbose --build-jobs "$test_jobs" \
        --test-threads "$test_jobs" docs
    else
      launch "cargo test -p cabinpkg --test cli (docs)" \
        env CARGO_TARGET_DIR="$PWD/target/ci-test" RUSTFLAGS="-D warnings" \
        cargo test -p cabinpkg --test cli --all-features --locked --no-fail-fast \
        --verbose --jobs "$test_jobs" -- --show-output --test-threads "$test_jobs" docs
    fi
  fi
fi

if [[ "$web_changed" -eq 1 ]]; then
  # `npm test` runs here too, matching website.yml: it is the only
  # check over src/lib/, so omitting it let the local gate print
  # "local CI green" on a change that lands red in CI.
  launch "npm ci && npm run lint && npm test && npm run build (website/)" \
    bash -c 'cd website && npm ci && npm run lint && npm test && npm run build'
else
  echo "skipping website lint/test/build: no website/, docs/ or ports/ changes since main"
fi

if [[ "${#phase_pids[@]}" -gt 0 ]]; then
  await_phases
fi

echo "local CI green"
