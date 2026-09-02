//! The verification pass
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
//! them - and never a credential, the corpus, or archive bytes.
//!
//! Two credentials, two planes (`registry/docs/runbook.md`,
//! "Verification pipeline"): the listings and downloads ride a
//! `verify`-scoped registry token the run mints for itself at start -
//! one exchange-audience JWT through the trusted-publishing exchange's
//! verifier arm (`registry/src/trustpub.rs`), revoked best-effort at
//! every clean exit and left to its half-hour expiry on an abort -
//! while each verdict PATCH carries a GitHub Actions OIDC JWT minted
//! fresh for it (`Minter`) - the registry consumes each token's `jti`
//! on use, so no JWT can authenticate twice.
//!
//! A dispatched run can instead resolve ONE abstained version
//! (`Resolution`, threaded through the `VERIFY_RESOLVE*` env vars):
//! `verify` waives the name advisories and lets the real checks
//! render the verdict, `reject` delivers the advisory rejection the
//! operator confirmed by hand.
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
//! The verifier child inherits the whole environment, exactly as the
//! shell's child did, minus the OIDC mint pair and any inherited
//! `CABIN_REGISTRY_TOKEN`, all scrubbed (`capture`): the pair could
//! mint the verdict-delivering JWT, and the token variable - never
//! set by the workflow, but a local operator shell may export it
//! (`registry/docs/runbook.md`) - is a credential the child has no
//! use for.  The run's own registry token is minted at start and
//! lives only in driver memory.  The child reads only the `VERIFY_*`
//! caps, and removing the rest would be a change rather than a port.
//! The publisher-controlled upstream download is the one request that
//! never sees a credential, and it runs on its own agent so it cannot.
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
//! - an unset `REGISTRY_ORIGIN` or `EXPECTED_API_ORIGIN` reads as
//!   empty, where `set -u` aborted with bash's unbound-variable
//!   message.  Same exit code, and the workflow sets both;
//! - the upstream budget models bash's `$SECONDS`: a whole-second
//!   clock anchored at process start, charged as the difference of two
//!   reads of it rather than as real elapsed time, so a transfer that
//!   straddles a second boundary is charged for it;
//! - the CA roots are `ureq`'s compiled-in `webpki-roots`, where `curl`
//!   read `CURL_CA_BUNDLE`, `SSL_CERT_FILE` and the system store - the
//!   same environment ceiling the crate's proxy handling carries
//!   (`crate::audit`);
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
//!   string (`registry/src/glue/bearer/verifier.rs`, `AdminVersionRecord`);
//!   `published_by` is its one integer, and nothing here reads it.
//!   `jq`'s own input extensions - `NaN` and kin read as `null`,
//!   lenient numeric forms like a leading zero, `+1` or `1.`, lone
//!   surrogates replaced - abort here as unparsable;
//! - every parse here carries `serde_json`'s 128-level recursion cap,
//!   where `jq` read deeper.  The registry parses each metadata
//!   document with the same cap at publish and again when the listing
//!   embeds it (`registry/src/publish.rs`, `registry/src/glue/bearer/verifier.rs`), so
//!   only a document within the listing wrapper's few levels of the
//!   cap can thread the needle - and it aborts the run fail-safe
//!   (everything stays pending, the run goes red) where the shell
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
use base64ct::{Base64UrlUnpadded, Encoding as _};
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
/// pin the same early-sorting versions to the front of every pass
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
    let minter = minter();
    let resolution = resolution();

    // The run's registry credential is minted, not configured: the
    // exchange stands in for the retired `REGISTRY_VERIFY_TOKEN`
    // secret, and its refusal fails the run before any listing is
    // read, so every version stays pending.
    let agent = plane_agent();
    let token = exchange(&agent, &minter, &expected_api_origin)
        .context("the trusted-publishing exchange failed")?;
    let plane = Plane { agent, token };

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
    if resolution.is_none() {
        if count.parse::<i64>().ok() == Some(0) {
            println!("nothing pending");
            let _ = plane.revoke(&api_origin);
            return Ok(());
        }
        println!("{count} pending version(s)");
    }

    // L40-47: the corpus for the name advisories, fetched once.  A
    // failed fetch fails the run - without it no advisory can run, and
    // proceeding could verify a confusable name.  Only a walk fetches
    // it: no resolution action reads it (`reject` renders no advisory,
    // `verify` waives them), so a resolution must not be coupled to
    // this endpoint's availability.
    let corpus = if resolution.is_none() {
        let corpus =
            tempfile::NamedTempFile::new().context("open a temporary file for the corpus")?;
        plane
            .download(
                &format!("{api_origin}/api/v1/admin/packages"),
                corpus.path(),
            )
            .context("the package corpus request failed")?;
        Some(corpus)
    } else {
        None
    };

    let mut pass = Pass {
        plane,
        minter,
        registry_origin,
        api_origin,
        corpus: corpus.as_ref().map(|corpus| corpus.path().to_owned()),
        started,
        budget: BUDGET,
    };

    // The walk (and the resolution lookup) read a single-document
    // listing: a multi-document count carries a newline and never
    // survives the arithmetic in [`loop_indices`].
    let versions = documents
        .first()
        .map(|doc| index(doc, "versions"))
        .transpose()?
        .unwrap_or(Value::Null);

    // A dispatched resolution touches exactly the named version and
    // nothing else; every other pending version waits for a normal
    // pass.
    if let Some(resolution) = resolution {
        let resolved = pass.resolve(&versions, &resolution);
        let _ = pass.plane.revoke(&pass.api_origin);
        return resolved;
    }

    // L66: the listing is sorted, so the order it is walked in must
    // not be.
    let mut order = loop_indices(&count);
    order.shuffle(&mut rand::rng());

    let mut failures = 0_u64;
    for position in order {
        if pass.version(&at(&versions, position)?, RunAdvisories::Yes)? {
            failures += 1;
        }
    }
    // Revoked here rather than after the failure check: the abort
    // below exits the process, and the token should not outlive a run
    // that got this far either way.
    let _ = pass.plane.revoke(&pass.api_origin);
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

    /// L219-223: the verdict - the one request whose credential is not
    /// the plane's token: verdicts authenticate only the workflow's own
    /// OIDC JWT (`registry/src/trustpub.rs`, `VERIFIER_AUDIENCE`).  A
    /// 409 means the row changed since the listing (a republish, a
    /// competing verdict) and the next run sees the replacement.
    /// `-o /dev/null` still drained the transfer, so a response body
    /// that cuts out mid-way is a counted PATCH failure, never a
    /// success line.
    fn patch(&self, url: &str, body: &str, jwt: &str) -> Result<()> {
        let response = self
            .agent
            .request("PATCH", url)
            .set("authorization", &format!("Bearer {jwt}"))
            .set("content-type", "application/json")
            .send_string(body)?;
        std::io::copy(&mut response.into_reader(), &mut std::io::sink())?;
        Ok(())
    }

    /// The run-minted token revokes itself
    /// (`DELETE /api/v1/trusted_publishing/tokens`).  Best-effort at
    /// the walk's exits - a failure changes nothing the 30-minute
    /// expiry does not already bound, and an aborted run never gets
    /// here at all - but the diagnostic mode propagates it, because
    /// proving revocation is that mode's point.
    fn revoke(&self, api_origin: &str) -> Result<()> {
        let response = self
            .request(
                "DELETE",
                &format!("{api_origin}/api/v1/trusted_publishing/tokens"),
            )
            .call()?;
        // The agent follows no redirect, so a 3xx is the answer
        // itself - and only the contract's 204 revoked anything.
        if response.status() != 204 {
            bail!("the revocation answered {}", response.status());
        }
        std::io::copy(&mut response.into_reader(), &mut std::io::sink())?;
        Ok(())
    }
}

/// The audience the verdict endpoint pins (`registry/src/trustpub.rs`,
/// `VERIFIER_AUDIENCE`).  Appended to the mint URL raw: a `/` is legal
/// in a query value, and GitHub echoes the parameter back verbatim as
/// the `aud` claim.
const AUDIENCE: &str = "cabinpkg.com/verifier";

/// The audience the trusted-publishing exchange verifies
/// (`registry/src/trustpub.rs`, `DEFAULT_AUDIENCE`): the same pinned
/// identity, minted for the registry's own audience, is what the
/// exchange's verifier arm answers with a verify-scoped token.
const EXCHANGE_AUDIENCE: &str = "cabinpkg.com";

/// The verdict credential's mint: `id-token: write` points
/// `ACTIONS_ID_TOKEN_REQUEST_URL` at an endpoint that answers a
/// bearer-authenticated GET with `{"value":"<jwt>"}`.  Its agent
/// follows no redirect for the same reason the token plane's does not:
/// the request carries a credential.
struct Minter {
    agent: ureq::Agent,
    url: String,
    request_token: String,
}

impl Minter {
    fn mint(&self, audience: &str) -> Result<String> {
        let mut body = Vec::new();
        self.agent
            .request("GET", &mint_url(&self.url, audience))
            .set("authorization", &format!("Bearer {}", self.request_token))
            .call()?
            .into_reader()
            .read_to_end(&mut body)?;
        minted_token(&captured_bytes(&body))
    }
}

/// The mint's env pair, populated when the workflow grants
/// `id-token: write`.  An absent pair fails the whole run up front -
/// every verdict would 401 - and the guard is spelled as an https
/// check so a cleartext endpoint can never see the request token.
fn minter() -> Minter {
    let url = env("ACTIONS_ID_TOKEN_REQUEST_URL");
    if !url.starts_with("https://") {
        abort(
            "ACTIONS_ID_TOKEN_REQUEST_URL must be https; does the workflow grant id-token: write?",
        );
    }
    let request_token = env("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    if request_token.is_empty() {
        abort("ACTIONS_ID_TOKEN_REQUEST_TOKEN is not populated");
    }
    Minter {
        agent: plane_agent(),
        url,
        request_token,
    }
}

/// The mint URL as GitHub populates it already carries a query string
/// (`api-version=…`), so the audience appends with `&`.
fn mint_url(request_url: &str, audience: &str) -> String {
    format!("{request_url}&audience={audience}")
}

/// The run's registry credential, minted through
/// `PUT /api/v1/trusted_publishing/tokens` rather than carried as a
/// repository secret: the JWT in the body is the request's only
/// credential, and the answered token is verify-scoped with a
/// 30-minute lifetime the workflow's 15-minute job timeout sits well
/// inside.  The exchange consumes the JWT's `jti` exactly as a
/// verdict does, so the JWT it spends is minted for it alone.  A
/// deliberate re-implementation, not a `cabin-registry-api` call
/// (`docs/architecture.md`, `xtask-registry-admin`).
fn exchange(agent: &ureq::Agent, minter: &Minter, api_origin: &str) -> Result<String> {
    let jwt = minter
        .mint(EXCHANGE_AUDIENCE)
        .context("the exchange JWT mint failed")?;
    let response = agent
        .request(
            "PUT",
            &format!("{api_origin}/api/v1/trusted_publishing/tokens"),
        )
        .set("content-type", "application/json")
        .send_string(&serde_json::json!({ "jwt": jwt }).to_string())?;
    // The agent follows no redirect, so a 3xx is the answer itself -
    // and only the contract's 200 carries a token.
    if response.status() != 200 {
        bail!("the exchange answered {}", response.status());
    }
    let mut body = Vec::new();
    response.into_reader().read_to_end(&mut body)?;
    exchanged_token(&captured_bytes(&body))
}

/// The `token` field of the exchange answer, which must be a
/// non-empty string, exactly as [`minted_token`] reads the mint's
/// `value`.
fn exchanged_token(body: &str) -> Result<String> {
    let answer: Value = serde_json::from_str(body).context("the exchange answer is not JSON")?;
    match index(&answer, "token")? {
        Value::String(token) if !token.is_empty() => Ok(token),
        _ => bail!("the exchange answer carries no token"),
    }
}

/// The `value` field of the mint answer, which must be a non-empty
/// string: anything else is the mint failing, never a token.
fn minted_token(body: &str) -> Result<String> {
    let answer: Value = serde_json::from_str(body).context("the mint answer is not JSON")?;
    match index(&answer, "value")? {
        Value::String(token) if !token.is_empty() => Ok(token),
        _ => bail!("the mint answer carries no token"),
    }
}

/// The JWT's payload segment, decoded and parsed.  The claims are all
/// public repository facts (the signature over them is the
/// credential), so the check mode prints them, and never the token
/// itself, which Actions does not mask.
fn claims(jwt: &str) -> Result<Value> {
    let payload = jwt
        .split('.')
        .nth(1)
        .context("the token has no payload segment")?;
    let bytes = Base64UrlUnpadded::decode_vec(payload)
        .ok()
        .context("the payload segment is not base64url")?;
    serde_json::from_slice(&bytes).context("the payload is not JSON")
}

/// The OIDC diagnostic (`cargo registry-verify --check-oidc`): mint
/// two tokens, print the first one's claims against the server-side
/// pins, prove the per-mint `jti` freshness the verdict design leans
/// on, then walk the trusted-publishing exchange round trip - mint,
/// exchange, revoke - all without sending a verdict.
///
/// # Errors
///
/// If a mint fails, a token will not decode, the two mints share a
/// `jti`, or the exchange or the revocation refuses.
pub fn check_oidc() -> Result<()> {
    let minter = minter();
    // Guarded before the first mint, like `run()`: every environment
    // guard fires before any network I/O.
    let api_origin = env("EXPECTED_API_ORIGIN");
    if !api_origin.starts_with("https://") {
        abort(&format!(
            "EXPECTED_API_ORIGIN must be https, got: {api_origin}"
        ));
    }

    let first = claims(&minter.mint(AUDIENCE).context("the first mint failed")?)?;
    let second = claims(&minter.mint(AUDIENCE).context("the second mint failed")?)?;
    println!("{}", serde_json::to_string_pretty(&first)?);
    if index(&first, "jti")? == index(&second, "jti")? {
        bail!("two mints returned the same jti; per-PATCH minting would replay");
    }
    println!("check-oidc OK (two mints, distinct jtis)");

    let agent = plane_agent();
    let token =
        exchange(&agent, &minter, &api_origin).context("the trusted-publishing exchange failed")?;
    Plane { agent, token }
        .revoke(&api_origin)
        .context("revoking the exchanged token failed")?;
    println!("check-oidc OK (exchanged and revoked a verify-scoped token)");
    Ok(())
}

/// What one pass carries from version to version: the plane, the
/// verdict credential's mint, the two origins, the corpus every
/// advisory reads, and the shared upstream budget.
struct Pass {
    plane: Plane,
    minter: Minter,
    registry_origin: String,
    api_origin: String,
    corpus: Option<PathBuf>,
    started: Instant,
    budget: i64,
}

impl Pass {
    /// L67-230, one pending version.  The `bool` is the shell's
    /// `failures=$((failures + 1))`: every exit from the loop body
    /// leaves the version pending, and only some of them count.
    fn version(&mut self, entry: &Value, advisories: RunAdvisories) -> Result<bool> {
        let name = field(entry, "name")?;
        let version = field(entry, "version")?;
        let revision = field(entry, "revision")?;
        let checksum = field(entry, "checksum")?;
        let published_at = field(entry, "published_at")?;

        let workdir = tempfile::TempDir::new().context("open a work directory")?;
        let entry_path = workdir.path().join("entry.json");
        std::fs::write(&entry_path, compact(entry)).context("write the listing entry")?;

        // A dispatched `verify` resolution waives exactly the advisory
        // gate - the real checks below still gate the verdict.
        if advisories == RunAdvisories::Yes
            && let Some(counted) = self.name_advisories(&entry_path, &name, &version)?
        {
            return Ok(counted);
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

        // The verdict binds to the listing's checksum, so the bytes
        // the verifier inspects must be the bytes that checksum names.
        // A mismatch says the registry handed over different bytes
        // (skew, corruption), nothing about the publisher: pending is
        // recoverable where a false rejection is terminal.
        match undeclared_digest(&archive, &checksum) {
            Ok(None) => {}
            Ok(Some(digest)) => {
                eprintln!(
                    "{name}@{version}: the verifier saw a different artifact \
                     (sha256:{digest}, listed {checksum}); leaving it pending"
                );
                return Ok(true);
            }
            Err(_) => {
                eprintln!(
                    "{name}@{version}: reading the downloaded archive failed; leaving it pending"
                );
                return Ok(true);
            }
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

        // Minted here rather than at startup: the downloads and the
        // verifier can take minutes against a JWT's short validity,
        // and the registry consumes each jti on use, so only one
        // fresh token per PATCH authenticates every verdict.
        let Ok(jwt) = self.minter.mint(AUDIENCE) else {
            eprintln!("{name}@{version}: OIDC token mint failed; leaving it pending");
            return Ok(true);
        };
        if self
            .plane
            .patch(&verdict_url(&self.api_origin, &name, &version), &body, &jwt)
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

    /// L77-89, the advisory gate: the name advisories run before the
    /// download - they need no bytes, so an abstained version costs a
    /// listing entry per pass and never a re-download.  Abstain
    /// renders no verdict and is deliberately not a failure: the
    /// version stays pending until the stuck-pending alert summons an
    /// operator (`registry/docs/runbook.md`, "Verification pipeline").
    /// `None` proceeds to the download; `Some(counted)` leaves the
    /// version pending, counted as an operational failure or not.
    fn name_advisories(
        &self,
        entry_path: &Path,
        name: &str,
        version: &str,
    ) -> Result<Option<bool>> {
        // Fail-closed, checked before the spawn: only a resolution
        // leaves the corpus unfetched, and a resolution never runs the
        // advisories - an empty stand-in here would advise `proceed`
        // for any name.
        let Some(corpus) = self.corpus.as_deref() else {
            bail!("the name advisories need the package corpus, which a resolution never fetches");
        };
        let Some(advice_json) = capture(
            Command::new(VERIFIER)
                .arg("--name-advisories")
                .arg(entry_path)
                .arg(corpus),
        ) else {
            eprintln!("{name}@{version}: name advisories failed operationally; leaving it pending");
            return Ok(Some(true));
        };
        let Some(advice_docs) = answer(&advice_json, "the name advisories answer")? else {
            eprintln!("{name}@{version}: unknown advice ''; leaving it pending");
            return Ok(Some(true));
        };
        let advice = read_stream(&advice_docs, |doc| Ok(raw(&index(doc, "advice")?)))?;
        match advice.as_str() {
            "proceed" => Ok(None),
            "abstain" => {
                let findings = read_stream(&advice_docs, |doc| join(&index(doc, "findings")?))?;
                println!(
                    "{name}@{version}: abstain ({findings}); \
                     leaving it pending for operator review"
                );
                Ok(Some(false))
            }
            _ => {
                eprintln!("{name}@{version}: unknown advice '{advice}'; leaving it pending");
                Ok(Some(true))
            }
        }
    }

    /// bash's `$SECONDS`: whole seconds since the process started.
    fn seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// One dispatched resolution, touching exactly the named version.
    /// Unlike the walk, every operational failure aborts loudly - the
    /// operator is watching this run - and the version stays pending
    /// for a re-dispatch either way.
    fn resolve(&mut self, versions: &Value, resolution: &Resolution) -> Result<()> {
        let Resolution { name, version, .. } = resolution;
        let Some(entry) = find_entry(versions, name, version, resolution.revision.as_deref())?
        else {
            abort(&format!("{name}@{version} is not in the pending listing"));
        };
        // The verdict binds to this entry's checksum, so the revision
        // it selects must be visible in the run's own output.
        println!(
            "resolving {name}@{version} revision {}",
            field(&entry, "revision")?
        );
        match &resolution.action {
            Action::Verify => {
                if self.version(&entry, RunAdvisories::Waived)? {
                    abort(&format!(
                        "{name}@{version}: the waived verification failed \
                         operationally and stays pending"
                    ));
                }
            }
            Action::Reject { rule } => {
                let checksum = field(&entry, "checksum")?;
                let published_at = field(&entry, "published_at")?;
                let reason = format!("name_advisory: {rule}");
                let body = verdict_body("rejected", Some(&reason), &checksum, &published_at);
                let Ok(jwt) = self.minter.mint(AUDIENCE) else {
                    abort(&format!(
                        "{name}@{version}: OIDC token mint failed; still pending"
                    ));
                };
                if self
                    .plane
                    .patch(&verdict_url(&self.api_origin, name, version), &body, &jwt)
                    .is_err()
                {
                    abort(&format!(
                        "{name}@{version}: verdict PATCH failed; still pending"
                    ));
                }
                println!("{name}@{version}: rejected ({reason})");
            }
        }
        Ok(())
    }
}

/// Whether [`Pass::version`] runs the name advisories: every normal
/// walk does, and only a dispatched `verify` resolution waives them -
/// the operator has reviewed the abstained name by then, and the
/// archive checks still gate the verdict either way.
#[derive(Clone, Copy, PartialEq)]
enum RunAdvisories {
    Yes,
    Waived,
}

/// One dispatched operator resolution (`registry/docs/runbook.md`,
/// "Verification pipeline"): the abstained version it names - plus
/// the revision selector when two pending revisions share it - and
/// what to do about it.
struct Resolution {
    name: String,
    version: String,
    revision: Option<String>,
    action: Action,
}

/// `verify` waives the name advisories and delivers whatever verdict
/// the real checks render - never `verified` from the name alone;
/// `reject` delivers the advisory rejection the operator confirmed.
/// Neither action re-checks that the version ever abstained (abstain
/// is a per-run advisory outcome, persisted nowhere): the dispatch is
/// the operator's authority, and it grants nothing repository write
/// access does not already imply - the same writer could edit this
/// client on `main` and hold the pinned OIDC identity outright.
enum Action {
    Verify,
    Reject { rule: String },
}

/// The dispatch inputs, threaded through as env vars so the workflow's
/// `run:` block stays a plain invocation.  An empty `VERIFY_RESOLVE`
/// is a normal pass - the dispatch form always submits its action
/// default, so a stray `verify` alone means nothing - but a reason or
/// an explicit `reject` without a target is an inconsistent form and
/// refuses rather than walking the listing as if nothing were asked.
fn resolution() -> Option<Resolution> {
    const SHAPE: &str = "VERIFY_RESOLVE must be <scope>/<name>@<version>[#<revision>]";
    let target = env("VERIFY_RESOLVE");
    let action = env("VERIFY_RESOLVE_ACTION");
    let rule = env("VERIFY_RESOLVE_REASON");
    if target.is_empty() {
        if !rule.is_empty() {
            abort("VERIFY_RESOLVE_REASON is set without VERIFY_RESOLVE");
        }
        if action == "reject" {
            abort("the reject action needs VERIFY_RESOLVE");
        }
        return None;
    }
    let (name, version) = match target.split_once('@') {
        Some((name, version)) if name.contains('/') && !version.is_empty() => (name, version),
        _ => abort(SHAPE),
    };
    // `#` is not legal in a version, so it can carry the revision
    // selector for the one ambiguous case: two pending revisions of
    // the same name and version (a `new-revision` republish while the
    // first still pends).
    let (version, revision) = match version.split_once('#') {
        None => (version, None),
        Some((version, revision)) if !version.is_empty() && !revision.is_empty() => {
            (version, Some(revision.to_owned()))
        }
        Some(_) => abort(SHAPE),
    };
    let action = match action.as_str() {
        "verify" => {
            if !rule.is_empty() {
                abort("VERIFY_RESOLVE_REASON is only for the reject action");
            }
            Action::Verify
        }
        "reject" => {
            if rule.is_empty() {
                abort("the reject action needs VERIFY_RESOLVE_REASON");
            }
            Action::Reject { rule }
        }
        other => abort(&format!("unknown VERIFY_RESOLVE_ACTION '{other}'")),
    };
    Some(Resolution {
        name: name.to_owned(),
        version: version.to_owned(),
        revision,
        action,
    })
}

/// The one listing entry naming `<name>@<version>` (and `<revision>`,
/// when the target carries the selector).  A listing with no
/// `versions` at all holds no entry; anything that is not an array is
/// malformed listing data and aborts, as every other listing read
/// does.  More than one match - two pending revisions of the same
/// version - aborts rather than silently binding the verdict to a
/// revision the operator may not have reviewed.
fn find_entry(
    versions: &Value,
    name: &str,
    version: &str,
    revision: Option<&str>,
) -> Result<Option<Value>> {
    let entries = match versions {
        Value::Array(entries) => entries.as_slice(),
        Value::Null => &[],
        other => bail!("cannot search {} for a version", kind(other)),
    };
    let mut matched = Vec::new();
    for entry in entries {
        if field(entry, "name")? != name || field(entry, "version")? != version {
            continue;
        }
        if let Some(revision) = revision
            && field(entry, "revision")? != revision
        {
            continue;
        }
        matched.push(entry.clone());
    }
    if matched.len() > 1 {
        let revisions: Vec<String> = matched
            .iter()
            .map(|entry| field(entry, "revision"))
            .collect::<Result<_>>()?;
        bail!(
            "{name}@{version} has {} pending revisions ({}); \
             disambiguate with @<version>#<revision>",
            matched.len(),
            revisions.join(", ")
        );
    }
    Ok(matched.pop())
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

/// The downloaded archive's digest when it is NOT the listing's
/// checksum, for the diagnostic; `None` is the match.  The comparison
/// is against the stored string whole, canonical `sha256:<hex>` form
/// included, so a stored checksum in any other spelling simply never
/// matches - the same operational-failure path as a real mismatch.
fn undeclared_digest(archive: &Path, checksum: &str) -> std::io::Result<Option<String>> {
    let digest = File::open(archive).and_then(cabin_core::hash::hash_reader)?;
    Ok((format!("sha256:{digest}") != checksum).then_some(digest))
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
/// stdin and the environment inherited as the shell's child had them -
/// minus the credentials.  The child parses publisher-controlled
/// bytes: the request token could mint the verdict-delivering JWT,
/// and `CABIN_REGISTRY_TOKEN` - unset in the workflow, but a local
/// operator shell may export it for the governor tools - is a
/// registry credential it has no use for.  The run's own registry
/// token lives only in driver memory.  `None` is any non-zero exit, a
/// verifier that could not be spawned included, which is what the
/// shell's `if ! …` caught.
fn capture(command: &mut Command) -> Option<String> {
    let output = command
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .env_remove(cabin_env::CABIN_REGISTRY_TOKEN)
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
    /// failing the run.
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

    /// The resolution lookup matches the listing's own rendered
    /// `name`/`version` strings exactly - the abstain log line is
    /// where the operator copies them from - and a listing shape no
    /// other read would accept aborts here too.
    #[test]
    fn a_resolution_finds_exactly_the_named_version() {
        let versions = serde_json::json!([
            { "name": "scope/pkg", "version": "1.0.0", "checksum": "aa" },
            { "name": "scope/pkg", "version": "2.0.0", "checksum": "bb" },
            { "name": "other/pkg", "version": "1.0.0", "checksum": "cc" },
        ]);
        let hit = find_entry(&versions, "scope/pkg", "2.0.0", None)
            .unwrap()
            .expect("the entry");
        assert_eq!(field(&hit, "checksum").unwrap(), "bb");
        assert!(
            find_entry(&versions, "scope/pkg", "3.0.0", None)
                .unwrap()
                .is_none()
        );
        assert!(
            find_entry(&versions, "Scope/Pkg", "1.0.0", None)
                .unwrap()
                .is_none(),
            "no case slack"
        );
        // No versions at all is "not found", not malformed data.
        assert!(
            find_entry(&Value::Null, "scope/pkg", "1.0.0", None)
                .unwrap()
                .is_none()
        );
        assert!(find_entry(&serde_json::json!({}), "scope/pkg", "1.0.0", None).is_err());
        // An entry that cannot be indexed is malformed listing data.
        assert!(find_entry(&serde_json::json!(["oops"]), "scope/pkg", "1.0.0", None).is_err());
    }

    /// Two pending revisions of the same version (a `new-revision`
    /// republish while the first still pends) must refuse rather than
    /// silently bind the verdict to whichever revision listed first;
    /// the `#<revision>` selector is the disambiguator.
    #[test]
    fn a_resolution_refuses_an_ambiguous_version_without_the_selector() {
        let versions = serde_json::json!([
            { "name": "scope/pkg", "version": "1.0.0", "revision": "r1", "checksum": "aa" },
            { "name": "scope/pkg", "version": "1.0.0", "revision": "r2", "checksum": "bb" },
        ]);
        let ambiguous = find_entry(&versions, "scope/pkg", "1.0.0", None).unwrap_err();
        assert_eq!(
            ambiguous.to_string(),
            "scope/pkg@1.0.0 has 2 pending revisions (r1, r2); \
             disambiguate with @<version>#<revision>"
        );
        let hit = find_entry(&versions, "scope/pkg", "1.0.0", Some("r2"))
            .unwrap()
            .expect("the selected revision");
        assert_eq!(field(&hit, "checksum").unwrap(), "bb");
        assert!(
            find_entry(&versions, "scope/pkg", "1.0.0", Some("r9"))
                .unwrap()
                .is_none()
        );
    }

    /// The advisory gate is what a dispatched `verify` resolution
    /// waives, and nothing else: with no corpus (a resolution never
    /// fetches one) a gated run fails closed on the corpus bail, while
    /// a waived run proceeds past the gate to the archive download -
    /// whose failure here (an origin with no host fails at URL parse,
    /// before any network I/O) counts operationally, as on any walk.
    #[test]
    fn the_advisory_gate_runs_only_on_a_normal_walk() {
        let mut pass = Pass {
            plane: Plane {
                agent: plane_agent(),
                token: "t".to_owned(),
            },
            minter: Minter {
                agent: plane_agent(),
                url: "https://".to_owned(),
                request_token: "rt".to_owned(),
            },
            registry_origin: "https://".to_owned(),
            api_origin: "https://".to_owned(),
            corpus: None,
            started: Instant::now(),
            budget: 0,
        };
        let entry = serde_json::json!({
            "name": "scope/pkg", "version": "1.0.0", "revision": "r1",
            "checksum": "ff", "published_at": "2026-01-01T00:00:00.000Z",
        });
        assert!(pass.version(&entry, RunAdvisories::Yes).is_err());
        assert!(pass.version(&entry, RunAdvisories::Waived).unwrap());
    }

    /// The mint URL appends the pinned audience to the query string
    /// GitHub's endpoint already carries, raw: percent-encoding the
    /// `/` would still verify (the server decodes), but the raw form
    /// is what GitHub's own documentation passes.
    #[test]
    fn the_mint_url_appends_the_audience_to_the_existing_query() {
        assert_eq!(
            mint_url("https://mint.invalid/token?api-version=2.0", AUDIENCE),
            "https://mint.invalid/token?api-version=2.0&audience=cabinpkg.com/verifier"
        );
        assert_eq!(
            mint_url(
                "https://mint.invalid/token?api-version=2.0",
                EXCHANGE_AUDIENCE
            ),
            "https://mint.invalid/token?api-version=2.0&audience=cabinpkg.com"
        );
    }

    /// The mint answer's token is its non-empty `value` string; every
    /// other shape is the mint failing, and the version stays pending.
    #[test]
    fn a_minted_token_is_the_non_empty_value_string() {
        assert_eq!(
            minted_token(r#"{"value":"aaa.bbb.ccc"}"#).unwrap(),
            "aaa.bbb.ccc"
        );
        assert!(minted_token(r#"{"value":""}"#).is_err());
        assert!(minted_token(r#"{"value":null}"#).is_err());
        assert!(minted_token(r#"{"value":5}"#).is_err());
        assert!(minted_token("{}").is_err());
        assert!(minted_token("not json").is_err());
        assert!(minted_token("").is_err());
    }

    /// The exchange answer's token is its non-empty `token` string;
    /// every other shape fails the exchange, and the run aborts before
    /// any listing is read.
    #[test]
    fn an_exchanged_token_is_the_non_empty_token_string() {
        assert_eq!(
            exchanged_token(r#"{"token":"cabin_tp_x","expires_at":"2026-08-20T00:00:00Z"}"#)
                .unwrap(),
            "cabin_tp_x"
        );
        assert!(exchanged_token(r#"{"token":""}"#).is_err());
        assert!(exchanged_token(r#"{"token":null}"#).is_err());
        assert!(exchanged_token(r#"{"expires_at":"2026-08-20T00:00:00Z"}"#).is_err());
        assert!(exchanged_token("not json").is_err());
        assert!(exchanged_token("").is_err());
    }

    /// Only the canonical `sha256:<hex>` spelling of the archive's own
    /// digest matches; any other stored form - bare hex, uppercase, a
    /// different digest - reads as a mismatch and leaves the version
    /// pending.
    #[test]
    fn only_the_canonical_checksum_of_the_downloaded_bytes_matches() {
        const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let archive = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(archive.path(), b"abc").unwrap();
        assert_eq!(
            undeclared_digest(archive.path(), &format!("sha256:{ABC}")).unwrap(),
            None
        );
        assert_eq!(
            undeclared_digest(archive.path(), ABC).unwrap().as_deref(),
            Some(ABC)
        );
        assert_eq!(
            undeclared_digest(archive.path(), &format!("sha256:{}", ABC.to_uppercase()))
                .unwrap()
                .as_deref(),
            Some(ABC)
        );
        assert_eq!(
            undeclared_digest(archive.path(), "sha256:0000")
                .unwrap()
                .as_deref(),
            Some(ABC)
        );
        assert!(undeclared_digest(&archive.path().with_extension("missing"), ABC).is_err());
    }

    /// The claims are the decoded payload segment, and nothing else of
    /// the token is ever read - verification is the server's job.
    #[test]
    fn claims_decode_the_payload_segment() {
        let payload =
            Base64UrlUnpadded::encode_string(br#"{"jti":"one","aud":"cabinpkg.com/verifier"}"#);
        let decoded = claims(&format!("hdr.{payload}.sig")).unwrap();
        assert_eq!(index(&decoded, "jti").unwrap(), serde_json::json!("one"));
        assert_eq!(
            index(&decoded, "aud").unwrap(),
            serde_json::json!("cabinpkg.com/verifier")
        );
        assert!(claims("no-dots").is_err());
        assert!(claims("a.!not-base64url!.c").is_err());
        assert!(claims("a..c").is_err());
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
