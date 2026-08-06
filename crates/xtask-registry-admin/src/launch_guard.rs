//! The launch guard (`registry/docs/runbook.md`, "Data policy"):
//! destructive maintenance runs this before touching anything.  It
//! reads `meta.launched` and succeeds only when the value is exactly
//! `false`.  `true` means the registry is launched and its data is
//! permanent, so the guard refuses - and so does every other state
//! (missing row, unreadable database, unexpected value), fail-safe.
//! Flipping the flag to `true` is a one-time launch-checklist item.
//!
//! Remote reads go through the `DB` binding, because wrangler resolves
//! even a database *name* through the config, so the binding is the
//! only real path.  Destructive commands like
//! `d1 delete cabin-registry` resolve that name against the ACCOUNT
//! instead, so the guard first proves the two resolutions agree: the
//! account's database named `cabin-registry` must carry exactly the id
//! the config binds, or the guard could read one database while a wipe
//! deletes another.  Local mode has no name resolution - the `DB`
//! binding is the local state - so it skips that check, as the shell
//! did.
//!
//! Nothing reaches stdout: a passing guard is silent, and every
//! refusal is one `launch guard:` line on stderr.
//!
//! One thing the port is strictly better at, rather than equal: the
//! shell's fail-safe rested on a `refuse` shell function calling
//! `exit`, both of which an exported function in the environment could
//! shadow, turning every refusal into a silent success.  A compiled
//! binary has no such dispatch.
//!
//! One thing it was worse at while its last caller was shell, now
//! closed.  `registry/scripts/wipe.sh` reached this through
//! `cargo run`, which the environment can redirect -
//! `CARGO_TARGET_<TRIPLE>_RUNNER`, or a `cargo` earlier on `PATH` - so
//! the guard could be made not to run at all.  The shell guard before
//! it had the same shape of hole through `npx`, which a fake on `PATH`
//! answered for.  That caller is [`crate::wipe`] now and the hop is
//! gone: nothing stands between its refusal and the `rm -rf` but a
//! function call.  What the environment still reaches - `wrangler`
//! through `npx` - it reached in the shell too, and the guard defends
//! against neither: it exists to stop a launched registry and a stale
//! config, not its own operator.
//!
//! Ceilings, where this deliberately stops short of the shell it
//! replaces.  All are fail-closed - the guard refuses where the shell
//! carried on, never the reverse, which is the only direction a guard
//! may differ in:
//!
//! - the mode must be the only argument.  The shell read `$1` and
//!   ignored the rest, so `--local --remote` ran in local mode;
//! - `meta.launched` must be exactly `false`.  Command substitution
//!   stripped trailing newlines and dropped NUL bytes before the
//!   shell's comparison, so `false\n` and `false\0` passed there;
//! - an answer that is not the documented shape ends the run.  The
//!   shell reached the same refusal through a `node` `TypeError`, but
//!   `results` that merely *has* a `length` - a string, an object -
//!   satisfied its duck-typed check and reported the row as missing;
//! - a `meta.launched` holding a JSON object is refused naming its
//!   JSON text, where the shell's `String()` wrote `[object Object]`.
//!   The verdict is the same refusal either way, and D1 returns no
//!   column that shape (see [`crate::display`]);
//! - the refusal line is prefixed `error:` by the command shim, where
//!   the shell wrote `launch guard:` alone.  What follows is the
//!   shell's own wording - `cargo registry-smoke` greps a refused
//!   wipe's output for one of these messages - except where the
//!   offending value is rendered into it, which follows
//!   [`crate::display`]'s ceilings rather than `String()`'s exact
//!   text.

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{declared_database_id, display, output, registry_root, results, wrangler};

/// The database whose two resolutions - the config's binding and the
/// account's name lookup - the remote check proves identical.
const DATABASE: &str = "cabin-registry";

const LAUNCHED: &str = "SELECT value FROM meta WHERE key = 'launched'";

/// The mode the guard was asked for.  Local state has no account-level
/// name resolution, which is the whole of the difference.
#[derive(Clone, Copy)]
pub enum Mode {
    Remote,
    Local,
}

impl Mode {
    /// The wrangler flag, which is also exactly what the shell took as
    /// its one argument.
    #[must_use]
    pub fn flag(self) -> &'static str {
        match self {
            Self::Remote => "--remote",
            Self::Local => "--local",
        }
    }
}

/// One entry of `wrangler d1 list --json`.
#[derive(Deserialize)]
struct Database {
    name: Option<String>,
    uuid: Option<String>,
    #[serde(rename = "database_id")]
    id: Option<String>,
}

impl Database {
    /// `db.uuid || db.database_id`, which is a *falsy* fallback: an
    /// empty `uuid` falls through to `database_id` exactly as a
    /// missing one does.
    fn bound(&self) -> Option<&str> {
        fn held(value: Option<&String>) -> Option<&str> {
            value.map(String::as_str).filter(|value| !value.is_empty())
        }
        held(self.uuid.as_ref()).or_else(|| held(self.id.as_ref()))
    }
}

/// Runs the guard.  Returns `Ok(())` only when the registry is
/// definitively not launched.
///
/// # Errors
///
/// On every other state, each carrying the shell's own message.
pub fn run(mode: Mode) -> Result<()> {
    if matches!(mode, Mode::Remote) {
        same_database()?;
    }

    let answer = output(
        wrangler(&[
            "d1",
            "execute",
            "DB",
            mode.flag(),
            "--json",
            "--command",
            LAUNCHED,
        ])
        .current_dir(registry_root()),
    )
    .map_err(|_| refusal("could not read meta.launched"))?;
    let rows = results(&answer).map_err(|_| refusal("unexpected wrangler output"))?;

    match launched(&rows).as_str() {
        "false" => Ok(()),
        "true" => Err(anyhow::anyhow!(
            "launch guard: the registry is launched (meta.launched = 'true'); its data is \
             permanent and destructive maintenance is forbidden (docs/runbook.md, \
             \"Data policy\")"
        )),
        "__MISSING__" => Err(refusal(
            "meta.launched is missing (baseline migration not applied?)",
        )),
        other => Err(refusal(&format!(
            "meta.launched is '{other}' (expected 'false')"
        ))),
    }
}

/// The flag's value as the shell's `node` rendered it, or the sentinel
/// it used for no rows.
fn launched(rows: &[serde_json::Map<String, serde_json::Value>]) -> String {
    let Some(row) = rows.first() else {
        return "__MISSING__".to_owned();
    };
    // `String(row.value)`, which is `display`'s coercion and not
    // `console.log`'s: a missing key renders as the word `undefined`,
    // and a one-element array renders as its element, so a `["false"]`
    // passes here as it passed there.
    //
    // The coercion widens the encoding, never the meaning. Every value
    // it renders as `false` - the string, the JSON boolean, arrays
    // nesting either - is a spelling of "not launched"; `true`,
    // `["true"]`, `null`, `0`, `""` and `{}` all render as something
    // else and refuse. Narrowing this to a JSON string would refuse
    // states the shell allowed without refusing any state it should
    // have caught.
    match row.get("value") {
        Some(value) => display(value),
        None => "undefined".to_owned(),
    }
}

/// Proves the account's `cabin-registry` is the database the config
/// binds, so a read here and a `d1 delete` by name cannot diverge.
fn same_database() -> Result<()> {
    let answer = output(wrangler(&["d1", "list", "--json"]).current_dir(registry_root()))
        .map_err(|_| missing_database())?;
    let databases: Vec<Database> = serde_json::from_str(&answer).map_err(|_| missing_database())?;
    // `list.find(...)`: the FIRST entry of that name wins. Two
    // databases sharing it would leave the check proving the config
    // matches whichever wrangler listed first, which is the shell's
    // behavior and is carried over rather than tightened here.
    let account = databases
        .iter()
        .find(|database| database.name.as_deref() == Some(DATABASE))
        .and_then(Database::bound)
        .ok_or_else(missing_database)?;

    let path = registry_root().join("wrangler.jsonc");
    let text = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| declared_database_id(&text))
        .ok_or_else(|| refusal("no database_id in wrangler.jsonc"))?;

    if account != text {
        bail!(
            "launch guard: the account's {DATABASE} is {account} but wrangler.jsonc binds \
             {text}; refusing (fail-safe)"
        );
    }
    Ok(())
}

fn missing_database() -> anyhow::Error {
    refusal(&format!("no database named {DATABASE} on the account"))
}

/// The shell's `refuse`: one `launch guard:` line, and the trailing
/// `(fail-safe)` every branch but the launched one carried.
fn refusal(message: &str) -> anyhow::Error {
    anyhow::anyhow!("launch guard: {message}; refusing (fail-safe)")
}
