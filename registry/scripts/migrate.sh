#!/usr/bin/env bash
#
# Apply D1 migrations and keep the deploy gate's stamp honest
# (docs/runbook.md, "Integrated topology and route management"): the
# CI deploy stays skipped while migrations/ disagrees with
# migrations-applied, and the stamp must only ever be refreshed after
# the live database really runs the files' content.
#
#   scripts/migrate.sh --local    apply to the local .wrangler/ state
#   scripts/migrate.sh --remote   apply to the live database, then stamp
#
# Only --remote touches the stamp: it attests the LIVE schema, and a
# local apply proves nothing about production. The applied set is read
# from D1's own bookkeeping (the d1_migrations table, by filename), so
# the script refuses every state a stamp refresh would wrongly certify:
# an already-applied file edited in place (D1 never replays it), and a
# recorded applied migration whose file was renamed or removed (its
# effects live on in the schema while the files pretend otherwise).
# Both route through scripts/wipe.sh pre-launch (drop, recreate, apply
# from zero); post-launch, schema changes are only ever NEW files.
# CABIN_MIGRATE_YES=1 skips the remote confirmation prompt.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."
. scripts/lib.sh

mode="${1:?usage: scripts/migrate.sh <--remote|--local>}"
case "$mode" in
  --remote | --local) ;;
  *) echo "usage: scripts/migrate.sh <--remote|--local>" >&2; exit 1 ;;
esac

if [[ "$mode" == "--local" ]]; then
  step "applying migrations to the local database"
  wrangler d1 migrations apply DB --local
  echo "local migrate OK (the migrations-applied stamp tracks the live"
  echo "database only; a local apply never touches it)"
  exit 0
fi

# One recorded-migration name per line, from D1's own bookkeeping. An
# absent d1_migrations table is the never-migrated database (first
# provisioning); any other failure refuses - an unreadable applied set
# must never read as an empty one.
applied_names() {
  local has_table
  has_table="$(wrangler d1 execute DB --remote --json --command \
    "SELECT COUNT(*) AS n FROM sqlite_master
     WHERE type = 'table' AND name = 'd1_migrations'" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      console.log(out[0].results[0].n);
    ')" || fail "could not read the live database's migration bookkeeping"
  [[ "$has_table" == "0" ]] && return 0
  wrangler d1 execute DB --remote --json --command \
    "SELECT name FROM d1_migrations ORDER BY name" |
    node -e '
      const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
      for (const row of out[0].results) console.log(row.name);
    ' || fail "could not read the live database's applied-migration names"
}

step "reading the applied set from the live database"
applied="$(applied_names)"
pending=0
applied_files=()
for file in migrations/*.sql; do
  if grep -qxF "$(basename "$file")" <<<"$applied"; then
    applied_files+=("$file")
  else
    pending=$((pending + 1))
  fi
done
printf '    %s applied, %s pending migration file(s)\n' "${#applied_files[@]}" "$pending"

# Every name D1 recorded must still exist as a file: a renamed or
# removed applied migration leaves its effects in the live schema
# while the files no longer describe them, and no stamp may certify
# that. (The wipe procedure is the pre-launch reset for this state.)
while IFS= read -r name; do
  [[ -z "$name" ]] && continue
  [[ -f "migrations/$name" ]] \
    || fail "D1 records applied migration '$name' but migrations/$name is gone
(renamed or removed). The live schema still carries its effects; restore
the file, or reset pre-launch via scripts/wipe.sh."
done <<<"$applied"

# The already-applied files must still hash to the recorded stamp
# before anything new is applied: D1 tracks applied migrations by
# FILENAME, so an in-place edit of an applied file would never replay,
# and refreshing the aggregate stamp after applying only new files
# would certify a live schema that does not match migrations/. A
# database with nothing applied yet has nothing to have been edited.
stamp="$(cat migrations/*.sql | shasum -a 256 | cut -d' ' -f1)"
applied_stamp="$(cat migrations-applied)"
if [[ "${#applied_files[@]}" -gt 0 ]] \
  && [[ "$(cat "${applied_files[@]}" | shasum -a 256 | cut -d' ' -f1)" != "$applied_stamp" ]]; then
  fail "an already-applied migration file was edited in place (the applied
files no longer hash to the migrations-applied stamp). D1 will NOT
replay it, and stamping would unblock deploys against a stale live
schema. Pre-launch, the edited baseline ships through scripts/wipe.sh;
post-launch, write a NEW migration file instead of editing an applied
one."
fi

if [[ "$pending" -eq 0 ]]; then
  echo "remote migrate OK (nothing pending; the stamp is already current)"
  exit 0
fi

# Every pending file must sort after every applied one: a fresh
# database replays migrations/*.sql in glob order, so a new file
# sorting before an applied one would give the live database and a
# rebuilt one different histories under the same stamp.
if [[ "${#applied_files[@]}" -gt 0 ]]; then
  last_applied="$(basename "${applied_files[${#applied_files[@]} - 1]}")"
  for file in migrations/*.sql; do
    name="$(basename "$file")"
    if ! grep -qxF "$name" <<<"$applied" && [[ "$name" < "$last_applied" ]]; then
      fail "pending migration $name sorts before applied $last_applied; a
fresh database would replay them in a different order than the live one
ran. Name new migrations after every applied one."
    fi
  done
fi

if [[ "${CABIN_MIGRATE_YES:-}" != "1" ]]; then
  printf 'About to apply %s migration file(s) to the LIVE database. Type "migrate" to confirm: ' "$pending"
  read -r answer
  [[ "$answer" == "migrate" ]] || fail "not confirmed"
fi

step "applying migrations to the live database"
wrangler d1 migrations apply DB --remote

step "verifying the live database now records every migration file"
applied="$(applied_names)"
for file in migrations/*.sql; do
  grep -qxF "$(basename "$file")" <<<"$applied" \
    || fail "$(basename "$file") is not recorded as applied after the apply; do not stamp"
done

step "refreshing the migrations-applied stamp"
printf '%s\n' "$stamp" >migrations-applied

cat <<EOF
remote migrate OK (stamp $stamp)

Follow-ups:
  - commit the migrations-applied change; the CI deploy stays skipped
    until it reaches main (docs/runbook.md, "Integrated topology")
EOF
