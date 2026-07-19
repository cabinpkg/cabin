#!/usr/bin/env bash
#
# The launch guard (docs/runbook.md, "Data policy"): destructive
# maintenance scripts run this before touching anything. It reads
# meta.launched from the database and exits 0 only when the value is
# exactly 'false'; 'true' means the registry is launched and its data
# is permanent, so the script refuses - and so does every other state
# (missing row, unreadable database, unexpected value), fail-safe.
# Flipping the flag to 'true' is a one-time launch-checklist item.
#
#   scripts/launch-guard.sh <--remote|--local>

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

mode="${1:?usage: scripts/launch-guard.sh <--remote|--local>}"
case "$mode" in
  --remote | --local) ;;
  *) echo "launch guard: unknown mode: $mode (expected --remote or --local)" >&2; exit 1 ;;
esac

refuse() { printf 'launch guard: %s\n' "$*" >&2; exit 1; }

# Remote reads go through the DB binding (wrangler resolves even a
# database NAME through the config, so the binding is the only real
# path) - but destructive commands like `d1 delete cabin-registry`
# resolve the name against the ACCOUNT. The guard therefore first
# proves the two resolutions agree: the account's database named
# cabin-registry must carry exactly the id the config binds, else it
# could read one database while a wipe deletes another. Local mode has
# no name resolution; the DB binding is the local state.
if [[ "$mode" == "--remote" ]]; then
  account_id="$(npx --yes wrangler@4.112.0 d1 list --json | node -e '
    const list = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const db = list.find((db) => db.name === "cabin-registry");
    if (!db) process.exit(1);
    console.log(db.uuid || db.database_id);
  ')" || refuse "no database named cabin-registry on the account; refusing (fail-safe)"
  config_id="$(node -e '
    const text = require("fs").readFileSync("wrangler.jsonc", "utf8");
    const m = text.match(/"database_id":\s*"([0-9a-f-]{36})"/);
    if (!m) process.exit(1);
    console.log(m[1]);
  ')" || refuse "no database_id in wrangler.jsonc; refusing (fail-safe)"
  [[ "$account_id" == "$config_id" ]] \
    || refuse "the account's cabin-registry is $account_id but wrangler.jsonc binds $config_id; refusing (fail-safe)"
fi

out="$(npx --yes wrangler@4.112.0 d1 execute DB "$mode" --json --command \
  "SELECT value FROM meta WHERE key = 'launched'")" \
  || refuse "could not read meta.launched; refusing (fail-safe)"

value="$(node -e '
  const out = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const results = out[0].results;
  console.log(results.length === 0 ? "__MISSING__" : String(results[0].value));
' <<<"$out")" || refuse "unexpected wrangler output; refusing (fail-safe)"

case "$value" in
  false) exit 0 ;;
  true) refuse "the registry is launched (meta.launched = 'true'); its data is permanent and destructive maintenance is forbidden (docs/runbook.md, \"Data policy\")" ;;
  __MISSING__) refuse "meta.launched is missing (baseline migration not applied?); refusing (fail-safe)" ;;
  *) refuse "meta.launched is '$value' (expected 'false'); refusing (fail-safe)" ;;
esac
