#!/usr/bin/env bash
#
# Static deploy-configuration guard (docs/runbook.md, "The cost
# governor", "Deploy notes"): proves wrangler.jsonc still declares the
# bindings, Durable Object lifecycle, crons, and parsable hard limits
# the Worker code depends on - the failures `wrangler deploy` would
# otherwise surface only against production, or (for a typo'd
# GOVERNOR_* var) not surface at all: a set-but-unparsable hard limit
# fails closed to a ZERO limit and blocks its pool loudly.
#
#   scripts/check-deploy.sh                    validate the config
#   scripts/check-deploy.sh --require-bundle   also require build/index.js
#
# When build/index.js exists (worker-build already ran), the guard also
# proves the bundle exports every bound Durable Object class - a class
# that compiles but is not exported fails only at deploy time with
# wrangler's export error, after CI is already green. --require-bundle
# (CI, after the Worker build step) makes a missing bundle a failure
# instead of a skip. Ceiling: the export check is a lexical scan of the
# bundle's `export{...}` lists; worker-build changing its output shape
# would break the scan loudly (the bound classes stop matching), never
# silently pass a missing export.

set -euo pipefail

cd "$(dirname -- "${BASH_SOURCE[0]}")/.."

require_bundle=""
if [[ "${1:-}" == "--require-bundle" ]]; then
  require_bundle=1
elif [[ -n "${1:-}" ]]; then
  echo "usage: scripts/check-deploy.sh [--require-bundle]" >&2
  exit 1
fi

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

if [[ -n "$require_bundle" && ! -f build/index.js ]]; then
  fail "build/index.js is missing; run worker-build before this guard (--require-bundle)"
fi

printf '==> validating wrangler.jsonc against the code'\''s deploy assumptions\n'
node - <<'JS' || fail "the deploy configuration no longer matches the code's assumptions"
const fs = require("fs");

// Comment/string-aware JSONC strip: a // or /* inside a string starts
// no comment (the config carries URLs and cron expressions).
function stripJsonc(text) {
  let out = "", i = 0;
  while (i < text.length) {
    const two = text.slice(i, i + 2);
    if (text[i] === '"') {
      out += text[i++];
      while (i < text.length && text[i] !== '"') {
        if (text[i] === "\\") { out += text[i++]; }
        out += text[i++];
      }
      out += text[i++] ?? "";
    } else if (two === "//") {
      while (i < text.length && text[i] !== "\n") i++;
    } else if (two === "/*") {
      i += 2;
      while (i < text.length && text.slice(i, i + 2) !== "*/") i++;
      i += 2;
    } else {
      out += text[i++];
    }
  }
  return out;
}

const failures = [];
const check = (ok, message) => { if (!ok) failures.push(message); };

let config;
try {
  config = JSON.parse(stripJsonc(fs.readFileSync("wrangler.jsonc", "utf8")));
} catch (err) {
  console.error(`wrangler.jsonc does not parse as JSONC: ${err.message}`);
  process.exit(1);
}

// The bindings the Worker code looks up by name (src/glue.rs,
// src/web_glue.rs, src/backup_glue.rs, src/governor_client.rs).
const d1 = config.d1_databases ?? [];
const boundDb = d1.find((db) => db.binding === "DB");
check(boundDb?.database_name === "cabin-registry",
  "d1_databases must bind DB to cabin-registry");
// The wipe and migrate scripts hash and certify migrations/*.sql; a
// drifted migrations_dir would have wrangler applying other files
// than the ones the stamp certifies.
check(boundDb?.migrations_dir === "migrations",
  "the DB binding's migrations_dir must stay migrations");
const r2 = config.r2_buckets ?? [];
check(r2.some((b) => b.binding === "BLOBS" && b.bucket_name === "cabin-registry-blobs"),
  "r2_buckets must bind BLOBS to cabin-registry-blobs");
check(r2.some((b) => b.binding === "BACKUP" && b.bucket_name === "cabin-registry-backup"),
  "r2_buckets must bind BACKUP to cabin-registry-backup");
const doBindings = config.durable_objects?.bindings ?? [];
check(doBindings.some((b) => b.name === "GOVERNOR" && b.class_name === "Governor"),
  "durable_objects must bind GOVERNOR to class Governor");

// The nightly dump exports whatever database D1_DATABASE_ID names; a
// value diverging from the DB-bound database backs up the wrong one
// (docs/runbook.md, "Wipe procedure").
check(boundDb !== undefined && config.vars?.D1_DATABASE_ID === boundDb.database_id,
  "vars.D1_DATABASE_ID must mirror the DB binding's database_id");

// The scheduled handler routes on the exact breaker expression; any
// other schedule runs the nightly dump (src/glue.rs). The daily dump
// cadence is pinned literally - a monthly rehearsal schedule may be
// ADDED, but replacing 0 3 * * * would quietly stretch the documented
// <= 24 h metadata RPO (docs/runbook.md, "Disaster recovery").
const crons = config.triggers?.crons ?? [];
check(crons.includes("*/15 * * * *"),
  "triggers.crons must contain the breaker's exact */15 * * * *");
check(crons.includes("0 3 * * *"),
  "triggers.crons must contain the nightly dump's exact 0 3 * * *");

// Durable Object lifecycle: every locally-bound class must be
// introduced by the migrations chain and never deleted - a deleted
// class destroys its SQLite storage, and the governor's monthly
// operation windows cannot be rebuilt (docs/runbook.md, "Deploy
// notes"). Wrangler's newer `exports` lifecycle must not coexist with
// the `migrations` array (the platform accepts only one flow).
check(!("exports" in config),
  "config mixes the exports DO lifecycle with the migrations array");
const migrations = config.migrations ?? [];
const tags = migrations.map((m) => m.tag ?? "");
check(tags.every((tag) => tag !== ""), "every migration needs a non-empty tag");
check(new Set(tags).size === tags.length, "migration tags must be unique");
// Applied DO migrations are immutable on the platform: editing an
// already-deployed tag passes any graph check but never replays, so
// the deployed history's first entry is pinned literally. Changing it
// must be a conscious review, not a drive-by edit.
check(JSON.stringify(migrations[0]) === '{"tag":"v1","new_sqlite_classes":["Governor"]}',
  "migrations[0] must stay the deployed v1 Governor migration verbatim");
// deleted_classes is banned outright, not just for the bound name: a
// rename away and a later delete of the renamed class would destroy
// the same storage while the bound name looks freshly introduced.
// Deleting a class is never routine here; if it ever becomes
// necessary, this guard is edited in the same conscious review.
check(migrations.every((m) => (m.deleted_classes ?? []).length === 0),
  "deleted_classes is forbidden: a class delete destroys Durable Object storage");
const localClasses = doBindings
  .filter((b) => !b.script_name) // a foreign class is not ours to migrate
  .map((b) => b.class_name);
for (const className of localClasses) {
  let name = className;
  let introduced = false;
  // Walk the chain backwards: the bound name may be the `to` of renames.
  for (const migration of [...migrations].reverse()) {
    const renamed = (migration.renamed_classes ?? []).find((r) => r.to === name);
    if (renamed) name = renamed.from;
    if ((migration.new_sqlite_classes ?? []).includes(name)
      || (migration.new_classes ?? []).includes(name)) introduced = true;
  }
  check(introduced, `bound class ${className} is never introduced by a migration`);
}

// Hard-limit preflight: a GOVERNOR_* var that does not parse fails
// closed to a zero limit in production (src/governor.rs) - correct
// there, but the typo belongs to CI, not to a blocked pool. BUDGET_*
// vars fall back to defaults instead, silently ignoring the intended
// override - the same class of typo, caught the same way. Names are
// checked against the exact sets the Rust code reads: a misspelled
// name parses fine and is silently ignored, which is the worst
// failure mode of all (the operator believes the override is live).
// A value past u64::MAX fails Rust's parse and lands on the same
// fail-closed zero, so the range is checked too.
const knownLimitVars = new Set([
  // src/governor.rs storage_env_var / op_env_var
  "GOVERNOR_STORAGE_PRIMARY_BYTES", "GOVERNOR_STORAGE_BACKUP_BYTES",
  "GOVERNOR_STORAGE_DUMP_BYTES", "GOVERNOR_R2_CLASS_A_PUBLISH_MONTH",
  "GOVERNOR_R2_CLASS_A_INFRA_MONTH", "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH",
  "GOVERNOR_R2_CLASS_B_SOURCE_MONTH", "GOVERNOR_R2_CLASS_B_VERIFIER_MONTH",
  "GOVERNOR_R2_CLASS_B_PUBLISH_MONTH", "GOVERNOR_R2_CLASS_B_INFRA_MONTH",
  // src/breaker.rs budgets
  "BUDGET_R2_STORAGE_BYTES", "BUDGET_R2_CLASS_A_MONTH",
  "BUDGET_WORKERS_REQ_DAY", "BUDGET_D1_ROWS_READ_DAY",
  "BUDGET_R2_CLASS_B_MONTH",
]);
for (const [name, value] of Object.entries(config.vars ?? {})) {
  if (!/^(GOVERNOR_|BUDGET_)/.test(name)) continue;
  check(knownLimitVars.has(name),
    `${name} is not a limit var the code reads; fix the name or teach this guard`);
  // Mirror each family's runtime parser exactly: the governor trims
  // before parsing (src/governor.rs), the breaker parses the raw
  // string (src/glue.rs env_budget) - so a whitespace-padded BUDGET_*
  // value would silently revert to the default at runtime and must be
  // refused here, while the same padding on a GOVERNOR_* var is fine.
  const parsed = name.startsWith("GOVERNOR_") && typeof value === "string"
    ? value.trim()
    : value;
  const digits = typeof parsed === "string" && /^[0-9]+$/.test(parsed);
  check(digits && BigInt(parsed) <= 2n ** 64n - 1n,
    `${name} must be a u64 integer as the runtime parses it, got ${JSON.stringify(value)}`);
}

// The wasm build catches a class that fails to compile; only wrangler's
// deploy-time export check catches one that compiles without being
// exported. The bundle scan moves that failure to CI.
if (fs.existsSync("build/index.js") && localClasses.length > 0) {
  console.log("==> checking the built bundle exports every bound Durable Object class");
  const bundle = fs.readFileSync("build/index.js", "utf8");
  const exported = new Set(
    [...bundle.matchAll(/export\s*\{([^}]*)\}/g)]
      .flatMap((m) => m[1].split(","))
      .map((entry) => entry.trim().split(/\s+as\s+/).pop()),
  );
  [...bundle.matchAll(/export\s+class\s+([A-Za-z0-9_$]+)/g)]
    .forEach((m) => exported.add(m[1]));
  for (const className of localClasses) {
    check(exported.has(className),
      `build/index.js does not export Durable Object class ${className}`);
  }
} else if (localClasses.length > 0) {
  console.log("==> build/index.js absent; skipping the bundle export check");
}

for (const message of failures) console.error(message);
process.exit(failures.length === 0 ? 0 : 1);
JS

echo "deploy config OK"
