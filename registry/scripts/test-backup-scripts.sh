#!/usr/bin/env bash
#
# Self-checks for the backup scripts: shellcheck over scripts/, the pure
# retention-plan core on fixed fixtures, the prune executor in --dry-run
# and delete mode against a fake rclone, and the dump verifier on good and
# broken dumps. Pure: no network, no real rclone. Run by CI (registry.yml)
# and by hand:
#
#   scripts/test-backup-scripts.sh

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Only the backup scripts: the pre-existing scripts are linted by hand, and
# CI runner shellcheck versions differ from local ones.
step "shellcheck backup scripts"
shellcheck ./backup-verify-dump.sh ./backup-prune.sh ./restore-drill.sh \
  ./test-backup-scripts.sh

step "prune plan: fewer than 30 dumps -> nothing pruned"
out="$(seq -f '2026-07-%02.0f' 1 9 | ./backup-prune.sh plan 2026-07-09)"
[[ -z "$out" ]] || fail "expected empty plan, got: $out"

step "prune plan: keeps 30 recent + monthly firsts, prunes the rest"
{
  seq -f '2026-06-%02.0f' 1 30
  seq -f '2026-07-%02.0f' 1 9
  echo 2025-08-03      # first dump of a month inside the 12-month window
  echo 2025-08-10      # same month, not the first -> pruned
  echo 2025-06-15      # 13 months old -> pruned even though first of month
} | sort -u >"$tmp/dates"
expected="$(printf '2025-06-15\n2025-08-10\n'; seq -f '2026-06-%02.0f' 2 9)"
out="$(./backup-prune.sh plan 2026-07-09 <"$tmp/dates" | sort)"
[[ "$out" == "$(sort <<<"$expected")" ]] \
  || fail "unexpected plan: $(tr '\n' ' ' <<<"$out")"

step "prune plan: january window wraps into the previous year"
{
  seq -f '2025-12-%02.0f' 17 31
  seq -f '2026-01-%02.0f' 1 15
  echo 2025-02-25      # outside the 30 recent, but 2025-02 is in the window
  echo 2024-12-20      # 2024-12 fell out of the window -> pruned
} | sort -u >"$tmp/dates"
out="$(./backup-prune.sh plan 2026-01-15 <"$tmp/dates")"
[[ "$out" == "2024-12-20" ]] \
  || fail "expected only 2024-12-20 pruned, got: $out"

# Fakes so the executor path (listing, sidecar pairing, dry-run vs delete)
# runs deterministically and without a network: rclone lsf prints the
# fixture listing, rclone deletefile appends to a log, and date is pinned.
mkdir "$tmp/bin"
cat >"$tmp/bin/rclone" <<EOF
#!/usr/bin/env bash
case "\$1" in
  lsf) cat "$tmp/listing" ;;
  deletefile) echo "\$2" >>"$tmp/deleted" ;;
  *) echo "fake rclone: unexpected: \$*" >&2; exit 1 ;;
esac
EOF
printf '#!/usr/bin/env bash\necho 2026-07-09\n' >"$tmp/bin/date"
chmod +x "$tmp/bin/rclone" "$tmp/bin/date"

# The dumps of 2026-06-01..30 + 2026-07-01..09, each with a sidecar, as of
# 2026-07-09: prunable dates are 2026-06-02..09 (see the plan test above).
{
  while IFS= read -r d; do
    echo "$d.sql.gz"
    echo "$d.sql.gz.sha256"
  done < <(seq -f '2026-06-%02.0f' 1 30; seq -f '2026-07-%02.0f' 1 9)
  echo "unrelated.txt"   # never touched
} >"$tmp/listing"
expected_files="$(while IFS= read -r d; do
  echo "r2:bucket/d1/$d.sql.gz"
  echo "r2:bucket/d1/$d.sql.gz.sha256"
done < <(seq -f '2026-06-%02.0f' 2 9))"
expected_dry="$(while IFS= read -r f; do
  echo "would delete: $f"
done <<<"$expected_files")"

step "prune --dry-run: prints, deletes nothing"
out="$(PATH="$tmp/bin:$PATH" ./backup-prune.sh --dry-run r2:bucket/d1)"
[[ "$out" == "$expected_dry" ]] || fail "unexpected dry-run output: $out"
[[ ! -e "$tmp/deleted" ]] || fail "dry-run called rclone deletefile"

step "prune: deletes exactly the pruned dumps and their sidecars"
PATH="$tmp/bin:$PATH" ./backup-prune.sh r2:bucket/d1 >/dev/null
[[ "$(cat "$tmp/deleted")" == "$expected_files" ]] \
  || fail "unexpected deletions: $(cat "$tmp/deleted")"

step "verify-dump: accepts a complete dump"
for table in users tokens packages versions meta; do
  echo "CREATE TABLE $table (id INTEGER PRIMARY KEY);"
done >"$tmp/good.sql"
echo "INSERT INTO meta (id) VALUES (1);" >>"$tmp/good.sql"
./backup-verify-dump.sh "$tmp/good.sql" >/dev/null

step "verify-dump: rejects empty, incomplete, and broken dumps"
: >"$tmp/empty.sql"
! ./backup-verify-dump.sh "$tmp/empty.sql" 2>/dev/null \
  || fail "accepted an empty dump"
grep -v 'CREATE TABLE meta' "$tmp/good.sql" >"$tmp/missing.sql"
! ./backup-verify-dump.sh "$tmp/missing.sql" 2>/dev/null \
  || fail "accepted a dump with a missing table"
{ cat "$tmp/good.sql"; echo "INSERT INTO nonexistent VALUES (1);"; } >"$tmp/broken.sql"
! ./backup-verify-dump.sh "$tmp/broken.sql" 2>/dev/null \
  || fail "accepted a dump that does not replay"

echo "backup script tests OK"
