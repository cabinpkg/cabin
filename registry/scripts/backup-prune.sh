#!/usr/bin/env bash
#
# Retention pruning for the D1 dump prefix of a backup bucket: keep the 30
# most recent daily dumps plus the first dump of each of the last 12
# calendar months, delete the rest (each dump's .sha256 sidecar follows its
# dump). See docs/runbook.md ("Disaster recovery").
#
#   scripts/backup-prune.sh [--dry-run] <rclone-remote:bucket/prefix>
#
# The rclone remote must already be configured (the backup workflow does it
# with RCLONE_CONFIG_* environment variables). The pure decision core is
# exposed for the tests as `plan`: unique YYYY-MM-DD dates on stdin, dates
# to delete on stdout.
#
#   scripts/backup-prune.sh plan <today>    # e.g. plan 2026-07-09

set -euo pipefail

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

plan() {
  local today="$1"
  [[ "$today" =~ ^([0-9]{4})-([0-9]{2})-[0-9]{2}$ ]] \
    || fail "plan: not a YYYY-MM-DD date: $today"
  local year=$((10#${BASH_REMATCH[1]})) month=$((10#${BASH_REMATCH[2]}))

  # The last 12 calendar months, current month included.
  local months=() i
  for ((i = 0; i < 12; i++)); do
    months+=("$(printf '%04d-%02d' "$year" "$month")")
    month=$((month - 1))
    if ((month == 0)); then
      month=12
      year=$((year - 1))
    fi
  done

  local dates=() d
  while IFS= read -r d; do
    if [[ -n "$d" ]]; then dates+=("$d"); fi
  done < <(sort -ru)
  if ((${#dates[@]} == 0)); then return 0; fi

  # Keep the 30 most recent dumps...
  local keep=" "
  for d in "${dates[@]:0:30}"; do keep+="$d "; done
  # ...plus the first (oldest) dump of each month in the window.
  local m first
  for m in "${months[@]}"; do
    first=""
    for d in "${dates[@]}"; do
      if [[ "$d" == "$m"-* ]]; then first="$d"; fi
    done
    if [[ -n "$first" ]]; then keep+="$first "; fi
  done

  for d in "${dates[@]}"; do
    if [[ "$keep" != *" $d "* ]]; then echo "$d"; fi
  done
}

dry_run=""
if [[ "${1:-}" == "--dry-run" ]]; then
  dry_run=1
  shift
fi
if [[ "${1:-}" == "plan" ]]; then
  plan "${2:?usage: scripts/backup-prune.sh plan <today>}"
  exit 0
fi

remote="${1:?usage: scripts/backup-prune.sh [--dry-run] <remote:bucket/prefix>}"

listing="$(rclone lsf --files-only "$remote")"
prune="$(sed -nE 's/^([0-9]{4}-[0-9]{2}-[0-9]{2})\.sql\.gz(\.sha256)?$/\1/p' \
           <<<"$listing" | sort -u | plan "$(date -u +%F)")"
if [[ -z "$prune" ]]; then
  echo "nothing to prune"
  exit 0
fi

# Delete every listed file whose date is pruned; names that do not look
# like a dump (or its sidecar) are never touched.
while IFS= read -r file; do
  [[ "$file" =~ ^([0-9]{4}-[0-9]{2}-[0-9]{2})\.sql\.gz(\.sha256)?$ ]] || continue
  grep -qxF "${BASH_REMATCH[1]}" <<<"$prune" || continue
  if [[ -n "$dry_run" ]]; then
    echo "would delete: $remote/$file"
  else
    echo "deleting: $remote/$file"
    rclone deletefile "$remote/$file"
  fi
done <<<"$listing"
