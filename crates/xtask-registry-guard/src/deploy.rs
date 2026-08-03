//! The deploy-configuration guard (`registry/docs/runbook.md`, "The cost
//! governor", "Deploy notes").
//!
//! Proves `wrangler.jsonc` still declares the bindings, Durable Object
//! lifecycle, crons, and parsable hard limits the Worker code depends
//! on - the failures `wrangler deploy` would otherwise surface only
//! against production, or (for a typo'd `GOVERNOR_*` var) not surface at
//! all: a set-but-unparsable hard limit fails closed to a ZERO limit and
//! blocks its pool loudly.
//!
//! When the built bundle exists, the guard also proves it exports every
//! bound Durable Object class - a class that compiles but is not
//! exported fails only at deploy time, after CI is already green.
//! `--require-bundle` (CI, after the Worker build step) makes a missing
//! bundle a failure instead of a skip.
//!
//! Three refusals are stricter than the node guard this replaces, none
//! of them accepting a config node rejected: a lifecycle key
//! (`deleted_classes`, `new_sqlite_classes`, `new_classes`,
//! `renamed_classes`) that is present but not an array is refused by
//! name instead of being coerced by JS (`?? []` only substitutes for
//! null, so a string flowed into `.length`, and an object made `.find`
//! throw); a migration tag that is not a string is refused instead of
//! being compared by JSON rendering; and both refusals name what is
//! wrong rather than surfacing a coercion artifact.
//!
//! ponytail: the export check is a lexical scan of the bundle's
//! `export{...}` lists; worker-build changing its output shape would
//! break the scan loudly (the bound classes stop matching), never
//! silently pass a missing export.

use std::path::Path;

use serde_json::Value;

/// The bindings the Worker code looks up by name (`src/glue.rs`,
/// `src/web_glue.rs`, `src/backup_glue.rs`, `src/governor_client.rs`).
const DATABASE: &str = "cabin-registry";
const BLOBS_BUCKET: &str = "cabin-registry-blobs";
const BACKUP_BUCKET: &str = "cabin-registry-backup";

/// The scheduled handler routes on the exact breaker expression; any
/// other schedule runs the nightly dump (`src/glue.rs`). The daily dump
/// cadence is pinned literally - a monthly rehearsal schedule may be
/// ADDED, but replacing it would quietly stretch the documented <= 24 h
/// metadata RPO.
const BREAKER_CRON: &str = "*/15 * * * *";
const DUMP_CRON: &str = "0 3 * * *";

/// Applied Durable Object migrations are immutable on the platform:
/// editing an already-deployed tag passes any graph check but never
/// replays, so the deployed history's first entry is pinned verbatim.
const DEPLOYED_V1_MIGRATION: &str = r#"{"tag":"v1","new_sqlite_classes":["Governor"]}"#;

/// The exact sets the Rust code reads. A misspelled name parses fine and
/// is silently ignored, which is the worst failure mode of all (the
/// operator believes the override is live).
const LIMIT_VARS: [&str; 15] = [
    // src/governor.rs storage_env_var / op_env_var
    "GOVERNOR_STORAGE_PRIMARY_BYTES",
    "GOVERNOR_STORAGE_BACKUP_BYTES",
    "GOVERNOR_STORAGE_DUMP_BYTES",
    "GOVERNOR_R2_CLASS_A_PUBLISH_MONTH",
    "GOVERNOR_R2_CLASS_A_INFRA_MONTH",
    "GOVERNOR_R2_CLASS_B_ORDINARY_MONTH",
    "GOVERNOR_R2_CLASS_B_SOURCE_MONTH",
    "GOVERNOR_R2_CLASS_B_VERIFIER_MONTH",
    "GOVERNOR_R2_CLASS_B_PUBLISH_MONTH",
    "GOVERNOR_R2_CLASS_B_INFRA_MONTH",
    // src/breaker.rs budgets
    "BUDGET_R2_STORAGE_BYTES",
    "BUDGET_R2_CLASS_A_MONTH",
    "BUDGET_WORKERS_REQ_DAY",
    "BUDGET_D1_ROWS_READ_DAY",
    "BUDGET_R2_CLASS_B_MONTH",
];

/// Everything the guard printed, and whether it accepted.
pub struct Report {
    /// Progress lines, in order, for stdout.
    pub notes: Vec<String>,
    /// Failure detail, in order, for stderr.
    pub failures: Vec<String>,
    /// The one-line summary printed after `failures`; `None` when the
    /// guard accepted.
    pub summary: Option<&'static str>,
}

impl Report {
    #[must_use]
    pub fn accepted(&self) -> bool {
        self.summary.is_none()
    }
}

const CONFIG_MISMATCH: &str = "the deploy configuration no longer matches the code's assumptions";
const BUNDLE_MISSING: &str =
    "build/index.js is missing; run worker-build before this guard (--require-bundle)";

/// Validates `registry_dir/wrangler.jsonc` against what the code
/// deploys against.
///
/// A file the guard cannot read is reported like a file that does not
/// parse - the same stream shape, the same summary - rather than as an
/// error: CI reads the summary line, not the exit path.
pub fn check(registry_dir: &Path, require_bundle: bool) -> Report {
    let bundle_path = registry_dir.join("build/index.js");
    if require_bundle && !bundle_path.is_file() {
        return Report {
            notes: Vec::new(),
            failures: Vec::new(),
            summary: Some(BUNDLE_MISSING),
        };
    }

    let mut notes =
        vec!["==> validating wrangler.jsonc against the code's deploy assumptions".into()];
    let config_path = registry_dir.join("wrangler.jsonc");
    let parsed = std::fs::read_to_string(&config_path)
        .map_err(|err| err.to_string())
        .and_then(|text| serde_json::from_str(&strip_jsonc(&text)).map_err(|err| err.to_string()));
    let config: Value = match parsed {
        Ok(config) => config,
        Err(err) => {
            return Report {
                notes,
                failures: vec![format!("wrangler.jsonc does not parse as JSONC: {err}")],
                summary: Some(CONFIG_MISMATCH),
            };
        }
    };

    let mut failures = Vec::new();
    validate_bindings(&config, &mut failures);
    validate_crons(&config, &mut failures);
    let local_classes = validate_durable_objects(&config, &mut failures);
    validate_limit_vars(&config, &mut failures);

    // The wasm build catches a class that fails to compile; only
    // wrangler's deploy-time export check catches one that compiles
    // without being exported. The bundle scan moves that failure to CI.
    if !local_classes.is_empty() {
        if bundle_path.is_file() {
            notes.push(
                "==> checking the built bundle exports every bound Durable Object class".into(),
            );
            match std::fs::read_to_string(&bundle_path) {
                Ok(bundle) => {
                    let exported = exported_names(&bundle);
                    for class in &local_classes {
                        if !exported.iter().any(|name| name == class) {
                            failures.push(format!(
                                "build/index.js does not export Durable Object class {class}"
                            ));
                        }
                    }
                }
                Err(err) => failures.push(format!("build/index.js could not be read: {err}")),
            }
        } else {
            notes.push("==> build/index.js absent; skipping the bundle export check".into());
        }
    }

    let summary = (!failures.is_empty()).then_some(CONFIG_MISMATCH);
    Report {
        notes,
        failures,
        summary,
    }
}

fn require(ok: bool, message: &str, failures: &mut Vec<String>) {
    if !ok {
        failures.push(message.to_owned());
    }
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], |a| a)
}

/// Whether `value` is JS-truthy, which is what the node guard this
/// replaces tested `script_name` with: `null`, `""`, `0` and `false` all
/// read as "not set".
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The keys whose value must be an array when present at all.
///
/// The node guard reached them through `?? []`, which only substitutes
/// for `null` and `undefined`: a string, a number or an object flowed
/// straight into `.length` / `.includes` / `.find` and either coerced
/// silently or threw. Refusing outright is stricter in every direction
/// and cannot fail open.
const ARRAY_VALUED: [&str; 4] = [
    "deleted_classes",
    "new_sqlite_classes",
    "new_classes",
    "renamed_classes",
];

/// Refuses a lifecycle key that is present but not an array.
fn require_array_valued(migrations: &[Value], failures: &mut Vec<String>) {
    for migration in migrations {
        for key in ARRAY_VALUED {
            let Some(value) = migration.get(key) else {
                continue;
            };
            require(
                value.is_null() || value.is_array(),
                &format!("migration {key} must be an array when present"),
                failures,
            );
        }
    }
}

/// The D1, R2, and Durable Object bindings, plus the dump's mirror of
/// the bound database id. Returns the bound `DB` entry.
fn validate_bindings<'a>(config: &'a Value, failures: &mut Vec<String>) -> Option<&'a Value> {
    let bound = array(config, "d1_databases")
        .iter()
        .find(|db| db.get("binding") == Some(&Value::from("DB")));
    require(
        bound.and_then(|db| db.get("database_name")) == Some(&Value::from(DATABASE)),
        &format!("d1_databases must bind DB to {DATABASE}"),
        failures,
    );
    // The wipe and migrate tooling hash and certify migrations/*.sql; a
    // drifted migrations_dir would have wrangler applying other files
    // than the ones the stamp certifies.
    require(
        bound.and_then(|db| db.get("migrations_dir")) == Some(&Value::from("migrations")),
        "the DB binding's migrations_dir must stay migrations",
        failures,
    );

    for (binding, bucket) in [("BLOBS", BLOBS_BUCKET), ("BACKUP", BACKUP_BUCKET)] {
        require(
            array(config, "r2_buckets").iter().any(|entry| {
                entry.get("binding") == Some(&Value::from(binding))
                    && entry.get("bucket_name") == Some(&Value::from(bucket))
            }),
            &format!("r2_buckets must bind {binding} to {bucket}"),
            failures,
        );
    }
    require(
        durable_object_bindings(config).iter().any(|entry| {
            entry.get("name") == Some(&Value::from("GOVERNOR"))
                && entry.get("class_name") == Some(&Value::from("Governor"))
        }),
        "durable_objects must bind GOVERNOR to class Governor",
        failures,
    );

    // The nightly dump exports whatever database D1_DATABASE_ID names; a
    // value diverging from the DB-bound database backs up the wrong one.
    require(
        bound.is_some()
            && config
                .get("vars")
                .and_then(|vars| vars.get("D1_DATABASE_ID"))
                == bound.and_then(|db| db.get("database_id")),
        "vars.D1_DATABASE_ID must mirror the DB binding's database_id",
        failures,
    );
    bound
}

fn validate_crons(config: &Value, failures: &mut Vec<String>) {
    let crons = config
        .get("triggers")
        .map_or(&[][..], |triggers| array(triggers, "crons"));
    for (cron, what) in [(BREAKER_CRON, "breaker's"), (DUMP_CRON, "nightly dump's")] {
        require(
            crons.iter().any(|entry| entry == &Value::from(cron)),
            &format!("triggers.crons must contain the {what} exact {cron}"),
            failures,
        );
    }
}

fn durable_object_bindings(config: &Value) -> &[Value] {
    config
        .get("durable_objects")
        .map_or(&[][..], |objects| array(objects, "bindings"))
}

/// Every locally-bound class must be introduced by the migrations chain
/// and never deleted - a deleted class destroys its `SQLite` storage, and
/// the governor's monthly operation windows cannot be rebuilt.
fn validate_durable_objects(config: &Value, failures: &mut Vec<String>) -> Vec<String> {
    // Wrangler's newer `exports` lifecycle must not coexist with the
    // `migrations` array (the platform accepts only one flow).
    require(
        config.get("exports").is_none(),
        "config mixes the exports DO lifecycle with the migrations array",
        failures,
    );
    let migrations = array(config, "migrations");
    require_array_valued(migrations, failures);
    // A missing or null tag is the empty tag (the node guard spelled
    // that `m.tag ?? ""`), and a tag that is not a string at all is
    // malformed - refusing it keeps the uniqueness comparison below on
    // plain strings, where it means what it says.
    let tags: Vec<&str> = migrations
        .iter()
        .map(|migration| {
            migration
                .get("tag")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    require(
        tags.iter().all(|tag| !tag.is_empty()),
        "every migration needs a non-empty tag",
        failures,
    );
    let mut unique = tags.clone();
    unique.sort_unstable();
    unique.dedup();
    require(
        unique.len() == tags.len(),
        "migration tags must be unique",
        failures,
    );
    require(
        migrations.first().map(ToString::to_string).as_deref() == Some(DEPLOYED_V1_MIGRATION),
        "migrations[0] must stay the deployed v1 Governor migration verbatim",
        failures,
    );
    // deleted_classes is banned outright, not just for the bound name: a
    // rename away and a later delete of the renamed class would destroy
    // the same storage while the bound name looks freshly introduced.
    require(
        migrations
            .iter()
            .all(|migration| array(migration, "deleted_classes").is_empty()),
        "deleted_classes is forbidden: a class delete destroys Durable Object storage",
        failures,
    );

    let local_classes: Vec<Value> = durable_object_bindings(config)
        .iter()
        // a foreign class is not ours to migrate
        .filter(|binding| !binding.get("script_name").is_some_and(truthy))
        .map(|binding| binding.get("class_name").cloned().unwrap_or(Value::Null))
        .collect();
    for class in &local_classes {
        let mut name = class.clone();
        let mut introduced = false;
        // Walk the chain backwards: the bound name may be the `to` of renames.
        for migration in migrations.iter().rev() {
            if let Some(renamed) = array(migration, "renamed_classes")
                .iter()
                .find(|renamed| renamed.get("to") == Some(&name))
            {
                name = renamed.get("from").cloned().unwrap_or(Value::Null);
            }
            introduced |= ["new_sqlite_classes", "new_classes"]
                .into_iter()
                .any(|key| array(migration, key).contains(&name));
        }
        require(
            introduced,
            &format!(
                "bound class {} is never introduced by a migration",
                render(class)
            ),
            failures,
        );
    }
    local_classes.iter().map(render).collect()
}

/// A hard-limit var that does not parse fails closed to a zero limit in
/// production (`src/governor.rs`) - correct there, but the typo belongs
/// to CI, not to a blocked pool. `BUDGET_*` vars fall back to defaults
/// instead, silently ignoring the intended override - the same class of
/// typo, caught the same way.
fn validate_limit_vars(config: &Value, failures: &mut Vec<String>) {
    let Some(vars) = config.get("vars").and_then(Value::as_object) else {
        return;
    };
    for (name, value) in vars {
        if !(name.starts_with("GOVERNOR_") || name.starts_with("BUDGET_")) {
            continue;
        }
        require(
            LIMIT_VARS.contains(&name.as_str()),
            &format!("{name} is not a limit var the code reads; fix the name or teach this guard"),
            failures,
        );
        // Mirror each family's runtime parser exactly: the governor
        // trims before parsing (`src/governor.rs`), the breaker parses
        // the raw string (`src/glue.rs` env_budget) - so a
        // whitespace-padded BUDGET_* value would silently revert to the
        // default at runtime and must be refused here, while the same
        // padding on a GOVERNOR_* var is fine. A value past u64::MAX
        // fails Rust's parse and lands on the same fail-closed zero, so
        // the range is checked too.
        let parsed = value.as_str().map(|text| {
            if name.starts_with("GOVERNOR_") {
                text.trim()
            } else {
                text
            }
        });
        let acceptable = parsed.is_some_and(|text| {
            !text.is_empty()
                && text.bytes().all(|byte| byte.is_ascii_digit())
                && text.parse::<u64>().is_ok()
        });
        require(
            acceptable,
            &format!(
                "{name} must be a u64 integer as the runtime parses it, got {}",
                serde_json::to_string(value).unwrap_or_else(|_| "?".into())
            ),
            failures,
        );
    }
}

/// A JSON value as the guard names it: strings bare, everything else in
/// its JSON rendering.
fn render(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

/// Comment- and string-aware JSONC strip: a `//` or `/*` inside a string
/// starts no comment (the config carries URLs and cron expressions).
fn strip_jsonc(text: &str) -> String {
    let source: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < source.len() {
        let two: String = source[i..source.len().min(i + 2)].iter().collect();
        if source[i] == '"' {
            out.push(source[i]);
            i += 1;
            while i < source.len() && source[i] != '"' {
                if source[i] == '\\' {
                    out.push(source[i]);
                    i += 1;
                }
                if i < source.len() {
                    out.push(source[i]);
                    i += 1;
                }
            }
            if i < source.len() {
                out.push(source[i]);
                i += 1;
            }
        } else if two == "//" {
            while i < source.len() && source[i] != '\n' {
                i += 1;
            }
        } else if two == "/*" {
            i += 2;
            while i < source.len() && source[i..source.len().min(i + 2)] != ['*', '/'] {
                i += 1;
            }
            i += 2;
        } else {
            out.push(source[i]);
            i += 1;
        }
    }
    out
}

/// Every name the bundle exports, from its `export{...}` lists and its
/// `export class` declarations.  A list entry may be renamed
/// (`Tb as Governor`), in which case the exported name is the last word.
fn exported_names(bundle: &str) -> Vec<String> {
    let mut names = Vec::new();
    for at in occurrences(bundle, "export") {
        let list = skip_space(bundle, at + "export".len());
        if bundle[list..].starts_with('{')
            && let Some(end) = bundle[list..].find('}')
        {
            for entry in bundle[list + 1..list + end].split(',') {
                names.push(renamed_export(entry.trim()));
            }
        }
    }
    for at in occurrences(bundle, "export") {
        let class = skip_space(bundle, at + "export".len());
        if class == at + "export".len() || !bundle[class..].starts_with("class") {
            continue;
        }
        let start = skip_space(bundle, class + "class".len());
        if start == class + "class".len() {
            continue;
        }
        let end = bundle[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .map_or(bundle.len(), |offset| start + offset);
        if end > start {
            names.push(bundle[start..end].to_owned());
        }
    }
    names
}

/// The name an export-list entry actually exports: the last piece of a
/// split on ` as ` (one or more spaces, `as`, one or more spaces).
/// Deliberately NOT the last whitespace-delimited token - an entry
/// carrying a trailing comment (`Foo // Governor`) would then read as an
/// export of the commented name.
fn renamed_export(entry: &str) -> String {
    let mut piece = 0;
    let mut cursor = 0;
    while cursor < entry.len() {
        let leading = whitespace_run(&entry[cursor..]);
        let name = cursor + leading;
        let trailing = if leading > 0 && entry[name..].starts_with("as") {
            whitespace_run(&entry[name + 2..])
        } else {
            0
        };
        if trailing > 0 {
            piece = name + 2 + trailing;
            cursor = piece;
            continue;
        }
        cursor += entry[cursor..].chars().next().map_or(1, char::len_utf8);
    }
    entry[piece..].to_owned()
}

/// The byte length of the leading whitespace run in `text`.
fn whitespace_run(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// Every byte offset at which `needle` occurs in `haystack`.
fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = haystack[at..].find(needle) {
        found.push(at + offset);
        at += offset + 1;
    }
    found
}

fn skip_space(text: &str, at: usize) -> usize {
    at + text[at..].len() - text[at..].trim_start().len()
}
