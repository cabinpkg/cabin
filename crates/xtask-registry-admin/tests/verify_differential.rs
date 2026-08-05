//! The whole-run differential for `cargo registry-verify`: the shell it
//! replaces and the port, run against one mock registry, compared on
//! stdout, stderr, exit status, the requests each made, and the bytes
//! each handed the verifier child.
//!
//! `tests/fixtures/registry-verify.sh.orig` is the original, byte for
//! byte: the `run:` block of the "Verify pending versions" step of
//! `.github/workflows/registry-verify.yml` as it stood on `main`,
//! dedented 10 spaces, `sha256`
//! `0085194b5f60003ce38438e820f9fbeba04c4af5532db8b0df521ce5ab0b6eca`.
//! Nothing is prepended and nothing is edited - this suite *runs* that
//! file, so even a header comment would be a change to the thing under
//! test. The provenance lives here instead.
//!
//! # Reaching a local mock through the `https` guard
//!
//! The script refuses a `REGISTRY_ORIGIN` or `EXPECTED_API_ORIGIN` that
//! does not start with `https://`, and the admin origin it then uses is
//! the one `config.json` declares, checked for equality against
//! `EXPECTED_API_ORIGIN`. Every request therefore goes to an `https`
//! origin, and neither client can be talked out of real TLS. So the
//! harness terminates TLS in front of the mock: `openssl` mints a
//! throwaway CA and a leaf for `IP:127.0.0.1`, and a ~30-line `python3`
//! forwarder accepts TLS and pipes the plaintext to the `tiny_http`
//! mock. Both sides then trust that CA through the environment alone -
//! `curl` reads `CURL_CA_BUNDLE`, and `ureq`'s `rustls-native-certs`
//! path reads `SSL_CERT_FILE` before it consults the platform store.
//!
//! The `native-certs` feature that makes the second half work is a
//! *dev-dependency* feature. Cargo unifies it into the binary only when
//! tests are being built, so `cargo build --release` still compiles the
//! shipped `xtask-registry-admin` against `webpki-roots` exactly as
//! before, and `SSL_CERT_FILE` does not reach it. The suite therefore
//! buys its reachability without adding a test-only knob to
//! `src/verify.rs`, which is the trade the port plan asked for.
//!
//! # Two origins, not one
//!
//! The index origin serves `config.json` and the artifacts; the admin
//! API lives on the origin `config.json` declares. A scenario that
//! pointed both at one listener would pass a port that routed the
//! listing to the index origin or the artifact to the admin one, so
//! the full verified path runs two listeners behind the same CA and
//! the request log records which of them answered. The guard and abort
//! scenarios stay single-origin: they abort before the split matters.
//!
//! # What is compared, and where the comparison stops
//!
//! stdout, the exit status, the request log and the verifier's inputs
//! are compared byte for byte in every live scenario but the
//! multi-version one, where the shuffle means stdout and the request
//! log compare as sorted multisets of whole lines - same lines, same
//! count, same framing, any order. stderr is
//! compared byte for byte only where the script itself is the sole
//! writer. Where `curl` also wrote to stderr - a failed archive fetch,
//! a failed `PATCH`, a failed upstream fetch - `curl`'s own wording is
//! not reproducible, so the assertion narrows to "both sides emitted
//! this exact line", which is the half the runbook quotes. Where the
//! script aborted through a bare `jq` substitution, stderr carried
//! `jq`'s error text and the port prints its own one-line diagnostic
//! instead (the port plan's ceiling 20), so those cases assert abort
//! *semantics* - that stdout stopped where the shell's stopped, that no
//! further request was made - and that the port said something rather
//! than aborting silently.
//!
//! The exit status is compared exactly everywhere the script chose it,
//! which is every 0 and every 1 it writes. It is not compared on the
//! abort paths, where `set -e` propagated whichever tool failed - 22
//! from `curl` on an HTTP error, 5 from a `jq` program error - and the
//! port collapses all of them to 1 (the port plan's ceiling 4). There
//! the assertion is that the shell refused at all and that the port
//! refused with the single status it promises.
//!
//! # Not covered here, and why
//!
//! - **Budget exhaustion.** `upstream_budget` starts at 300 seconds and
//!   only a real transfer spends it, so driving it to zero costs at
//!   least 300 seconds of wall clock however the mock is arranged, and
//!   the script has no knob to lower it. The arithmetic - the `< 120`
//!   cap, the one-second floor, the `<= 0` test - is unit-tested over a
//!   fake clock beside the port instead.
//! - **The 256 MiB upstream size cap.** `tiny_http` answers a reader it
//!   cannot measure with `Transfer-Encoding: chunked` and drops a
//!   `content-length` header supplied by hand, so the mock cannot
//!   overstate a length, and the only other way to reach the cap is to
//!   move a real 256 MiB through it twice. Both halves of the cap - the
//!   `content-length` pre-check and the counted read that aborts
//!   mid-transfer - are unit-tested beside the port. What this suite
//!   does pin is the branch they land on: a failed upstream fetch
//!   verifies without the file and never rejects.
//! - **An unset variable, as opposed to an empty one.** Under `set -u`
//!   an unset `REGISTRY_VERIFY_TOKEN` is a bash diagnostic naming the
//!   script's own path and line, not the script's message, so it cannot
//!   be matched by anything that is not bash. The workflow always sets
//!   all three variables (an absent repository variable arrives as an
//!   empty string), so empty is the reachable case and the one the
//!   scenarios use.
//! - **Number rendering beyond what the registry emits.** `jq` prints
//!   numbers it did not compute with using their original text -
//!   `1e3` stays `1E+3`, `1.0` stays `1.0`, integers past 2^64 survive
//!   intact - which no `serde_json` without `arbitrary_precision` can
//!   reproduce. The admin listing only ever carries small non-negative
//!   integers, so the scenarios pin those and the divergence beyond
//!   them is a stated ceiling rather than a test that would have to
//!   fail.
//!
//! The suite is Unix-only outright. The original is a bash script and
//! its tools are matched by name; a Windows host's lookalikes (Git
//! Bash, a `jq` built for another shell's quoting) EXIST on `PATH` and
//! would pass a presence check while meaning something else. Every test
//! skips rather than fails when a tool it needs is missing.
#![cfg(unix)]

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader, Cursor};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use assert_fs::TempDir;

/// The token every live scenario sends. Distinctive on purpose: the
/// request oracle compares the `authorization` header's exact value, so
/// a side that hard-coded a header would not match it.
const TOKEN: &str = "differential-verify-token";

/// Proves the child inherits the *whole* environment, not a curated
/// subset: the script handed its child everything it had, and the cap
/// is one the real verifier reads (`limits_from_env`).
const SENTINEL: &str = "reaches-the-child";
const CAP: &str = "4096";

/// The tools every live scenario drives, on top of the port itself.
const LIVE_TOOLS: [&str; 6] = ["bash", "jq", "curl", "shuf", "openssl", "python3"];

/// Terminates TLS in front of the mock. It knows nothing about the
/// registry: it accepts, wraps, and pipes bytes both ways, so every
/// routing decision stays in Rust where the scenarios are written.
const FORWARDER: &str = r#"import socket, ssl, sys, threading

chain, key, backend = sys.argv[1], sys.argv[2], int(sys.argv[3])
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(chain, key)
listener = socket.create_server(("127.0.0.1", 0))
print(listener.getsockname()[1], flush=True)


def pipe(src, dst):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            dst.sendall(data)
    except OSError:
        pass
    try:
        dst.shutdown(socket.SHUT_WR)
    except OSError:
        pass


def serve(raw):
    try:
        front = context.wrap_socket(raw, server_side=True)
    except OSError:
        raw.close()
        return
    try:
        back = socket.create_connection(("127.0.0.1", backend))
    except OSError:
        front.close()
        return
    upward = threading.Thread(target=pipe, args=(front, back), daemon=True)
    upward.start()
    pipe(back, front)
    upward.join()
    front.close()
    back.close()


while True:
    conn, _ = listener.accept()
    threading.Thread(target=serve, args=(conn,), daemon=True).start()
"#;

/// Stands in for `target/release/cabin-registry-verify`. It records
/// what it was handed - the entry bytes, the corpus bytes, and its own
/// argument shape with the random `mktemp` directories stripped - and
/// then replays whatever the scenario scripted. Recordings are keyed by
/// package name, so a multi-version scenario compares as a set and does
/// not depend on the shuffle, and by an invocation number, so a
/// repeated call records beside the first rather than over it. The key
/// doubles `-` before mapping `/` onto it, which keeps `a-b/c` and
/// `a/b-c` apart where a plain `tr` would collide them.
const VERIFIER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
rec=$root/rec
script=$root/script

# The child inherits the whole environment, the privileged token
# included.  Recording it from inside the child is the only way to
# compare what each side actually handed down.
# Per package and phase, so the sequence does not depend on the
# shuffle: a second call for the same pair records -2, it does not
# overwrite -1.
nth() {
  counter=$rec/.n-$1
  n=$(( $(cat "$counter" 2>/dev/null || echo 0) + 1 ))
  printf '%s' "$n" >"$counter"
  printf '%s' "$n"
}

record_env() {
  printf '%s\n' "sentinel=${CABIN_VERIFY_DIFFERENTIAL-<unset>}" \
    "token=${REGISTRY_VERIFY_TOKEN-<unset>}" \
    "cap=${VERIFY_MAX_ENTRIES-<unset>}" >"$rec/env-$1"
}

if [ "$1" = --name-advisories ]; then
  entry=$2
  stem=$(jq -r '.name' <"$entry" | sed 's/-/--/g; s|/|-|')
  seq=$(nth "advice-$stem")
  cp "$entry" "$rec/advice-entry-$stem-$seq.json"
  cp "$3" "$rec/corpus"
  printf '%s\n' "--name-advisories" "${2##*/}" "<corpus>" >"$rec/advice-argv-$stem-$seq"
  record_env "advice-$stem-$seq"
  cat "$script/advice.out"
  exit "$(cat "$script/advice.rc")"
fi

archive=$1
entry=$2
stem=$(jq -r '.name' <"$entry" | sed 's/-/--/g; s|/|-|')
seq=$(nth "inspect-$stem")
cp "$entry" "$rec/inspect-entry-$stem-$seq.json"
cp "$archive" "$rec/archive-$stem-$seq"
previous=""
for arg in "$@"; do
  if [ "$previous" = --upstream ]; then cp "$arg" "$rec/upstream-$stem-$seq"; fi
  previous=$arg
done
for arg in "$@"; do printf '%s\n' "${arg##*/}"; done >"$rec/inspect-argv-$stem-$seq"
record_env "inspect-$stem-$seq"
cat "$script/inspect.err" >&2
cat "$script/inspect.out"
exit "$(cat "$script/inspect.rc")"
"#;

/// What the stub verifier replays. One script per scenario: no
/// scenario here needs a per-package answer, and keying by package
/// would let a scenario depend on the shuffle without saying so.
struct Verifier {
    advice: &'static str,
    advice_status: i32,
    inspect: &'static str,
    inspect_status: i32,
    inspect_stderr: &'static [u8],
}

impl Default for Verifier {
    fn default() -> Self {
        Self {
            advice: "{\"advice\":\"proceed\"}",
            advice_status: 0,
            inspect: "{\"verdict\":\"verified\"}",
            inspect_status: 0,
            inspect_stderr: b"",
        }
    }
}

/// One request the mock answered, rendered into the log both sides are
/// compared on. The authorization header is recorded verbatim: the
/// split between the token-bearing calls and the publisher-controlled
/// upstream fetch matters, and so does the value, since a port that
/// sent some other credential would still make the same split.
struct Recorded {
    /// Which listener answered. The index origin serves `config.json`
    /// and the artifacts; the admin API lives on the origin
    /// `config.json` declares. A port that routed either to the other
    /// would still answer correctly, so the role is part of the log.
    role: &'static str,
    method: String,
    path: String,
    authorization: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Recorded {
    fn line(&self) -> String {
        let mut line = format!(
            "[{}] {} {} authorization={}",
            self.role,
            self.method,
            self.path,
            self.authorization.as_deref().unwrap_or("<absent>")
        );
        if let Some(kind) = &self.content_type {
            let _ = write!(line, " content-type={kind}");
        }
        if !self.body.is_empty() {
            let _ = write!(line, " body={}", String::from_utf8_lossy(&self.body));
        }
        line
    }
}

/// What the mock sends back.
struct Reply {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
}

impl Reply {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
        }
    }

    fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            body,
            delay: Duration::ZERO,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            delay: Duration::ZERO,
        }
    }
}

/// The throwaway CA, the leaf it signs, and the forwarder source. Built
/// once per process, and only for a scenario that asks for a mock, so
/// the guard scenarios need nothing but bash. A refusal here is the
/// harness failing rather than a tool missing, so [`tls`] panics on it.
struct Tls {
    _dir: TempDir,
    authority: PathBuf,
    chain: PathBuf,
    key: PathBuf,
    forwarder: PathBuf,
}

static TLS: OnceLock<Option<Tls>> = OnceLock::new();

/// Panics rather than skipping. A missing `openssl` is a tool check
/// [`ready`] already made; getting here and failing means the harness
/// is broken, and a broken harness that skipped would leave the suite
/// green while testing nothing.
fn tls() -> &'static Tls {
    TLS.get_or_init(mint_tls)
        .as_ref()
        .expect("the harness could not mint its throwaway CA")
}

/// `rustls` rejects a self-signed certificate presented as a leaf
/// (`CaUsedAsEndEntity`), so this mints a CA and a separate leaf that
/// carries `IP:127.0.0.1` as its subject alternative name.
fn mint_tls() -> Option<Tls> {
    let dir = TempDir::new().ok()?;
    let at = |name: &str| dir.path().join(name);
    let (authority, authority_key) = (at("ca.pem"), at("ca.key"));
    let (leaf, leaf_key, request) = (at("leaf.pem"), at("leaf.key"), at("leaf.csr"));
    let (extensions, chain) = (at("leaf.ext"), at("chain.pem"));

    fs::write(
        &extensions,
        "basicConstraints=critical,CA:FALSE\n\
         keyUsage=critical,digitalSignature,keyEncipherment\n\
         extendedKeyUsage=serverAuth\n\
         subjectAltName=IP:127.0.0.1\n",
    )
    .ok()?;

    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        &show(&authority_key),
        "-out",
        &show(&authority),
        "-days",
        "2",
        "-subj",
        "/CN=cabin-verify-differential",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign",
    ])?;
    openssl(&[
        "req",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        &show(&leaf_key),
        "-out",
        &show(&request),
        "-subj",
        "/CN=127.0.0.1",
    ])?;
    openssl(&[
        "x509",
        "-req",
        "-in",
        &show(&request),
        "-CA",
        &show(&authority),
        "-CAkey",
        &show(&authority_key),
        "-CAcreateserial",
        "-out",
        &show(&leaf),
        "-days",
        "2",
        "-extfile",
        &show(&extensions),
    ])?;

    let mut bundle = fs::read(&leaf).ok()?;
    bundle.extend_from_slice(&fs::read(&authority).ok()?);
    fs::write(&chain, bundle).ok()?;

    let forwarder = at("forwarder.py");
    fs::write(&forwarder, FORWARDER).ok()?;

    Some(Tls {
        _dir: dir,
        authority,
        chain,
        key: leaf_key,
        forwarder,
    })
}

fn openssl(args: &[&str]) -> Option<()> {
    let done = Command::new("openssl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    done.success().then_some(())
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The mock registry: a `tiny_http` server on loopback with the TLS
/// forwarder in front of it, plus the log both sides are compared on.
struct Mock {
    role: &'static str,
    origin: String,
    log: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    forwarder: Child,
    server: Option<tiny_http::Server>,
}

impl Mock {
    /// Binds and starts the forwarder without serving anything yet, so
    /// a scenario can build its handler around the origin it is about
    /// to be reached at.
    fn start() -> Self {
        Self::bind("origin", Arc::new(Mutex::new(Vec::new())))
    }

    /// The index origin and the admin origin as two separate listeners
    /// sharing one log, so the log says which of them answered.
    fn pair() -> (Self, Self) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self::bind("registry", Arc::clone(&log)),
            Self::bind("api", log),
        )
    }

    /// Every failure here is the harness failing, not a tool missing,
    /// so every one of them panics.
    fn bind(role: &'static str, log: Arc<Mutex<Vec<Recorded>>>) -> Self {
        let tls = tls();
        let server = tiny_http::Server::http("127.0.0.1:0").expect("the mock could not bind");
        let plaintext = server
            .server_addr()
            .to_ip()
            .expect("the mock bound something that is not an ip")
            .port();

        let mut forwarder = Command::new("python3")
            .arg(&tls.forwarder)
            .arg(&tls.chain)
            .arg(&tls.key)
            .arg(plaintext.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the tls forwarder could not start");
        let mut announced = String::new();
        BufReader::new(forwarder.stdout.take().expect("the forwarder's stdout"))
            .read_line(&mut announced)
            .expect("the forwarder did not announce a port");
        let port: u16 = announced
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("the forwarder announced {announced:?}, not a port"));

        Self {
            role,
            origin: format!("https://127.0.0.1:{port}"),
            log,
            stop: Arc::new(AtomicBool::new(false)),
            forwarder,
            server: Some(server),
        }
    }

    fn serve(&mut self, handler: impl Fn(&Recorded) -> Reply + Send + 'static) {
        let server = self.server.take().expect("the mock serves once");
        let role = self.role;
        let log = Arc::clone(&self.log);
        let stop = Arc::clone(&self.stop);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body).ok();
                let headers = request.headers();
                let recorded = Recorded {
                    role,
                    method: request.method().as_str().to_owned(),
                    path: request.url().to_owned(),
                    authorization: headers
                        .iter()
                        .find(|h| h.field.equiv("authorization"))
                        .map(|h| h.value.as_str().to_owned()),
                    content_type: headers
                        .iter()
                        .find(|h| h.field.equiv("content-type"))
                        .map(|h| h.value.as_str().to_owned()),
                    body,
                };
                let reply = handler(&recorded);
                log.lock().expect("the log is not poisoned").push(recorded);
                if !reply.delay.is_zero() {
                    thread::sleep(reply.delay);
                }
                let length = Some(reply.body.len());
                request
                    .respond(tiny_http::Response::new(
                        tiny_http::StatusCode(reply.status),
                        Vec::new(),
                        Cursor::new(reply.body),
                        length,
                        None,
                    ))
                    .ok();
            }
        });
    }

    /// Renders and clears the log, so the same mock can serve both
    /// sides and be compared run against run.
    fn drain(&self) -> (Vec<String>, Vec<Vec<u8>>) {
        let mut log = self.log.lock().expect("the log is not poisoned");
        let lines = log.iter().map(Recorded::line).collect();
        let bodies = log.iter().map(|request| request.body.clone()).collect();
        log.clear();
        (lines, bodies)
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.forwarder.kill().ok();
        self.forwarder.wait().ok();
    }
}

/// Everything one side of a scenario produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    requests: Vec<String>,
    /// The request bodies raw, beside the rendered log. A body folded
    /// into the log line would compare through a lossy rendering.
    bodies: Vec<Vec<u8>>,
    handed_over: Vec<(String, Vec<u8>)>,
}

impl Outcome {
    /// The streams as text, for assertions about lines the script
    /// wrote. The comparison between sides is on the bytes.
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// How far stderr can be compared.
enum Diagnostics<'a> {
    /// The script was the only writer: compare it byte for byte.
    Exact,
    /// `curl` also wrote. Assert both sides emitted each of these as a
    /// whole line and leave the rest to the ceiling.
    Lines(&'a [&'a str]),
    /// A bare `jq` substitution aborted the run and stderr carried
    /// `jq`'s text. Assert only that the port diagnosed something.
    Aborted,
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/registry-verify.sh.orig")
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

/// `seq 0 -1` prints nothing under GNU coreutils and counts DOWN (`0`
/// then `-1`) under BSD. A count that is not an integer reaches exactly
/// that call, so the same listing walks nothing on the workflow's
/// runner and walks `.versions[-1]` - the LAST entry - on a Mac. The
/// divergence is the vendor's, not the port's, so the scenarios that
/// depend on it run only where `seq` is the one the workflow has.
fn gnu_seq() -> bool {
    Command::new("seq")
        .args(["0", "-1"])
        .output()
        .is_ok_and(|out| out.stdout.is_empty())
}

fn ready(test: &str, tools: &[&str]) -> bool {
    for tool in tools {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}

/// Lays out one side's working directory: the stub verifier where the
/// script looks for the real one, the script it replays, and the
/// recording directory it writes into.
fn workspace(verifier: &Verifier) -> TempDir {
    let dir = TempDir::new().expect("a scratch directory");
    let release = dir.path().join("target/release");
    fs::create_dir_all(&release).expect("the verifier's directory");
    fs::create_dir_all(dir.path().join("rec")).expect("the recording directory");
    let script = dir.path().join("script");
    fs::create_dir_all(&script).expect("the script directory");

    let stub = release.join("cabin-registry-verify");
    fs::write(&stub, VERIFIER).expect("the stub verifier");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("the stub is executable");

    for (name, body) in [
        ("advice.out", verifier.advice.as_bytes().to_vec()),
        ("advice.rc", verifier.advice_status.to_string().into_bytes()),
        ("inspect.out", verifier.inspect.as_bytes().to_vec()),
        (
            "inspect.rc",
            verifier.inspect_status.to_string().into_bytes(),
        ),
        ("inspect.err", verifier.inspect_stderr.to_vec()),
    ] {
        fs::write(script.join(name), body).expect("the scripted answer");
    }
    dir
}

/// What the stub verifier recorded, sorted so a multi-version scenario
/// does not depend on the shuffle.
fn handed_over(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut seen = Vec::new();
    let Ok(entries) = fs::read_dir(dir.join("rec")) else {
        return seen;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // The stub's own counters, not something the child was handed.
        if name.starts_with(".n-") {
            continue;
        }
        seen.push((name, fs::read(entry.path()).unwrap_or_default()));
    }
    seen.sort();
    seen
}

fn finish(output: &Output, dir: &Path, log: (Vec<String>, Vec<Vec<u8>>)) -> Outcome {
    Outcome {
        stdout: output.stdout.clone(),
        stderr: output.stderr.clone(),
        status: output.status.code(),
        requests: log.0,
        bodies: log.1,
        handed_over: handed_over(dir),
    }
}

/// Runs both sides of one scenario. The mock, when there is one, serves
/// them in turn and its log is taken between the two, so both see the
/// same origin and the same scripted answers.
fn both(
    mock: Option<&Mock>,
    verifier: &Verifier,
    token: &str,
    origin: &str,
    api: &str,
) -> (Outcome, Outcome) {
    let authority = mock.map_or_else(String::new, |_| show(&tls().authority));
    let environment = [
        ("REGISTRY_VERIFY_TOKEN", token),
        ("REGISTRY_ORIGIN", origin),
        ("EXPECTED_API_ORIGIN", api),
        ("CURL_CA_BUNDLE", &authority),
        ("SSL_CERT_FILE", &authority),
        ("CABIN_VERIFY_DIFFERENTIAL", SENTINEL),
        ("VERIFY_MAX_ENTRIES", CAP),
    ];

    let shell_dir = workspace(verifier);
    let mut bash = Command::new("bash");
    bash.arg(fixture());
    let bashed = run(bash, shell_dir.path(), &environment);
    let shell = finish(
        &bashed,
        shell_dir.path(),
        mock.map_or_else(Default::default, Mock::drain),
    );

    let port_dir = workspace(verifier);
    let mut ported = Command::new(env!("CARGO_BIN_EXE_xtask-registry-admin"));
    ported.arg("verify");
    let ran = run(ported, port_dir.path(), &environment);
    let port = finish(
        &ran,
        port_dir.path(),
        mock.map_or_else(Default::default, Mock::drain),
    );

    ran_against(mock, &shell);
    ran_against(mock, &port);
    (shell, port)
}

/// A mock that answered nothing means a dead listener or forwarder,
/// which would otherwise let two identically-failed runs compare equal.
fn ran_against(mock: Option<&Mock>, outcome: &Outcome) {
    assert!(
        mock.is_none() || !outcome.requests.is_empty(),
        "the mock answered nothing: the harness is broken, not the port"
    );
}

fn run(mut command: Command, dir: &Path, environment: &[(&str, &str)]) -> Output {
    command.current_dir(dir);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().expect("running one side of the scenario")
}

fn diff(case: &str, shell: &Outcome, port: &Outcome, diagnostics: &Diagnostics) {
    assert!(
        shell.stdout == port.stdout,
        "{case}: stdout\nshell: {}\nport:  {}",
        shell.stdout.escape_ascii(),
        port.stdout.escape_ascii()
    );
    assert_eq!(shell.requests, port.requests, "{case}: requests");
    assert!(
        shell.bodies == port.bodies,
        "{case}: request bodies differ below the rendering"
    );
    // Compared as bytes: any rendering that collapses invalid UTF-8
    // would equate two different archives. Rendering is for the
    // failure message only.
    assert!(
        shell.handed_over == port.handed_over,
        "{case}: what the verifier was handed\nshell:\n{}\nport:\n{}",
        rendered(&shell.handed_over),
        rendered(&port.handed_over)
    );
    match diagnostics {
        Diagnostics::Exact => {
            assert!(
                shell.stderr == port.stderr,
                "{case}: stderr\nshell: {}\nport:  {}",
                shell.stderr.escape_ascii(),
                port.stderr.escape_ascii()
            );
            assert_eq!(shell.status, port.status, "{case}: exit status");
        }
        Diagnostics::Lines(lines) => {
            for line in *lines {
                for (side, text) in [("shell", &shell.err()), ("port", &port.err())] {
                    assert!(
                        text.lines().any(|emitted| emitted == *line),
                        "{case}: {side} stderr is missing `{line}`, got:\n{text}"
                    );
                }
            }
            assert_eq!(shell.status, port.status, "{case}: exit status");
        }
        Diagnostics::Aborted => {
            assert!(
                !port.err().is_empty(),
                "{case}: the port aborted without saying why"
            );
            // The exit-code ceiling: `set -e` propagated whatever tool
            // failed - 22 from `curl`'s HTTP error, 5 from a `jq`
            // program error - and the port collapses all of them to 1.
            // Nothing reads the distinction, so the assertion is that
            // both refused, and that the port refused with the one
            // status it promises.
            assert!(
                shell.status.is_none_or(|status| status != 0),
                "{case}: the shell did not abort"
            );
            assert_eq!(port.status, Some(1), "{case}: the port's abort status");
        }
    }
}

/// The one recording whose name starts with `prefix`, as bytes.
fn handed<'a>(recordings: &'a [(String, Vec<u8>)], prefix: &str) -> &'a [u8] {
    let found = recordings.iter().find(|(name, _)| name.starts_with(prefix));
    let (_, bytes) = found.unwrap_or_else(|| panic!("no recording named {prefix}*"));
    bytes
}

fn rendered(recordings: &[(String, Vec<u8>)]) -> String {
    recordings
        .iter()
        .map(|(name, bytes)| format!("  {name}: {}", bytes.escape_ascii()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A listing entry, shaped like `AdminVersionRecord` (`glue.rs`):
/// `revision` is a string and `published_by` the one integer, which is
/// small, so `jq` and `serde_json` render it alike.
fn entry(name: &str, version: &str, metadata: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"version\":\"{version}\",\"revision\":\"1\",\
         \"checksum\":\"{}\",\"published_by\":7,\
         \"published_at\":\"2026-08-04T00:00:00Z\",\"metadata\":{metadata}}}",
        "a".repeat(64)
    )
}

fn listing(entries: &[String]) -> String {
    format!("{{\"versions\":[{}]}}", entries.join(","))
}

/// The default route table: `config.json` naming the mock as the admin
/// origin, an empty corpus, and an archive for anything under
/// `/artifacts/`.
fn routes(origin: &str, pending: String) -> impl Fn(&Recorded) -> Reply + Send + 'static {
    let config = format!("{{\"api\":\"{origin}\"}}");
    move |request: &Recorded| match request.path.as_str() {
        "/config.json" => Reply::ok(&config),
        "/api/v1/admin/versions?status=pending" => Reply::ok(&pending),
        "/api/v1/admin/packages" => Reply::ok("{\"packages\":[]}"),
        path if path.starts_with("/artifacts/") => Reply::ok("archive bytes"),
        path if path.starts_with("/api/v1/admin/versions/") => Reply::status(204),
        _ => Reply::status(404),
    }
}

/// The guards run before anything is fetched, so this scenario needs no
/// server at all - which also makes it the one test that runs on a host
/// with neither `openssl` nor `python3`.
#[test]
fn the_guards_refuse_the_same_inputs() {
    if !ready("the_guards_refuse_the_same_inputs", &["bash"]) {
        return;
    }
    let verifier = Verifier::default();

    let (shell, port) = both(None, &verifier, "", "https://x", "https://x");
    diff("empty token", &shell, &port, &Diagnostics::Exact);
    assert_eq!(shell.err(), "REGISTRY_VERIFY_TOKEN is not configured\n");
    assert_eq!(shell.status, Some(1));

    // The empty tail is the reachable shape: an unset repository
    // variable arrives as an empty string, and the message renders it.
    for origin in [
        "",
        "http://x",
        "HTTPS://x",
        "https:/x",
        "ftp://x",
        " https://x",
    ] {
        let (shell, port) = both(None, &verifier, "t", origin, "https://x");
        diff(
            &format!("index origin `{origin}`"),
            &shell,
            &port,
            &Diagnostics::Exact,
        );
        assert_eq!(
            shell.err(),
            format!("REGISTRY_ORIGIN must be https, got: {origin}\n")
        );

        let (shell, port) = both(None, &verifier, "t", "https://x", origin);
        diff(
            &format!("api origin `{origin}`"),
            &shell,
            &port,
            &Diagnostics::Exact,
        );
        assert_eq!(
            shell.err(),
            format!("EXPECTED_API_ORIGIN must be https, got: {origin}\n")
        );
    }
}

/// An empty list and a list with no `versions` key are the same thing:
/// `jq`'s `length` of a missing path is 0, so a listing shaped like
/// `{}` is "nothing pending" and a clean exit, not a malformed answer.
#[test]
fn nothing_pending_ends_the_run_cleanly() {
    if !ready("nothing_pending_ends_the_run_cleanly", &LIVE_TOOLS) {
        return;
    }
    for pending in ["{\"versions\":[]}", "{}", "{\"versions\":null}"] {
        let mut mock = Mock::start();
        let origin = mock.origin.clone();
        mock.serve(routes(&origin, pending.to_owned()));

        let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
        diff(
            &format!("listing `{pending}`"),
            &shell,
            &port,
            &Diagnostics::Exact,
        );
        assert_eq!(shell.out(), "nothing pending\n");
        assert_eq!(shell.status, Some(0));
        assert_eq!(shell.requests.len(), 2, "the corpus is not fetched");
    }
}

/// The admin origin is whatever `config.json` declares, and it must
/// equal the pinned one. A mismatch is a diagnosed abort; a missing
/// `api` is the silent one, because `jq -er` writes `null` to the
/// captured stdout and exits 1 with nothing on stderr.
#[test]
fn the_pinned_api_origin_is_enforced_the_same_way() {
    if !ready(
        "the_pinned_api_origin_is_enforced_the_same_way",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let served = Arc::new(Mutex::new(String::new()));
    let answer = Arc::clone(&served);
    mock.serve(move |request: &Recorded| {
        if request.path != "/config.json" {
            return Reply::status(404);
        }
        let body = answer.lock().expect("the answer is not poisoned").clone();
        Reply::ok(&body)
    });

    let cases = [
        (
            "a mismatched api",
            "{\"api\":\"https://elsewhere.example\"}",
            Diagnostics::Exact,
            false,
        ),
        ("no api at all", "{}", Diagnostics::Aborted, true),
        ("a null api", "{\"api\":null}", Diagnostics::Aborted, true),
        (
            "a config that is not json",
            "not json",
            Diagnostics::Aborted,
            false,
        ),
    ];
    for (case, config, diagnostics, silent) in cases {
        {
            let mut answer = served.lock().expect("the answer is not poisoned");
            answer.clear();
            answer.push_str(config);
        }
        let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
        diff(case, &shell, &port, &diagnostics);
        assert_eq!(shell.out(), "", "{case}: nothing reaches stdout");
        assert_eq!(
            shell.requests.len(),
            1,
            "{case}: the listing is not fetched"
        );
        // `jq -er` writes `null` to the captured stdout and exits 1
        // with nothing on stderr, so the script aborts saying nothing
        // at all. The port is allowed to say why - that is ceiling 20 -
        // but it must not go on.
        assert_eq!(
            shell.err().is_empty(),
            silent,
            "{case}: the script's own stderr changed shape"
        );
    }
}

/// A verified verdict is the full path: the entry bytes reach the
/// child, the `PATCH` goes to a three-segment path built by
/// concatenation - the scoped `/` and the version's `+` unencoded -
/// and the body carries its four keys in `jq`'s order.
#[test]
fn a_verified_version_patches_the_same_bytes() {
    if !ready("a_verified_version_patches_the_same_bytes", &LIVE_TOOLS) {
        return;
    }
    let (mut registry, mut api) = Mock::pair();
    let (index, admin) = (registry.origin.clone(), api.origin.clone());
    let pending = listing(&[entry("scope/pkg", "10.2.1+cabin.1", "{}")]);
    let config = format!("{{\"api\":\"{admin}\"}}");
    // Split strictly: neither listener answers for the other's role, so
    // a misrouted request 404s instead of quietly succeeding.
    registry.serve(move |request: &Recorded| match request.path.as_str() {
        "/config.json" => Reply::ok(&config),
        path if path.starts_with("/artifacts/") => Reply::ok("archive bytes"),
        _ => Reply::status(404),
    });
    api.serve(move |request: &Recorded| match request.path.as_str() {
        "/api/v1/admin/versions?status=pending" => Reply::ok(&pending),
        "/api/v1/admin/packages" => Reply::ok("{\"packages\":[]}"),
        path if path.starts_with("/api/v1/admin/versions/") => Reply::status(204),
        _ => Reply::status(404),
    });

    let (shell, port) = both(Some(&registry), &Verifier::default(), TOKEN, &index, &admin);
    diff("a verified version", &shell, &port, &Diagnostics::Exact);

    // Which origin served what, in order. config.json and the artifact
    // come off the index origin; the listing, the corpus and the
    // verdict off the one config.json declared.
    let roles: Vec<&str> = shell
        .requests
        .iter()
        .map(|line| line.split(' ').next().expect("a role prefix"))
        .collect();
    assert_eq!(
        roles,
        ["[registry]", "[api]", "[api]", "[registry]", "[api]"],
        "a request went to the wrong origin: {:?}",
        shell.requests
    );

    assert_eq!(
        shell.out(),
        "1 pending version(s)\nscope/pkg@10.2.1+cabin.1: verified\n"
    );
    assert_eq!(shell.status, Some(0));
    let patch = shell
        .requests
        .iter()
        .find(|line| line.contains("] PATCH "))
        .expect("a verdict was sent");
    assert!(
        patch.starts_with(
            "[api] PATCH /api/v1/admin/versions/scope/pkg/10.2.1+cabin.1 \
             authorization=Bearer differential-verify-token \
             content-type=application/json body={\"verdict\":\"verified\","
        ),
        "the verdict went to {patch}"
    );
    assert!(
        shell.requests.iter().any(|line| line.starts_with(
            "[registry] GET /artifacts/scope/pkg/scope-pkg-10.2.1+cabin.1-1.zip \
             authorization=Bearer differential-verify-token"
        )),
        "the archive url flattens the scope and ends in the revision: {:?}",
        shell.requests
    );

    // Every call on the token plane carries the header verbatim. `diff`
    // has already established the two sides recorded the same values;
    // this says which value that has to be.
    assert_eq!(shell.requests.len(), 5, "{:?}", shell.requests);
    for line in &shell.requests {
        assert!(
            line.contains(&format!(" authorization=Bearer {TOKEN}")),
            "a token-plane request lost the header: {line}"
        );
    }

    // L84/L190 spawn the child with the environment as it stands, the
    // privileged token included.  Recorded from inside the child, so a
    // port that curated the environment would differ here.
    let inherited = String::from_utf8_lossy(handed(&shell.handed_over, "env-inspect"));
    assert_eq!(
        inherited,
        format!("sentinel={SENTINEL}\ntoken={TOKEN}\ncap={CAP}\n"),
        "the child was handed a different environment"
    );

    // The archive bytes the child was handed are the bytes the mock
    // served, not a re-encoding of them.
    let archive = String::from_utf8_lossy(handed(&shell.handed_over, "archive-"));
    assert_eq!(archive, "archive bytes");
}

/// A rejection joins its reason codes with commas and repeats them in
/// the log line and in the body - including a reason that itself
/// contains a comma, a quote, a backslash and a newline, which is where
/// a hand-built body would diverge from `jq -cn --arg`.
#[test]
fn a_rejected_version_joins_the_same_reasons() {
    if !ready("a_rejected_version_joins_the_same_reasons", &LIVE_TOOLS) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    mock.serve(routes(
        &origin,
        listing(&[entry("scope/pkg", "1.0.0", "{}")]),
    ));

    let verifier = Verifier {
        inspect: "{\"verdict\":\"rejected\",\
                  \"reasons\":[\"checksum-mismatch\",\"a,b\",\"q\\\"s\\\\b\\nl\"]}",
        ..Verifier::default()
    };
    let (shell, port) = both(Some(&mock), &verifier, TOKEN, &origin, &origin);
    diff("a rejected version", &shell, &port, &Diagnostics::Exact);
    assert!(
        shell
            .out()
            .contains("scope/pkg@1.0.0: rejected (checksum-mismatch,a,b,q\"s\\b\nl)"),
        "the runbook's recognition line changed: {:?}",
        shell.out()
    );
    assert_eq!(shell.status, Some(0), "a rejection is the verifier working");
}

/// Abstain renders no verdict and is deliberately not a failure: the
/// version stays pending, nothing is sent, and the run still exits 0.
#[test]
fn abstain_leaves_the_version_pending_without_failing() {
    if !ready(
        "abstain_leaves_the_version_pending_without_failing",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    mock.serve(routes(
        &origin,
        listing(&[entry("scope/pkg", "1.0.0", "{}")]),
    ));

    let verifier = Verifier {
        advice: "{\"advice\":\"abstain\",\"findings\":[\"confusable\",\"near-miss\"]}",
        ..Verifier::default()
    };
    let (shell, port) = both(Some(&mock), &verifier, TOKEN, &origin, &origin);
    diff("abstain", &shell, &port, &Diagnostics::Exact);
    assert_eq!(
        shell.out(),
        "1 pending version(s)\nscope/pkg@1.0.0: abstain (confusable,near-miss); \
         leaving it pending for operator review\n"
    );
    assert_eq!(shell.status, Some(0));
    assert!(
        !shell.requests.iter().any(|line| line.contains("] PATCH ")),
        "an abstained version is never sent a verdict"
    );
    assert!(
        !shell
            .requests
            .iter()
            .any(|line| line.contains("/artifacts/")),
        "advisories run before the download, so no bytes were fetched"
    );
}

/// The six failures that are counted rather than fatal: each leaves its
/// version pending, lets the rest of the list run, and fails the run at
/// the end. With one version in the list the count is the whole story.
#[test]
fn the_counted_failures_stay_counted() {
    if !ready("the_counted_failures_stay_counted", &LIVE_TOOLS) {
        return;
    }
    let failed = "1 version(s) hit operational failures and stay pending";
    let cases = [
        (
            "the advisories exit non-zero",
            Verifier {
                advice_status: 2,
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: name advisories failed operationally; leaving it pending",
            Diagnostics::Exact,
        ),
        (
            "an advice nobody knows",
            Verifier {
                advice: "{\"advice\":\"maybe\"}",
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: unknown advice 'maybe'; leaving it pending",
            Diagnostics::Exact,
        ),
        (
            "the verifier exits non-zero",
            Verifier {
                inspect_status: 2,
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: verifier failed operationally; leaving it pending",
            Diagnostics::Exact,
        ),
        (
            "a verdict nobody knows",
            Verifier {
                inspect: "{\"verdict\":\"maybe\"}",
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: unknown verdict 'maybe'; leaving it pending",
            Diagnostics::Exact,
        ),
        (
            "the verifier writes to stderr",
            Verifier {
                inspect_status: 2,
                // Invalid UTF-8 on purpose: the child's stderr is
                // passed through, not decoded, and a lossy comparison
                // would equate two different messages.
                inspect_stderr: b"the child said this \xff\xfe\n",
                ..Verifier::default()
            },
            "the child said this \u{fffd}\u{fffd}",
            Diagnostics::Exact,
        ),
        // A child that exits 0 having printed nothing. `jq` runs its
        // filter once per input document, so no input means no output
        // and a clean exit - the extraction yields the empty string and
        // lands in the unknown arm, a counted failure. A port that
        // parsed the child's stdout as one JSON document would abort
        // the whole run here instead: the same trap as a malformed
        // listing entry, reached through the child.
        (
            "the advisories say nothing at all",
            Verifier {
                advice: "",
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: unknown advice ''; leaving it pending",
            Diagnostics::Exact,
        ),
        (
            "the verifier says nothing at all",
            Verifier {
                inspect: "",
                ..Verifier::default()
            },
            "scope/pkg@1.0.0: unknown verdict ''; leaving it pending",
            Diagnostics::Exact,
        ),
    ];

    for (case, verifier, line, diagnostics) in cases {
        let mut mock = Mock::start();
        let origin = mock.origin.clone();
        mock.serve(routes(
            &origin,
            listing(&[entry("scope/pkg", "1.0.0", "{}")]),
        ));

        let (shell, port) = both(Some(&mock), &verifier, TOKEN, &origin, &origin);
        diff(case, &shell, &port, &diagnostics);
        assert_eq!(shell.status, Some(1), "{case}: the run fails at the end");
        assert!(shell.err().contains(line), "{case}: got {:?}", shell.err());
        assert!(shell.err().contains(failed), "{case}: the tally is missing");
        assert!(
            !shell.requests.iter().any(|l| l.contains("] PATCH ")),
            "{case}: no verdict is sent for a failed version"
        );
    }
}

/// The two counted failures `curl` also comments on. Only the script's
/// own line is compared; `curl`'s wording is the stated ceiling.
#[test]
fn a_failed_fetch_and_a_failed_patch_stay_counted() {
    if !ready(
        "a_failed_fetch_and_a_failed_patch_stay_counted",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let cases = [
        (
            "the archive is gone",
            "/artifacts/",
            "scope/pkg@1.0.0: archive download failed; leaving it pending",
        ),
        (
            "the row moved under us",
            "/api/v1/admin/versions/",
            "scope/pkg@1.0.0: verdict PATCH failed; leaving it pending",
        ),
    ];
    for (case, broken, line) in cases {
        let mut mock = Mock::start();
        let origin = mock.origin.clone();
        let table = routes(&origin, listing(&[entry("scope/pkg", "1.0.0", "{}")]));
        mock.serve(move |request: &Recorded| {
            if request.path.starts_with(broken) {
                // 409 for the verdict, 404 for the archive; both are
                // `curl -f` failures and neither carries a body.
                return Reply::status(if broken.contains("admin") { 409 } else { 404 });
            }
            table(request)
        });

        let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
        diff(case, &shell, &port, &Diagnostics::Lines(&[line]));
        assert_eq!(shell.status, Some(1), "{case}");
    }
}

/// The corpus is fetched once, before the loop, and a failure there
/// fails the whole run under `set -e` - after the count has already
/// been printed, which is what separates it from an early abort.
#[test]
fn a_missing_corpus_aborts_the_whole_run() {
    if !ready("a_missing_corpus_aborts_the_whole_run", &LIVE_TOOLS) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let table = routes(&origin, listing(&[entry("scope/pkg", "1.0.0", "{}")]));
    mock.serve(move |request: &Recorded| {
        if request.path == "/api/v1/admin/packages" {
            return Reply::status(500);
        }
        table(request)
    });

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff(
        "the corpus is unavailable",
        &shell,
        &port,
        &Diagnostics::Aborted,
    );
    assert_eq!(shell.out(), "1 pending version(s)\n");
    assert_ne!(shell.status, Some(0));
    assert!(
        !shell.err().contains("stay pending"),
        "an abort is not a counted failure"
    );
    assert_eq!(shell.requests.len(), 3, "the loop never started");
}

/// The port plan's sharpest trap: a field extraction the script wrote
/// as a bare substitution aborts the *entire* run when `jq` errors,
/// where the guarded commands count a failure and carry on. A struct-
/// shaped port turns the first into the second by default, and the two
/// differ in exit status, in the tally, and in how much of the list ran.
#[test]
fn a_malformed_entry_aborts_the_run_rather_than_counting_it() {
    if !ready(
        "a_malformed_entry_aborts_the_run_rather_than_counting_it",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    // `.metadata.upstream.url` cannot index a string: `jq` exits 5 and
    // `set -e` takes the run with it, mid-loop.
    let only = entry("scope/pkg", "1.0.0", "\"oops\"");
    mock.serve(routes(&origin, listing(&[only])));

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff("metadata is a string", &shell, &port, &Diagnostics::Aborted);
    assert_eq!(shell.out(), "1 pending version(s)\n");
    assert_ne!(shell.status, Some(0));
    assert!(
        !shell.err().contains("stay pending"),
        "the abort skipped the tally the counted path prints"
    );
    assert!(
        !shell.requests.iter().any(|line| line.contains("] PATCH ")),
        "nothing was sent after the abort"
    );
}

/// The upstream fetch is the one request that must not carry the token,
/// and the one URL the publisher controls. `url` absent, `null` or
/// `false` all mean "no upstream" - `jq`'s `//` takes `false` as absent
/// too, which an `Option<String>` port gets wrong by default.
#[test]
fn an_absent_upstream_is_absent_however_it_is_spelled() {
    if !ready(
        "an_absent_upstream_is_absent_however_it_is_spelled",
        &LIVE_TOOLS,
    ) {
        return;
    }
    for metadata in [
        "{}",
        "{\"upstream\":{}}",
        "{\"upstream\":{\"url\":null}}",
        "{\"upstream\":{\"url\":false}}",
        "{\"upstream\":{\"url\":\"\"}}",
    ] {
        let mut mock = Mock::start();
        let origin = mock.origin.clone();
        mock.serve(routes(
            &origin,
            listing(&[entry("scope/pkg", "1.0.0", metadata)]),
        ));

        let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
        diff(
            &format!("metadata {metadata}"),
            &shell,
            &port,
            &Diagnostics::Exact,
        );
        assert_eq!(shell.status, Some(0), "metadata {metadata}");
        let argv = String::from_utf8_lossy(handed(&shell.handed_over, "inspect-argv"));
        assert!(
            !argv.contains("--upstream"),
            "metadata {metadata}: the child was told about an upstream anyway"
        );
    }
}

/// A stored upstream URL that is not `https` is corrupt registry state,
/// never a verdict: it is a counted failure and no request is made.
#[test]
fn a_cleartext_upstream_url_is_a_counted_failure() {
    if !ready("a_cleartext_upstream_url_is_a_counted_failure", &LIVE_TOOLS) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let metadata = "{\"upstream\":{\"url\":\"http://upstream.invalid/a.tar.gz\"}}";
    mock.serve(routes(
        &origin,
        listing(&[entry("scope/pkg", "1.0.0", metadata)]),
    ));

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff("a cleartext upstream", &shell, &port, &Diagnostics::Exact);
    assert_eq!(
        shell.err(),
        "scope/pkg@1.0.0: stored upstream url is not https; leaving it pending\n\
         1 version(s) hit operational failures and stay pending\n"
    );
    assert_eq!(shell.status, Some(1));
    assert!(
        !shell.requests.iter().any(|line| line.contains("] PATCH ")),
        "no verdict is sent"
    );
}

/// A reachable upstream reaches the child as `--upstream`, and the
/// request that fetched it carried no authorization header. That split
/// is the most security-relevant line of the port.
#[test]
fn a_reachable_upstream_reaches_the_child_without_the_token() {
    if !ready(
        "a_reachable_upstream_reaches_the_child_without_the_token",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let metadata = format!("{{\"upstream\":{{\"url\":\"{origin}/upstream/a.tar.gz\"}}}}");
    let table = routes(&origin, listing(&[entry("scope/pkg", "1.0.0", &metadata)]));
    mock.serve(move |request: &Recorded| {
        if request.path == "/upstream/a.tar.gz" {
            return Reply::ok("upstream bytes");
        }
        table(request)
    });

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff("a reachable upstream", &shell, &port, &Diagnostics::Exact);
    assert_eq!(shell.status, Some(0));
    assert!(
        shell
            .requests
            .contains(&"[origin] GET /upstream/a.tar.gz authorization=<absent>".to_owned()),
        "the token followed a publisher-controlled url: {:?}",
        shell.requests
    );
    let argv = String::from_utf8_lossy(handed(&shell.handed_over, "inspect-argv"));
    assert_eq!(
        argv, "archive.zip\nentry.json\n--upstream\nupstream-archive\n",
        "the child was handed the wrong argument shape"
    );
    // The bytes behind `--upstream` are the publisher's, downloaded
    // without the token and passed through untouched.
    let downloaded = String::from_utf8_lossy(handed(&shell.handed_over, "upstream-"));
    assert_eq!(downloaded, "upstream bytes");
}

/// A failed upstream fetch never rejects and never counts: the verifier
/// still runs, without the file. A flaky upstream host is not the
/// publisher's fault, and a false rejection is terminal where pending
/// is recoverable.
#[test]
fn a_failed_upstream_fetch_still_verifies_without_it() {
    if !ready(
        "a_failed_upstream_fetch_still_verifies_without_it",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let line = "scope/pkg@1.0.0: upstream archive download failed; verifying without it";
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let metadata = format!("{{\"upstream\":{{\"url\":\"{origin}/upstream/a.tar.gz\"}}}}");
    let table = routes(&origin, listing(&[entry("scope/pkg", "1.0.0", &metadata)]));
    mock.serve(move |request: &Recorded| {
        if request.path == "/upstream/a.tar.gz" {
            return Reply::status(404);
        }
        table(request)
    });

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff(
        "a missing upstream",
        &shell,
        &port,
        &Diagnostics::Lines(&[line]),
    );
    assert_eq!(shell.status, Some(0), "a flaky host is not a failure");
    assert_eq!(
        shell.out(),
        "1 pending version(s)\nscope/pkg@1.0.0: verified\n",
        "the verifier still rendered a verdict"
    );
    let argv = String::from_utf8_lossy(handed(&shell.handed_over, "inspect-argv"));
    assert!(!argv.contains("--upstream"));
}

/// The entry the child reads is `jq -c` of the listing element, written
/// with no trailing newline. Non-ASCII stays raw, escapes stay escaped,
/// and nested metadata survives - the child parses these bytes, so a
/// re-render that "cleans them up" is a wire change.
///
/// The non-ASCII sits in a metadata value rather than in the name on
/// purpose. A name is concatenated into the artifact and verdict URLs,
/// where `curl` sends the bytes as they are and any URL type
/// percent-encodes them; package names are ASCII, so pinning that
/// divergence would pin a shape neither side can be reached with.
#[test]
fn the_entry_bytes_reach_the_child_unchanged() {
    if !ready("the_entry_bytes_reach_the_child_unchanged", &LIVE_TOOLS) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let metadata = "{\"upstream\":{\"url\":null},\"notes\":\"q\\\"s\\\\b\\tt\\nl \u{e9}\u{4e2d}\",\
                    \"deep\":{\"a\":[1,2,{\"b\":true}]},\"n\":9007199254740993}";
    let only = entry("scope/pkg", "1.0.0-rc.1+build.2", metadata);
    mock.serve(routes(&origin, listing(std::slice::from_ref(&only))));

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff(
        "an entry with everything in it",
        &shell,
        &port,
        &Diagnostics::Exact,
    );

    let written = String::from_utf8_lossy(handed(&shell.handed_over, "inspect-entry"));
    assert_eq!(
        written, only,
        "the entry was re-rendered on the way through"
    );
    assert!(
        !written.ends_with('\n'),
        "the entry gained a trailing newline"
    );
    assert!(
        shell
            .out()
            .contains("scope/pkg@1.0.0-rc.1+build.2: verified"),
        "{:?}",
        shell.out()
    );
}

/// Several versions exercise the shuffle, so the comparison is on the
/// set of lines and the set of requests rather than their order. The
/// shuffle itself - that it is a permutation, and not always the
/// identity - is unit-tested beside the port.
#[test]
fn several_versions_produce_the_same_set_of_effects() {
    if !ready(
        "several_versions_produce_the_same_set_of_effects",
        &LIVE_TOOLS,
    ) {
        return;
    }
    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    // The last two collide under a plain `/` -> `-` mapping, which
    // would share one recording key and one invocation counter.
    let entries: Vec<String> = ["scope/pkg0", "scope/pkg1", "a-b/c", "a/b-c"]
        .into_iter()
        .map(|name| entry(name, "1.0.0", "{}"))
        .collect();
    mock.serve(routes(&origin, listing(&entries)));

    let (mut shell, mut port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    shell.requests.sort();
    port.requests.sort();
    shell.bodies.sort();
    port.bodies.sort();
    // The shuffle permits a different ORDER and nothing else: the same
    // lines, the same number of them, and the same framing. A spurious
    // diagnostic or a headline that moved off line one still fails.
    // Split on the byte, so a CR that moved is a difference and the
    // trailing empty element carries the final newline into the
    // comparison.
    let sorted = |bytes: &[u8]| {
        let mut lines: Vec<Vec<u8>> = bytes
            .split(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        lines.sort();
        lines
    };
    for (stream, shell_side, port_side) in [
        ("stdout", &shell.stdout, &port.stdout),
        ("stderr", &shell.stderr, &port.stderr),
    ] {
        assert!(
            sorted(shell_side) == sorted(port_side),
            "{stream}, as a multiset of lines\nshell: {}\nport:  {}",
            shell_side.escape_ascii(),
            port_side.escape_ascii()
        );
        // Sorting moves the trailing empty element as readily as any
        // other line, so the final newline is pinned on its own.
        assert_eq!(
            shell_side.ends_with(b"\n"),
            port_side.ends_with(b"\n"),
            "{stream}: final newline"
        );
        // Only the per-version lines may move; the headline is line one
        // on both sides or the ordering allowance has hidden something.
        let first = |bytes: &[u8]| {
            bytes
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap_or_default()
                .to_vec()
        };
        assert_eq!(
            first(shell_side),
            first(port_side),
            "{stream}: the first line moved"
        );
    }
    assert_eq!(
        shell.stdout.split(|byte| *byte == b'\n').next(),
        Some(&b"4 pending version(s)"[..]),
        "the headline is not the first line"
    );
    assert_eq!(shell.bodies, port.bodies, "the verdict bodies, as a set");
    assert_eq!(shell.status, port.status, "exit status");
    assert_eq!(shell.requests, port.requests, "requests, as a set");
    let keys: Vec<&str> = shell
        .handed_over
        .iter()
        .filter(|(name, _)| name.starts_with("inspect-argv-"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "inspect-argv-a--b-c-1",
            "inspect-argv-a-b--c-1",
            "inspect-argv-scope-pkg0-1",
            "inspect-argv-scope-pkg1-1"
        ],
        "two names shared a recording key"
    );
    assert!(shell.out().ends_with('\n'), "stdout lost its final newline");
    assert!(
        shell.err().is_empty(),
        "nothing failed, so nothing is diagnosed"
    );
    assert_eq!(shell.handed_over, port.handed_over, "the verifier's inputs");

    assert_eq!(shell.status, Some(0));
    assert_eq!(
        shell
            .requests
            .iter()
            .filter(|l| l.contains("] PATCH "))
            .count(),
        4,
        "every version got a verdict"
    );
    assert!(shell.out().starts_with("4 pending version(s)\n"));
}

/// `[ "$count" -eq 0 ]` is the only integer test in the script, and a
/// count that is not an integer fails it rather than matching zero: the
/// headline prints with the junk count in it, the corpus is fetched,
/// and then `seq 0 $((count - 1))` walks nothing. Three ways to get
/// there, all of them things the admin plane could return on a bad day.
#[test]
fn a_count_that_is_not_a_number_prints_and_walks_nothing() {
    let test = "a_count_that_is_not_a_number_prints_and_walks_nothing";
    if !ready(test, &LIVE_TOOLS) {
        return;
    }
    if !gnu_seq() {
        eprintln!("skipping {test}: seq is not the GNU one the workflow runs");
        return;
    }
    for (case, pending) in [
        // `length` of a number is its magnitude, so a `versions` that is
        // not an array yields a fractional count.
        ("a fractional count", "{\"versions\":2.5}"),
        // `jq` runs its filter once per document and prints one line
        // each, so a concatenated answer yields a multi-line count.
        ("two documents", "{\"versions\":[1]}\n{\"versions\":[1]}"),
        // No input at all: the filter runs zero times and prints
        // nothing, so the count is the empty string.
        ("an empty body", ""),
    ] {
        let mut mock = Mock::start();
        let origin = mock.origin.clone();
        mock.serve(routes(&origin, pending.to_owned()));

        let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
        // bash writes its own lines here - `[: 2.5: integer expected`
        // and an arithmetic syntax error - and they name the script's
        // own path, so nothing that is not bash can reproduce them. No
        // line is common to both sides, so none is asserted; stdout,
        // the exit status, the requests and the verifier's inputs
        // still compare exactly.
        diff(case, &shell, &port, &Diagnostics::Lines(&[]));
        assert!(
            port.err().is_empty(),
            "{case}: the port invented a diagnostic where bash's noise stood: {}",
            port.err()
        );
        assert_eq!(
            shell.status,
            Some(0),
            "{case}: nothing failed, so nothing fails"
        );
        // Both sides printing nothing would compare equal, so pin that
        // the headline really was printed, junk count and all.
        assert!(
            shell.out().ends_with(" pending version(s)\n"),
            "{case}: no headline: {:?}",
            shell.out()
        );
        assert_eq!(
            shell.requests.len(),
            3,
            "{case}: the corpus is fetched and then nothing is walked: {:?}",
            shell.requests
        );
        assert!(
            shell
                .requests
                .iter()
                .any(|line| line.starts_with("[origin] GET /api/v1/admin/packages ")),
            "{case}: the corpus fetch is past the headline, so it still happens: {:?}",
            shell.requests
        );
        assert!(
            !shell.requests.iter().any(|line| line.contains("] PATCH ")),
            "{case}: a verdict was sent for a version nobody walked: {:?}",
            shell.requests
        );
        assert!(
            shell.handed_over.is_empty(),
            "{case}: the verifier ran anyway: {:?}",
            shell.handed_over
        );
    }
}

/// The archive and the upstream file are opaque bytes on their way to
/// the child - never decoded, never re-encoded. A body that is not
/// valid UTF-8 is the case a text-shaped oracle cannot see: two
/// different archives both render as replacement characters and
/// compare equal.
#[test]
fn opaque_bodies_reach_the_child_byte_for_byte() {
    if !ready("opaque_bodies_reach_the_child_byte_for_byte", &LIVE_TOOLS) {
        return;
    }
    // A local zip header, a lone continuation byte, an unpaired
    // surrogate's worth of nonsense, a NUL, and a byte that is never
    // valid UTF-8 anywhere.
    let archive_body: Vec<u8> = vec![
        0x50, 0x4b, 0x03, 0x04, 0x80, 0xbf, 0x00, 0xed, 0xa0, 0x80, 0xff, 0xfe, 0x0a, 0x41,
    ];
    let upstream_body: Vec<u8> = vec![0x1f, 0x8b, 0x08, 0x00, 0xff, 0x00, 0xc0, 0x80, 0xfd];

    let mut mock = Mock::start();
    let origin = mock.origin.clone();
    let metadata = format!("{{\"upstream\":{{\"url\":\"{origin}/upstream/a.tar.gz\"}}}}");
    let table = routes(&origin, listing(&[entry("scope/pkg", "1.0.0", &metadata)]));
    let (served_archive, served_upstream) = (archive_body.clone(), upstream_body.clone());
    mock.serve(move |request: &Recorded| match request.path.as_str() {
        "/upstream/a.tar.gz" => Reply::bytes(served_upstream.clone()),
        path if path.starts_with("/artifacts/") => Reply::bytes(served_archive.clone()),
        _ => table(request),
    });

    let (shell, port) = both(Some(&mock), &Verifier::default(), TOKEN, &origin, &origin);
    diff("opaque bodies", &shell, &port, &Diagnostics::Exact);
    assert_eq!(shell.status, Some(0));
    assert_eq!(
        handed(&shell.handed_over, "archive-"),
        archive_body,
        "the archive was not the bytes the mock served"
    );
    assert_eq!(
        handed(&shell.handed_over, "upstream-"),
        upstream_body,
        "the upstream file was not the bytes the mock served"
    );
}
