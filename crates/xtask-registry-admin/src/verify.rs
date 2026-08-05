//! The scheduled verification pass
//! (`.github/workflows/registry-verify.yml`, `registry/docs/runbook.md`
//! "Verification pipeline"): read the pending versions off the admin
//! plane, hand each one's archive to `cabin-registry-verify`, and PATCH
//! back the verdict it renders.  A rejection is the verifier working,
//! not a failure; only operational trouble - a download that would not
//! come, a verifier that would not run, a PATCH the service refused -
//! counts, and every one of those leaves its version pending for the
//! next pass.
//!
//! Unlike the rest of the crate this runs unattended, and its log is a
//! public CI artifact.  It prints package names and versions, which
//! the admin API already discloses to any verify-scope holder, plus
//! the verdicts and reason codes its own verifier run computes for
//! them - and never the token, the corpus, or archive bytes.
//!
//! Which of the two failure classes a step takes is a property of the
//! shell this replaces, not a judgment call: a download or a child
//! process guarded by `if !` counted a failure and moved on, while
//! every bare `$(jq …)` field read aborted the WHOLE run under
//! `set -e`, leaving that version and every unprocessed one pending.
//! Malformed listing data therefore stops the run rather than
//! rejecting or skipping a package, and the split is preserved per
//! call site.  In the same spirit `jq -r` renders a missing or null
//! field as the literal string `null`, so an entry without a `name`
//! goes on to verify a package called `null`; that is what the shell
//! did.
//!
//! The verifier child inherits the whole environment, the privileged
//! `REGISTRY_VERIFY_TOKEN` included, exactly as the shell's child did.
//! It reads only the `VERIFY_*` caps, and removing the rest would be a
//! change rather than a port.  The publisher-controlled upstream
//! download is the one request that never sees the token, and it runs
//! on its own agent so it cannot.
//!
//! Ceilings, each either invisible on the ephemeral runner this is
//! scheduled on or fail-safe:
//!
//! - every abort exits 1, where the shell propagated `curl`'s 22/6/7
//!   and `jq`'s 5.  Nothing reads the distinction: GitHub Actions
//!   splits zero from non-zero, and no other consumer exists;
//! - the shell's stderr carried `curl`'s, `jq`'s and bash's own error
//!   lines beside the diagnostics the script itself wrote.  Only the
//!   script's lines are reproduced, byte for byte; an abort those
//!   tools diagnosed carries a one-line diagnostic of this crate's in
//!   their place, and noise the run *survived* - the `[ -eq ]` and
//!   arithmetic errors of a non-integer count - is dropped outright;
//! - an unset `REGISTRY_VERIFY_TOKEN`, `REGISTRY_ORIGIN` or
//!   `EXPECTED_API_ORIGIN` reads as empty, where `set -u` aborted with
//!   bash's unbound-variable message.  Same exit code, and the
//!   workflow sets all three;
//! - the upstream budget models bash's `$SECONDS`: a whole-second
//!   clock anchored at process start, charged as the difference of two
//!   reads of it rather than as real elapsed time, so a transfer that
//!   straddles a second boundary is charged for it;
//! - the CA roots are `ureq`'s compiled-in `webpki-roots`, where `curl`
//!   read `CURL_CA_BUNDLE`, `SSL_CERT_FILE` and the system store - the
//!   same environment ceiling the crate's proxy handling carries
//!   (`crate::audit`).  A *test* build additionally turns on `ureq`'s
//!   `native-certs`, which is what lets the differential point both
//!   sides at one local mock; nothing but the compiled-in roots reaches
//!   the shipped binary;
//! - a redirect on the upstream download must be an absolute `https://`
//!   URL.  `curl` resolved a relative `Location` against the URL it
//!   came from; here that fails the download, and a failed upstream
//!   download is "verifying without it", never a rejection;
//! - `entry.json` and the PATCH body are `serde_json`'s compact
//!   rendering, which differs from `jq`'s for exponent-form floats,
//!   integers outside `i64`/`u64`, and U+007F (`jq` escapes it).  Both
//!   are re-parsed by their only consumer, and the parsed documents
//!   are identical.  The same normalization reaches every number this
//!   module renders: `jq` 1.7+ preserves some numeric lexemes and
//!   canonicalizes others, varying by version and by output path, so
//!   exact parity is not stable even against the runner's own `jq` -
//!   and every field this loop reads (`name`, `version`, `revision`,
//!   `checksum`, `published_at`) the production listing types as a
//!   string (`registry/src/glue.rs`, `AdminVersionRecord`);
//!   `published_by` is its one integer, and nothing here reads it.
//!   `jq`'s own input extensions - `NaN` and kin read as `null`,
//!   lenient numeric forms like a leading zero, `+1` or `1.`, lone
//!   surrogates replaced - abort here as unparsable;
//! - every parse here carries `serde_json`'s 128-level recursion cap,
//!   where `jq` read deeper.  The registry parses each metadata
//!   document with the same cap at publish and again when the listing
//!   embeds it (`registry/src/publish.rs`, `registry/src/glue.rs`), so
//!   only a document within the listing wrapper's few levels of the
//!   cap can thread the needle - and it aborts the run fail-safe
//!   (everything stays pending, the cron goes red) where the shell
//!   walked on;
//! - the token plane's connect ceiling covers the TCP connect alone
//!   where `curl`'s 300-second default spanned DNS, TCP and TLS, and
//!   the upstream deadline cannot reach into DNS resolution either -
//!   `ureq` draws those lines, a stalled resolver still lands on the
//!   budget as elapsed charge, and the workflow's 15-minute job
//!   timeout backstops both exactly as it did the shell;
//! - a run that returns through `main` - a clean finish or an abort a
//!   tool diagnosed - drops the work directory and the corpus file,
//!   where the shell installed no trap and left both behind.  The five
//!   diagnostics the shell wrote itself still exit through
//!   [`std::process::exit`], leaking them exactly as it did.

use std::fs::File;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rand::seq::SliceRandom as _;
use serde_json::{Map, Value};

/// L52: a relative path resolved against the working directory, which
/// the Cargo alias makes the repository root.  Deliberately not
/// anchored to this crate's manifest directory - the shell anchored it
/// to neither, and the binary is a build artifact of the workspace,
/// not of the tool.
const VERIFIER: &str = "target/release/cabin-registry-verify";

/// L65: the aggregate seconds one run may spend on upstream archives.
/// The admin listing is deterministically sorted, so without a
/// per-run bound a publisher with several slow upstream hosts could
/// pin the same early-sorting versions to the front of every cron pass
/// and starve everything behind them; the shuffle rotates who gets the
/// budget when it runs out.
const BUDGET: i64 = 300;

/// L175: `--max-time` for one transfer, never more than this however
/// much budget is left.
const TRANSFER_CAP: i64 = 120;

/// L178: `--max-filesize`, part of the documented provenance contract
/// (`docs/manifest.md`) and the bound on runner disk.
const MAX_UPSTREAM_BYTES: u64 = 268_435_456;

/// L178: `--connect-timeout 10`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `curl`'s default `--max-redirs`, which L177's `-L` took.
const MAX_REDIRECTS: usize = 50;

/// The verification pass, end to end.
///
/// # Errors
///
/// If a request, a temporary file, a child process or a field read
/// fails - the paths the shell aborted on under `set -e` carrying the
/// failing tool's own diagnostic.  The five diagnostics the shell
/// wrote itself do not come back this way: they print bare and exit,
/// so nothing prefixes them.
pub fn run() -> Result<()> {
    // bash anchors `$SECONDS` at shell start, before the first
    // request, and the budget below is a difference of two reads of it.
    let started = Instant::now();

    // L2-17: the privileged token travels to exactly two origins, and
    // neither `curl` nor `ureq` refuses cleartext on its own.
    let token = env("REGISTRY_VERIFY_TOKEN");
    if token.is_empty() {
        abort("REGISTRY_VERIFY_TOKEN is not configured");
    }
    let registry_origin = env("REGISTRY_ORIGIN");
    if !registry_origin.starts_with("https://") {
        abort(&format!(
            "REGISTRY_ORIGIN must be https, got: {registry_origin}"
        ));
    }
    let expected_api_origin = env("EXPECTED_API_ORIGIN");
    if !expected_api_origin.starts_with("https://") {
        abort(&format!(
            "EXPECTED_API_ORIGIN must be https, got: {expected_api_origin}"
        ));
    }

    let plane = Plane {
        agent: plane_agent(),
        token,
    };

    // L19-29: one hostname, one role.  The index origin serves
    // config.json and the artifacts; the admin API lives on the origin
    // config.json declares, and a mismatch fails the run so versions
    // stay pending.
    let config = plane
        .get(&format!("{registry_origin}/config.json"))
        .context("the config.json request failed")?;
    let api_origin = declared_api_origin(&config)?;
    if api_origin != expected_api_origin {
        abort("config.json api does not match the pinned verifier API origin");
    }

    // L31-38: the count as `jq length` rendered it and `[ -eq 0 ]`
    // compared it.  Only a count spelling the integer zero is "nothing
    // pending"; one that is no integer at all - a float length, a
    // multi-document listing, an empty body - fails the comparison into
    // the false branch, so the headline still prints, the corpus is
    // still fetched, and the loop expansion's arithmetic error then
    // empties the walk ([`loop_indices`]).  Only `jq` itself failing -
    // a listing that will not parse, a `versions` with no length -
    // aborts.
    let listing = plane
        .get(&format!(
            "{api_origin}/api/v1/admin/versions?status=pending"
        ))
        .context("the pending listing request failed")?;
    let documents = answer(&listing, "the pending listing")?.unwrap_or_default();
    let count = read_stream(&documents, |doc| length(&index(doc, "versions")?))?;
    if count.parse::<i64>().ok() == Some(0) {
        println!("nothing pending");
        return Ok(());
    }
    println!("{count} pending version(s)");

    // L40-47: the corpus for the name advisories, fetched once.  A
    // failed fetch fails the run - without it no advisory can run, and
    // proceeding could verify a confusable name.
    let corpus = tempfile::NamedTempFile::new().context("open a temporary file for the corpus")?;
    plane
        .download(
            &format!("{api_origin}/api/v1/admin/packages"),
            corpus.path(),
        )
        .context("the package corpus request failed")?;

    let mut pass = Pass {
        plane,
        registry_origin,
        api_origin,
        corpus: corpus.path().to_owned(),
        started,
        budget: BUDGET,
    };

    // L66: the listing is sorted, so the order it is walked in must
    // not be.
    let mut order = loop_indices(&count);
    order.shuffle(&mut rand::rng());
    // The walk is only reachable off a single-document listing: a
    // multi-document count carries a newline and never survives the
    // arithmetic in [`loop_indices`].
    let versions = documents
        .first()
        .map(|doc| index(doc, "versions"))
        .transpose()?
        .unwrap_or(Value::Null);

    let mut failures = 0_u64;
    for position in order {
        if pass.version(&at(&versions, position)?)? {
            failures += 1;
        }
    }
    // L233-236: one version failing operationally must not starve the
    // rest, so failures are aggregated and fail the run at the end.
    if failures > 0 {
        abort(&format!(
            "{failures} version(s) hit operational failures and stay pending"
        ));
    }
    Ok(())
}

/// The token plane: every request that carries the privileged
/// credential, on an agent that follows no redirect - `curl` without
/// `-L`, so a 3xx is the answer rather than a step toward one, and the
/// credential cannot be forwarded anywhere the pinned origins did not
/// name.
struct Plane {
    agent: ureq::Agent,
    token: String,
}

/// The token plane's agent.  No redirect is followed, and the connect
/// ceiling is `curl`'s own 300-second default - `ureq` would otherwise
/// impose a 30-second one the shell never had.  No overall timeout, for
/// the same reason: none of the token-plane `curl` calls set one.
fn plane_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_mins(5))
        .build()
}

impl Plane {
    fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.agent
            .request(method, url)
            .set("authorization", &format!("Bearer {}", self.token))
    }

    /// `config=$(curl -fsS …)`: the body as command substitution
    /// captured it.
    fn get(&self, url: &str) -> Result<String> {
        let mut body = Vec::new();
        self.request("GET", url)
            .call()?
            .into_reader()
            .read_to_end(&mut body)?;
        Ok(captured_bytes(&body))
    }

    /// `curl -fsS -o "$path" …`: the body straight to a file, never
    /// parsed here.  The corpus and the archive are both
    /// publisher-influenced bytes whose only reader is the verifier
    /// child.
    fn download(&self, url: &str, path: &Path) -> Result<()> {
        let response = self.request("GET", url).call()?;
        let mut file = File::create(path)?;
        std::io::copy(&mut response.into_reader(), &mut file)?;
        Ok(())
    }

    /// L219-223: the verdict.  A 409 means the row changed since the
    /// listing (a republish, a competing verdict) and the next run sees
    /// the replacement.  `-o /dev/null` still drained the transfer, so
    /// a response body that cuts out mid-way is a counted PATCH
    /// failure, never a success line.
    fn patch(&self, url: &str, body: &str) -> Result<()> {
        let response = self
            .request("PATCH", url)
            .set("content-type", "application/json")
            .send_string(body)?;
        std::io::copy(&mut response.into_reader(), &mut std::io::sink())?;
        Ok(())
    }
}

/// What one pass carries from version to version: the plane, the two
/// origins, the corpus every advisory reads, and the shared upstream
/// budget.
struct Pass {
    plane: Plane,
    registry_origin: String,
    api_origin: String,
    corpus: PathBuf,
    started: Instant,
    budget: i64,
}

impl Pass {
    /// L67-230, one pending version.  The `bool` is the shell's
    /// `failures=$((failures + 1))`: every exit from the loop body
    /// leaves the version pending, and only some of them count.
    fn version(&mut self, entry: &Value) -> Result<bool> {
        let name = field(entry, "name")?;
        let version = field(entry, "version")?;
        let revision = field(entry, "revision")?;
        let checksum = field(entry, "checksum")?;
        let published_at = field(entry, "published_at")?;

        let workdir = tempfile::TempDir::new().context("open a work directory")?;
        let entry_path = workdir.path().join("entry.json");
        std::fs::write(&entry_path, compact(entry)).context("write the listing entry")?;

        // L77-89: the name advisories run before the download - they
        // need no bytes, so an abstained version costs a listing entry
        // per pass and never a re-download.  Abstain renders no verdict
        // and is deliberately not a failure: the version stays pending
        // until the stuck-pending alert summons an operator
        // (`registry/docs/runbook.md`, "Verification pipeline").
        let Some(advice_json) = capture(
            Command::new(VERIFIER)
                .arg("--name-advisories")
                .arg(&entry_path)
                .arg(&self.corpus),
        ) else {
            eprintln!("{name}@{version}: name advisories failed operationally; leaving it pending");
            return Ok(true);
        };
        let Some(advice_docs) = answer(&advice_json, "the name advisories answer")? else {
            eprintln!("{name}@{version}: unknown advice ''; leaving it pending");
            return Ok(true);
        };
        let advice = read_stream(&advice_docs, |doc| Ok(raw(&index(doc, "advice")?)))?;
        match advice.as_str() {
            "proceed" => {}
            "abstain" => {
                let findings = read_stream(&advice_docs, |doc| join(&index(doc, "findings")?))?;
                println!(
                    "{name}@{version}: abstain ({findings}); leaving it pending for operator review"
                );
                return Ok(false);
            }
            _ => {
                eprintln!("{name}@{version}: unknown advice '{advice}'; leaving it pending");
                return Ok(true);
            }
        }

        let archive = workdir.path().join("archive.zip");
        if self
            .plane
            .download(
                &artifact_url(&self.registry_origin, &name, &version, &revision),
                &archive,
            )
            .is_err()
        {
            eprintln!("{name}@{version}: archive download failed; leaving it pending");
            return Ok(true);
        }

        let upstream = match self.upstream(entry, workdir.path(), &name, &version)? {
            Upstream::Refused => return Ok(true),
            Upstream::Absent => None,
            Upstream::Downloaded(path) => Some(path),
        };

        let mut inspect = Command::new(VERIFIER);
        inspect.arg(&archive).arg(&entry_path);
        if let Some(upstream) = &upstream {
            inspect.arg("--upstream").arg(upstream);
        }
        let Some(result) = capture(&mut inspect) else {
            eprintln!("{name}@{version}: verifier failed operationally; leaving it pending");
            return Ok(true);
        };

        // L196-214
        let Some(result_docs) = answer(&result, "the verifier answer")? else {
            eprintln!("{name}@{version}: unknown verdict ''; leaving it pending");
            return Ok(true);
        };
        let verdict = read_stream(&result_docs, |doc| Ok(raw(&index(doc, "verdict")?)))?;
        let mut reason = String::new();
        let body = match verdict.as_str() {
            "verified" => verdict_body("verified", None, &checksum, &published_at),
            "rejected" => {
                reason = read_stream(&result_docs, |doc| join(&index(doc, "reasons")?))?;
                verdict_body("rejected", Some(&reason), &checksum, &published_at)
            }
            _ => {
                eprintln!("{name}@{version}: unknown verdict '{verdict}'; leaving it pending");
                return Ok(true);
            }
        };

        if self
            .plane
            .patch(&verdict_url(&self.api_origin, &name, &version), &body)
            .is_err()
        {
            eprintln!("{name}@{version}: verdict PATCH failed; leaving it pending");
            return Ok(true);
        }
        let detail = if reason.is_empty() {
            String::new()
        } else {
            format!(" ({reason})")
        };
        println!("{name}@{version}: {verdict}{detail}");
        Ok(false)
    }

    /// L125-188: the pinned upstream archive, when the stored metadata
    /// declares one.
    ///
    /// The URL is publisher-controlled, so the privileged token is
    /// never sent with this request and every hop must be https;
    /// integrity comes from the verifier's SHA-256 pin.
    /// Destination-IP filtering (private and loopback ranges, per
    /// redirect hop and DNS answer) is deliberately not attempted: this
    /// job runs only on ephemeral GitHub-hosted runners with no
    /// privileged network position, and the response bytes are never
    /// disclosed - the digest pin reduces any reachable endpoint to an
    /// equality oracle.  Revisit before ever moving this job to a
    /// self-hosted runner.
    ///
    /// Publish validation already rejected non-https URLs, so a
    /// cleartext URL here is corrupt registry state - an operational
    /// failure, never a verdict.  A failed download must not reject the
    /// package (a flaky upstream host is not the publisher's fault):
    /// the verifier still runs without the file, its first two passes
    /// can reject a bad archive on their own, and a version whose
    /// provenance could not be checked exits operationally and stays
    /// pending.  Rejecting on a size-cap abort is avoided for the same
    /// reason - the judgment can reflect a server-misreported length,
    /// and a false rejection is terminal where pending is recoverable.
    fn upstream(
        &mut self,
        entry: &Value,
        workdir: &Path,
        name: &str,
        version: &str,
    ) -> Result<Upstream> {
        let url = upstream_url(entry)?;
        if url.is_empty() {
            return Ok(Upstream::Absent);
        }
        if !url.starts_with("https://") {
            eprintln!("{name}@{version}: stored upstream url is not https; leaving it pending");
            return Ok(Upstream::Refused);
        }
        if self.budget <= 0 {
            eprintln!("{name}@{version}: upstream download budget exhausted; verifying without it");
            return Ok(Upstream::Absent);
        }

        // One transfer may use at most the remaining budget (never more
        // than TRANSFER_CAP), and every attempt charges at least one
        // whole second, so the aggregate bound holds even against
        // sub-second transfers.
        let cap = transfer_cap(self.budget);
        let began = self.seconds();
        let path = workdir.join("upstream-archive");
        let downloaded = download_upstream(&url, &path, cap).is_ok();
        if !downloaded {
            eprintln!("{name}@{version}: upstream archive download failed; verifying without it");
        }
        // Charged after the diagnostic, where the shell read `$SECONDS`
        // again: the clock is what the charge is a difference of.
        self.budget -= spent(began, self.seconds());
        if downloaded {
            Ok(Upstream::Downloaded(path))
        } else {
            Ok(Upstream::Absent)
        }
    }

    /// bash's `$SECONDS`: whole seconds since the process started.
    fn seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// What the upstream step leaves the verifier invocation with, which is
/// not the same question as whether a download happened: a stored
/// cleartext URL refuses the version outright, while a download that
/// did not come just verifies without the file.
enum Upstream {
    Refused,
    Absent,
    Downloaded(PathBuf),
}

/// The shell's own `echo … >&2; exit 1`: printed without the `error:`
/// prefix `main` renders, exiting where `set -e` did.  Skipping
/// destructors is faithful - the shell installed no trap, so an aborted
/// run left its temporary files behind.
fn abort(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

/// An unset variable reads as empty, which is the case the guards
/// above already answer.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// bash's `$(…)`, which edits what it captures in exactly two ways: a
/// NUL cannot live in a variable and is dropped, and every trailing
/// newline is stripped.  A reason code ending in a newline therefore
/// reaches the PATCH body without it.
fn captured(text: &str) -> String {
    let mut text: String = text
        .chars()
        .filter(|character| *character != '\0')
        .collect();
    text.truncate(text.trim_end_matches('\n').len());
    text
}

/// [`captured`] over the raw bytes a body or a child's stdout arrived
/// as: the NULs vanish from the BYTE stream before the text is read
/// off it - `c2 00 a0` is one no-break space to bash, not two
/// replacement characters - and only then do invalid sequences take
/// U+FFFD, as a bash variable printed through a UTF-8 pipeline reads.
fn captured_bytes(bytes: &[u8]) -> String {
    let bytes: Vec<u8> = bytes.iter().copied().filter(|byte| *byte != 0).collect();
    captured(&String::from_utf8_lossy(&bytes))
}

/// `$(jq -r '.<key>' <<<"$value")` (L68-72, L90, L196).
fn field(value: &Value, key: &str) -> Result<String> {
    Ok(captured(&raw(&index(value, key)?)))
}

/// What a captured child's stdout holds, as `jq` read it: a *stream* of
/// JSON documents, the filter run once per document.  A capture that is
/// empty or all whitespace is no input at all - the filter runs zero
/// times, prints nothing, and every field read off it is the empty
/// string, which reaches the `unknown advice ''` / `unknown verdict ''`
/// arm and counts one failure - where a capture that will not parse is
/// a `jq` parse error that aborts the whole run, any documents before
/// the malformed one notwithstanding.  A child exiting 0 with nothing
/// on stdout is the difference between those two, so the empty case
/// cannot be folded into the parse.
fn answer(text: &str, what: &str) -> Result<Option<Vec<Value>>> {
    // jq's lexer skips one leading byte-order mark, so a BOM-prefixed
    // response parses in the shell and must parse here.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // "No input" is JSON's own whitespace alone: jq does not skip the
    // wider Unicode set `str::trim` would, so an NBSP-only capture is a
    // parse error that aborts the run, never a blank answer.
    if text
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return Ok(None);
    }
    let documents: Vec<Value> = serde_json::Deserializer::from_str(text)
        .into_iter()
        .collect::<Result<_, _>>()
        .with_context(|| format!("{what} is not JSON"))?;
    Ok(Some(documents))
}

/// One `jq -r` filter over a document stream: each document's rendering
/// on its own line, the whole captured as `$(…)` captured it.  A
/// two-document answer therefore reads as a two-line value that matches
/// no `case` arm, and a trailing document that renders empty vanishes
/// into whatever its predecessors spelled.
fn read_stream(documents: &[Value], read: impl Fn(&Value) -> Result<String>) -> Result<String> {
    let lines: Vec<String> = documents.iter().map(read).collect::<Result<_>>()?;
    Ok(captured(&lines.join("\n")))
}

/// jq's `.<key>`: an object answers with its member or `null`, `null`
/// answers `null`, and anything else is a program error that aborts the
/// whole run.
fn index(value: &Value, key: &str) -> Result<Value> {
    match value {
        Value::Object(map) => Ok(map.get(key).cloned().unwrap_or(Value::Null)),
        Value::Null => Ok(Value::Null),
        other => bail!("cannot index {} with \"{key}\"", kind(other)),
    }
}

/// jq's `.[<n>]`, on the same terms.
fn at(value: &Value, position: usize) -> Result<Value> {
    match value {
        Value::Array(items) => Ok(items.get(position).cloned().unwrap_or(Value::Null)),
        Value::Null => Ok(Value::Null),
        other => bail!("cannot index {} with a number", kind(other)),
    }
}

/// The type name jq puts in its error text.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `jq -r`: a string is its own text, and everything else takes jq's
/// default *indented* rendering, because none of these reads passed
/// `-c`.  A `null` - a missing field included - is therefore the
/// four-character string `null`.
fn raw(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        // Serializing a `Value` cannot fail: it holds no non-string map
        // key and no non-finite number.
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// `jq -c`, the wire form of `entry.json` and of the PATCH body.
fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// jq's `length` (L33), as `-r` printed it: `null` counts 0, a string
/// counts codepoints, an array its elements, an object its keys, and a
/// number is its own absolute value.  A boolean has no length, which
/// aborts the run.
fn length(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "0".to_owned(),
        Value::String(text) => text.chars().count().to_string(),
        Value::Array(items) => items.len().to_string(),
        Value::Object(map) => map.len().to_string(),
        Value::Number(number) => {
            let text = number.to_string();
            text.strip_prefix('-').unwrap_or(&text).to_owned()
        }
        Value::Bool(_) => bail!("boolean has no length"),
    })
}

/// jq's `join(",")` (L94, L204): `null` elements render empty, strings
/// render themselves, numbers and booleans render as JSON - and an
/// element that is itself an array or an object cannot be added to the
/// accumulated string, which aborts the run, as does joining anything
/// that cannot be iterated.  An object iterates its values.
fn join(value: &Value) -> Result<String> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => map.values().collect(),
        other => bail!("cannot iterate over {}", kind(other)),
    };
    let mut joined = String::new();
    for (position, item) in items.iter().enumerate() {
        if position > 0 {
            joined.push(',');
        }
        match item {
            Value::Null => {}
            Value::String(text) => joined.push_str(text),
            Value::Bool(_) | Value::Number(_) => joined.push_str(&compact(item)),
            other => bail!("{} cannot be joined", kind(other)),
        }
    }
    Ok(joined)
}

/// `.metadata.upstream.url // empty` (L157).  `//` takes its left side
/// as absent when it is null *or* false, so a stored `url: false` reads
/// as "no upstream" rather than as corrupt state; a `metadata` that is
/// not indexable aborts the run.
fn upstream_url(entry: &Value) -> Result<String> {
    let url = index(&index(&index(entry, "metadata")?, "upstream")?, "url")?;
    Ok(match url {
        Value::Null | Value::Bool(false) => String::new(),
        other => captured(&raw(&other)),
    })
}

/// L115-118.  The scoped name `<scope>/<name>` nests the artifact
/// directory while the filename flattens the `/` to `-` and ends in the
/// row's packaging revision, matching the registry's read route: each
/// revision of a version has its own URL, so this names exactly the
/// bytes the listing entry does.
fn artifact_url(origin: &str, name: &str, version: &str, revision: &str) -> String {
    let stem = name.replace('/', "-");
    format!("{origin}/artifacts/{name}/{stem}-{version}-{revision}.zip")
}

/// L223.  Concatenated, never percent-encoded: `$name` already carries
/// the `/` that makes this route three segments, and a version may
/// carry `+` and `.`.
fn verdict_url(api_origin: &str, name: &str, version: &str) -> String {
    format!("{api_origin}/api/v1/admin/versions/{name}/{version}")
}

/// The verdict body (L200-206), whose key order is the wire format:
/// `reason` sits between the verdict and the checksum, and only a
/// rejection carries it.
fn verdict_body(verdict: &str, reason: Option<&str>, checksum: &str, published_at: &str) -> String {
    let mut body = Map::new();
    body.insert("verdict".to_owned(), verdict.into());
    if let Some(reason) = reason {
        body.insert("reason".to_owned(), reason.into());
    }
    body.insert("checksum".to_owned(), checksum.into());
    body.insert("published_at".to_owned(), published_at.into());
    compact(&Value::Object(body))
}

/// `api_origin=$(jq -er '.api' <<<"$config")` (L25), the filter run
/// once per document of the captured body.  `-e` makes a `null` or
/// `false` last answer - a config.json that declares no API origin - an
/// abort, which is fail-safe: versions stay pending.
fn declared_api_origin(config: &str) -> Result<String> {
    let Some(documents) = answer(config, "config.json")? else {
        bail!("config.json is empty");
    };
    let rendered = read_stream(&documents, |doc| Ok(raw(&index(doc, "api")?)))?;
    // `-e` judges the LAST output value, after every document rendered.
    let last = index(documents.last().unwrap_or(&Value::Null), "api")?;
    if matches!(last, Value::Null | Value::Bool(false)) {
        bail!("config.json declares no api origin");
    }
    Ok(rendered)
}

/// `transfer_cap=$((upstream_budget < 120 ? upstream_budget : 120))`
/// (L175).
fn transfer_cap(budget: i64) -> i64 {
    budget.min(TRANSFER_CAP)
}

/// `spent=$((SECONDS - download_started))` with its `< 1` clamp
/// (L184-185): a difference of two whole-second reads, so a transfer
/// that crosses a second boundary is charged for it and one that does
/// not still costs the clamped second.
fn spent(began: u64, now: u64) -> i64 {
    i64::try_from(now.saturating_sub(began))
        .unwrap_or(i64::MAX)
        .max(1)
}

/// `"$verifier" …` (L84, L190): stdout captured as command
/// substitution captured it, stderr straight through to the job log,
/// stdin and the whole environment inherited - all four exactly as the
/// shell's child had them.  `None` is any non-zero exit, a verifier
/// that could not be spawned included, which is what the shell's
/// `if ! …` caught.
fn capture(command: &mut Command) -> Option<String> {
    let output = command
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| captured_bytes(&output.stdout))
}

/// L177-179: `curl -fsSL --proto '=https' --proto-redir '=https'
/// --connect-timeout 10 --max-time "$cap" --max-filesize 268435456`.
///
/// `ureq` has no per-hop scheme policy, so the redirect chain is walked
/// by hand over an agent that follows none of it, and no request in the
/// chain carries a credential.  `--max-time` bounds the whole transfer,
/// which an agent timeout alone would not: the deadline is re-derived
/// for every hop and checked again inside the body copy, so a
/// slow-dripping server cannot outlive the budget.
fn download_upstream(url: &str, path: &Path, cap: i64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(u64::try_from(cap).unwrap_or(0));
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if !is_https(&url) {
            bail!("the upstream url is not an absolute https url: {url}");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("the upstream transfer exceeded {cap}s");
        }
        // `--connect-timeout 10` ran INSIDE `--max-time`: a connect may
        // never spend more than what is left of the transfer cap, so
        // the hop's agent clamps one to the other.
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(CONNECT_TIMEOUT.min(remaining))
            .build();
        let response = agent.get(&url).timeout(remaining).call()?;
        match redirect(&response) {
            Some(location) => url = location,
            None => return store(response, path, deadline),
        }
    }
    bail!("the upstream url redirected more than {MAX_REDIRECTS} times")
}

/// `--proto '=https' --proto-redir '=https'`: scheme matching is
/// case-insensitive, as `curl`'s URL parser is.  The shell's own
/// lowercase glob ran before `curl` ever saw the *stored* URL, so only
/// a redirect hop can arrive spelled `HTTPS://` - and it must still be
/// absolute (the relative-`Location` ceiling in the module doc).
fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

/// `seq 0 $((count - 1))` (L66), as the GNU `seq` of the workflow's
/// runner walked it: a valid integer count of at least one walks
/// `0..n`, and everything else walks nothing - an empty capture
/// evaluates as zero, any other non-integer is an arithmetic error that
/// empties the expansion, and GNU `seq` counting from 0 to a negative
/// limit prints nothing (BSD `seq` would count DOWN through negative
/// `jq` indices; the workflow never ran there).  The shell carried on
/// to the failures check either way.
fn loop_indices(count: &str) -> Vec<usize> {
    let limit = if count.is_empty() {
        Some(0)
    } else {
        count.parse::<i64>().ok()
    };
    match limit {
        Some(limit) if limit >= 1 => (0..usize::try_from(limit).unwrap_or(0)).collect(),
        _ => Vec::new(),
    }
}

/// Where a 3xx points, which is the only reason to make another
/// request: `curl` follows a redirect exactly when it carries a
/// `Location`.
fn redirect(response: &ureq::Response) -> Option<String> {
    (300..400)
        .contains(&response.status())
        .then(|| response.header("Location"))
        .flatten()
        .map(str::to_owned)
}

/// The response body to a file, refusing the size cap on the declared
/// length *and* on what actually arrives - a server may understate
/// `Content-Length`, or send none at all.
fn store(response: ureq::Response, path: &Path, deadline: Instant) -> Result<()> {
    let declared = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    copy_capped(
        response.into_reader(),
        declared,
        path,
        deadline,
        MAX_UPSTREAM_BYTES,
    )
}

/// Split from [`store`] because a `ureq::Response` needs a live socket:
/// the cap and the deadline are proven over plain readers, and no
/// harness can overstate a `Content-Length` or move 256 MiB per run.
fn copy_capped(
    mut reader: impl Read,
    declared: Option<u64>,
    path: &Path,
    deadline: Instant,
    cap: u64,
) -> Result<()> {
    if let Some(declared) = declared
        && declared > cap
    {
        bail!("the upstream archive declares {declared} bytes");
    }
    let mut file = File::create(path)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        total += u64::try_from(read).unwrap_or(u64::MAX);
        if total > cap {
            bail!("the upstream archive exceeds {cap} bytes");
        }
        if Instant::now() >= deadline {
            bail!("the upstream transfer ran out of time");
        }
        file.write_all(&buffer[..read])?;
    }
}

#[cfg(test)]
mod tests {
    //! Every expectation below was taken from the shell the port
    //! replaces, replayed through `jq` 1.8.2 locally; the runner's
    //! image carries 1.7, and the module doc's rendering ceiling owns
    //! the drift between the two.  They live here rather than in
    //! `tests/` because what they exercise - how one interpreter
    //! rendered a value, and what the loop read that rendering to
    //! mean - is not this crate's API.

    use super::*;

    /// Command substitution edits exactly two things, and both change
    /// what reaches a PATCH body or a URL.
    #[test]
    fn command_substitution_drops_nuls_and_trailing_newlines() {
        assert_eq!(captured("plain"), "plain");
        assert_eq!(captured("trailing\n\n\n"), "trailing");
        assert_eq!(captured("a\0b"), "ab");
        assert_eq!(captured("keeps\nthe\nmiddle\n"), "keeps\nthe\nmiddle");
        assert_eq!(captured("\n"), "");
        assert_eq!(captured("\r\n"), "\r");
    }

    /// The NULs vanish from the BYTE stream before the text is read
    /// off it: a NUL splitting a UTF-8 sequence heals into the
    /// character bash saw, never into replacement characters.
    #[test]
    fn a_captured_body_drops_nul_bytes_before_decoding() {
        assert_eq!(captured_bytes(b"\xc2\x00\xa0"), "\u{a0}");
        assert_eq!(captured_bytes(b"plain\n"), "plain");
        assert_eq!(captured_bytes(b"\x00"), "");
        assert_eq!(captured_bytes(b"\xff"), "\u{fffd}");
    }

    /// `jq -r` renders a missing field and an explicit `null` alike, as
    /// the literal `null`, and a composite takes the indented form
    /// because none of these reads passed `-c`.
    #[test]
    fn a_raw_read_renders_null_as_its_own_text() {
        let entry = serde_json::json!({
            "name": "scope/pkg",
            "nulled": null,
            "number": 1.5,
            "object": { "x": 1 },
            "array": [1, 2],
        });
        assert_eq!(field(&entry, "name").unwrap(), "scope/pkg");
        assert_eq!(field(&entry, "nulled").unwrap(), "null");
        assert_eq!(field(&entry, "absent").unwrap(), "null");
        assert_eq!(field(&entry, "number").unwrap(), "1.5");
        assert_eq!(field(&entry, "object").unwrap(), "{\n  \"x\": 1\n}");
        assert_eq!(field(&entry, "array").unwrap(), "[\n  1,\n  2\n]");
        // A field read off something that is not indexable aborts the
        // whole run rather than counting one version's failure.
        assert!(field(&serde_json::json!("oops"), "name").is_err());
        assert_eq!(field(&Value::Null, "name").unwrap(), "null");
    }

    /// A verifier that exits 0 having printed nothing is not malformed
    /// data: `jq` over no input prints nothing, so the field read is
    /// empty and the version counts one failure and stays pending.
    /// Anything else it cannot parse aborts the whole run - documents
    /// before the malformed point notwithstanding.
    #[test]
    fn an_empty_child_answer_is_no_input_rather_than_a_parse_failure() {
        assert!(answer("", "an answer").unwrap().is_none());
        assert!(answer("   \n\t ", "an answer").unwrap().is_none());
        let advice = |text: &str| {
            read_stream(&answer(text, "an answer")?.unwrap(), |doc| {
                Ok(raw(&index(doc, "advice")?))
            })
        };
        assert_eq!(advice(r#"{"advice":"proceed"}"#).unwrap(), "proceed");
        assert!(answer("garbage", "an answer").is_err());
        assert!(answer("{", "an answer").is_err());
        assert!(answer(r#"{"advice":"proceed"} garbage"#, "an answer").is_err());
        // jq's lexer skips one leading byte-order mark (verified against
        // jq 1.8), and a mark alone is still no input at all.
        assert_eq!(
            advice("\u{feff}{\"advice\":\"proceed\"}").unwrap(),
            "proceed"
        );
        assert!(answer("\u{feff}", "an answer").unwrap().is_none());
        // "No input" is JSON's own whitespace alone: jq reads an
        // NBSP-only capture as a parse error (verified, exit 5), so it
        // aborts the run rather than reading as blank.
        assert!(answer("\u{a0}", "an answer").is_err());
    }

    /// `jq` runs its filter once per document of a multi-document
    /// answer, and `$(…)` strips what a trailing empty rendering leaves
    /// behind: a two-line advice matches no `case` arm and counts a
    /// failure, while a trailing `""` document vanishes into the arm its
    /// predecessor named - both exactly as the shell read them.
    #[test]
    fn a_multi_document_answer_reads_one_line_per_document() {
        let advice = |text: &str| {
            read_stream(&answer(text, "an answer").unwrap().unwrap(), |doc| {
                Ok(raw(&index(doc, "advice")?))
            })
        };
        assert_eq!(advice(r#"{"advice":"a"} {"advice":"b"}"#).unwrap(), "a\nb");
        assert_eq!(
            advice(r#"{"advice":"abstain"} {"advice":""}"#).unwrap(),
            "abstain"
        );
        assert_eq!(
            advice(r#"{"advice":"abstain"} {}"#).unwrap(),
            "abstain\nnull"
        );
        // A later document that cannot be indexed is a `jq` program
        // error mid-stream: the whole run aborts.
        assert!(advice(r#"{"advice":"proceed"} "oops""#).is_err());
    }

    /// `.versions | length` decides between "nothing pending" and the
    /// loop, so every shape it can meet matters.
    #[test]
    fn the_pending_count_is_jqs_length() {
        let length = |json: &str| length(&serde_json::from_str(json).unwrap());
        assert_eq!(length("null").unwrap(), "0");
        assert_eq!(length("[]").unwrap(), "0");
        assert_eq!(length("[1,2,3]").unwrap(), "3");
        assert_eq!(length(r#"{"a":1}"#).unwrap(), "1");
        assert_eq!(length(r#""café""#).unwrap(), "4");
        assert_eq!(length("-3").unwrap(), "3");
        assert_eq!(length("2.5").unwrap(), "2.5");
        assert!(length("true").is_err());
    }

    /// A listing with no `versions` key at all is "nothing pending",
    /// not an error - which is what keeps an empty admin answer from
    /// failing the cron.
    #[test]
    fn a_listing_without_versions_is_nothing_pending() {
        let listing: Value = serde_json::from_str("{}").unwrap();
        let versions = index(&listing, "versions").unwrap();
        assert_eq!(length(&versions).unwrap(), "0");
        assert!(at(&versions, 0).unwrap().is_null());
        // An object of versions has a length but cannot be indexed by
        // number, so it aborts at the first iteration.
        assert!(at(&serde_json::json!({ "a": 1 }), 0).is_err());
    }

    /// The findings and reason codes reach a log line and a PATCH body
    /// through `join`, whose element rules are not `tojson`'s: a
    /// composite element aborts the run.
    #[test]
    fn join_renders_the_elements_jq_could_add() {
        let join = |json: &str| join(&serde_json::from_str(json).unwrap());
        assert_eq!(join("[]").unwrap(), "");
        assert_eq!(join(r#"["one"]"#).unwrap(), "one");
        assert_eq!(join(r#"["a","b"]"#).unwrap(), "a,b");
        assert_eq!(join(r#"["a,b","c"]"#).unwrap(), "a,b,c");
        assert_eq!(join(r#"[null,1,true,"x"]"#).unwrap(), ",1,true,x");
        assert_eq!(join(r#"{"a":"x","b":"y"}"#).unwrap(), "x,y");
        assert!(join(r#"["a",["b"]]"#).is_err());
        assert!(join(r#"["a",{"b":1}]"#).is_err());
        assert!(join(r#""str""#).is_err());
        assert!(join("null").is_err());
    }

    /// `// empty` reads `false` as absent, which is the difference
    /// between "no upstream" and an operational refusal.
    #[test]
    fn an_upstream_url_is_absent_when_it_is_null_or_false() {
        let url = |json: &str| upstream_url(&serde_json::from_str(json).unwrap());
        let stored = |value: &str| format!(r#"{{"metadata":{{"upstream":{{"url":{value}}}}}}}"#);
        assert_eq!(
            url(&stored(r#""https://example.invalid/a.tar.gz""#)).unwrap(),
            "https://example.invalid/a.tar.gz"
        );
        assert_eq!(url(&stored("false")).unwrap(), "");
        assert_eq!(url(&stored("null")).unwrap(), "");
        assert_eq!(url(r#"{"metadata":{"upstream":{}}}"#).unwrap(), "");
        assert_eq!(url(r#"{"metadata":null}"#).unwrap(), "");
        assert_eq!(url("{}").unwrap(), "");
        // Anything else is rendered and then fails the https guard,
        // where metadata that cannot be indexed aborts the run.
        assert_eq!(url(&stored("5")).unwrap(), "5");
        assert_eq!(url(&stored("true")).unwrap(), "true");
        assert!(url(r#"{"metadata":"oops"}"#).is_err());
    }

    /// Both URLs are concatenated, and both carry a `/` inside a single
    /// path segment: percent-encoding either one would name a route the
    /// registry does not serve.
    #[test]
    fn the_urls_are_concatenated_around_a_scoped_name() {
        assert_eq!(
            artifact_url(
                "https://registry.cabinpkg.com",
                "my-scope/pkg",
                "10.2.1+cabin.1",
                "abcdef01"
            ),
            "https://registry.cabinpkg.com/artifacts/my-scope/pkg/\
             my-scope-pkg-10.2.1+cabin.1-abcdef01.zip"
        );
        assert_eq!(
            verdict_url("https://cabinpkg.com", "my-scope/pkg", "10.2.1+cabin.1"),
            "https://cabinpkg.com/api/v1/admin/versions/my-scope/pkg/10.2.1+cabin.1"
        );
    }

    /// The PATCH body is a wire format: the key order is the one the
    /// `jq -cn` object literal spelled, and the values are escaped
    /// rather than interpolated.
    #[test]
    fn the_verdict_body_keeps_the_literal_key_order() {
        assert_eq!(
            verdict_body("verified", None, "abc", "2026-01-01T00:00:00.000Z"),
            r#"{"verdict":"verified","checksum":"abc","published_at":"2026-01-01T00:00:00.000Z"}"#
        );
        assert_eq!(
            verdict_body(
                "rejected",
                Some("checksum_mismatch,upstream_mismatch"),
                "abc",
                "2026-01-01T00:00:00.000Z"
            ),
            concat!(
                r#"{"verdict":"rejected","reason":"checksum_mismatch,upstream_mismatch","#,
                r#""checksum":"abc","published_at":"2026-01-01T00:00:00.000Z"}"#
            )
        );
        assert_eq!(
            verdict_body("rejected", Some("a\"b\\c\nd\te"), "\u{e9}", ""),
            r#"{"verdict":"rejected","reason":"a\"b\\c\nd\te","checksum":"é","published_at":""}"#
        );
    }

    /// `entry.json` is the compact rendering, key order preserved, with
    /// no trailing newline - the bytes the verifier parses as a
    /// `PendingVersion`.
    #[test]
    fn the_listing_entry_is_written_compactly_in_its_own_key_order() {
        let entry: Value = serde_json::from_str(
            r#"{"name":"scope/pkg","version":"0.2.0","revision":"abcdef01",
                "checksum":"ff","published_at":"2026-01-01T00:00:00.000Z",
                "metadata":{"upstream":{"url":"https://example.invalid/a.zip"}}}"#,
        )
        .unwrap();
        assert_eq!(
            compact(&entry),
            concat!(
                r#"{"name":"scope/pkg","version":"0.2.0","revision":"abcdef01","checksum":"ff","#,
                r#""published_at":"2026-01-01T00:00:00.000Z","#,
                r#""metadata":{"upstream":{"url":"https://example.invalid/a.zip"}}}"#
            )
        );
    }

    /// `-e` turns a config.json that declares no API origin into an
    /// abort, which is what keeps a stripped config from being compared
    /// against the pin as the string `null`.
    #[test]
    fn a_config_without_an_api_origin_aborts() {
        assert_eq!(
            declared_api_origin(r#"{"api":"https://cabinpkg.com"}"#).unwrap(),
            "https://cabinpkg.com"
        );
        assert!(declared_api_origin("{}").is_err());
        assert!(declared_api_origin(r#"{"api":null}"#).is_err());
        assert!(declared_api_origin(r#"{"api":false}"#).is_err());
        assert!(declared_api_origin("not json").is_err());
        assert!(declared_api_origin(r#""str""#).is_err());
        assert!(declared_api_origin("").is_err());
        // `-e` judges the last document's rendering, after every
        // document printed: a two-document config renders two lines and
        // then fails the origin comparison rather than the `-e` check.
        assert!(declared_api_origin(r#"{"api":"https://x"} {"api":null}"#).is_err());
        assert_eq!(
            declared_api_origin(r#"{"api":null} {"api":"https://x"}"#).unwrap(),
            "null\nhttps://x"
        );
    }

    /// The budget arithmetic, over a clock that only ever reads whole
    /// seconds.  A transfer is charged at least one second so the
    /// aggregate bound holds against sub-second transfers, and no
    /// single transfer may take more than the per-transfer ceiling.
    #[test]
    fn the_budget_charges_whole_seconds_and_caps_one_transfer() {
        assert_eq!(transfer_cap(300), 120);
        assert_eq!(transfer_cap(121), 120);
        assert_eq!(transfer_cap(120), 120);
        assert_eq!(transfer_cap(119), 119);
        assert_eq!(transfer_cap(1), 1);

        assert_eq!(spent(5, 5), 1, "a transfer inside one second still costs");
        assert_eq!(spent(5, 6), 1);
        assert_eq!(spent(5, 7), 2);
        assert_eq!(spent(7, 5), 1, "a clock that did not advance costs one");

        // 300 seconds of whole-second transfers exhaust the budget and
        // no further version pays for one.
        let mut budget = BUDGET;
        for _ in 0..3 {
            budget -= spent(0, u64::try_from(transfer_cap(budget)).unwrap());
        }
        assert_eq!(budget, 0);
        assert!(budget <= 0, "the next version verifies without upstream");
    }

    /// The walk `seq 0 $((count - 1)) | shuf` expanded to, GNU `seq`
    /// semantics included: only a count spelling an integer of at least
    /// one walks anything.  A float, a multi-line count off a
    /// multi-document listing, and an empty capture all empty the
    /// expansion, and the run then finishes clean having processed
    /// nothing - the shell's own behavior, verified against bash.
    #[test]
    fn a_non_integer_count_walks_nothing() {
        assert_eq!(loop_indices("3"), [0, 1, 2]);
        assert_eq!(loop_indices("1"), [0]);
        assert!(loop_indices("0").is_empty());
        assert!(loop_indices("").is_empty());
        assert!(loop_indices("2.5").is_empty());
        assert!(loop_indices("3\n2").is_empty());
        assert!(loop_indices("-3").is_empty());
        assert!(loop_indices("three").is_empty());
    }

    /// `--proto-redir '=https'` matches the scheme case-insensitively,
    /// so an uppercase redirect hop is followed where a cleartext or
    /// relative one fails the download.
    #[test]
    fn the_redirect_scheme_matches_case_insensitively() {
        assert!(is_https("https://example.invalid/a"));
        assert!(is_https("HTTPS://example.invalid/a"));
        assert!(is_https("hTtPs://example.invalid/a"));
        assert!(!is_https("http://example.invalid/a"));
        assert!(!is_https("HTTP://example.invalid/a"));
        assert!(!is_https("/relative/path"));
        assert!(!is_https("https:/"));
        assert!(!is_https(""));
    }

    /// `--max-filesize` fires on an overstated `Content-Length` before
    /// a byte moves and on the counted bytes as they arrive, and the
    /// whole-transfer deadline lives inside the same loop.  The caps
    /// here are small stand-ins: the real 256 MiB constant reaches
    /// [`copy_capped`] only through [`store`], and moving that much per
    /// test is what this seam exists to avoid.
    #[test]
    fn the_upstream_copy_refuses_the_cap_and_the_deadline() {
        let dir = tempfile::TempDir::new().expect("a scratch directory");
        let path = dir.path().join("upstream-archive");
        let soon = Instant::now() + Duration::from_mins(1);
        // Declared over the cap: refused before a byte moves.
        assert!(copy_capped(std::io::empty(), Some(9), &path, soon, 8).is_err());
        // Understated declaration: the count catches what arrives, and
        // so does a body that declared nothing at all.
        assert!(copy_capped(std::io::repeat(0).take(9), Some(1), &path, soon, 8).is_err());
        assert!(copy_capped(std::io::repeat(0).take(9), None, &path, soon, 8).is_err());
        // Under the cap: the bytes land.
        copy_capped(std::io::repeat(7).take(8), None, &path, soon, 8).expect("under the cap");
        assert_eq!(std::fs::read(&path).expect("the stored file"), [7; 8]);
        // A deadline already passed stops a transfer that still has
        // bytes - but an empty body completes: there is no read left to
        // run out of time.  `now` is over by the time the loop looks.
        let passed = Instant::now();
        assert!(copy_capped(std::io::repeat(0).take(9), None, &path, passed, 100).is_err());
        copy_capped(std::io::empty(), None, &path, passed, 8).expect("an empty body");
    }

    /// The walk order must be a permutation of the listing and must not
    /// be the listing's own order every run: the admin listing is
    /// deterministically sorted, so a stable order lets slow upstream
    /// hosts starve everything behind them.
    #[test]
    fn the_walk_order_is_a_shuffled_permutation() {
        let identity: Vec<usize> = (0..8).collect();
        let mut shuffled = 0;
        for _ in 0..64 {
            let mut order = identity.clone();
            order.shuffle(&mut rand::rng());
            let mut sorted = order.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, identity, "the order dropped or repeated a version");
            if order != identity {
                shuffled += 1;
            }
        }
        assert!(shuffled > 0, "the order never changed across 64 draws");
    }
}
