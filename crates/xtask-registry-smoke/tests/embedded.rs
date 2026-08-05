//! Level 1 of the differential harness (the port plan's §6): every pure
//! byte-level program `registry/scripts/smoke.sh` embedded is replayed
//! here through its *real* interpreter and diffed against the Rust port,
//! so "the port emits what the shell emitted" stays a fact this suite
//! re-derives rather than a claim one review made once.
//!
//! Each program is selected out of the script by content and asserted
//! unique.  An extraction that quietly matched nothing would diff empty
//! against empty and pass for the wrong reason - the exact failure this
//! suite exists to rule out - so [`diff`] also refuses an empty shell
//! side.  The extracted text is never edited: it is wrapped verbatim in
//! a driver that calls it.
//!
//! Every test skips (never fails) when its interpreter is absent, which
//! is what makes the file portable to a runner without `node`, `python3`
//! or `openssl`.  The suite is Unix-only outright: the original is a
//! bash script, and a Windows host's lookalike tools (Git Bash, the
//! App Store `python3` stub) EXIST on PATH but do not mean the same
//! thing, so a presence check passes and the drivers then fail.  The
//! port's helpers keep their cross-platform coverage through the
//! crate's own unit tests.
#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use assert_fs::TempDir;
use xtask_registry_smoke::bytes::{frame, retarget_hash, revision_of, tamper_zip};
use xtask_registry_smoke::legs::session::{listing_entry, session_cookie};

/// The shell functions this suite replays, each also the extraction key.
const PROGRAMS: [&str; 6] = [
    "u32le",
    "frame",
    "revision_of",
    "retarget_hash",
    "tamper_zip",
    "listing_entry",
];

/// L752: the session MAC is a bare statement rather than a function, so
/// it is selected by its own unique line.
const MAC_STATEMENT: &str = r#"session_mac="$(printf 'session:%s'"#;

/// The 64 hex the frozen fixture's `checksum` and `source.path` name
/// (L1338 reads the same digest out of the metadata at run time).
const FIXTURE_HASH: &str = "1ca5e59a7e30f792fc32a833252dbbd18ae617298082905aa6e4c27699bcc8ea";

/// A digest no fixture carries, so a substitution that failed to apply
/// cannot coincidentally match.
const NEW_HASH: &str = "9f0e1d2c3b4a59687766554433221100ffeeddccbbaa99887766554433221100";

#[test]
fn tamper_zip_flips_the_same_byte() {
    let Some(dir) = ready("tamper_zip", &["python3"]) else {
        return;
    };
    let program = shell_fn(&original_script(), "tamper_zip");
    let driver = driver(&program, r#"tamper_zip "$1" "$2" "$3""#);
    let odd = (0u8..=254).collect::<Vec<_>>();
    let corpus: [(&str, &[u8], u32); 4] = [
        (
            "the frozen fixture, seed 1",
            &fixture("smoke-withdep-0.2.0.zip"),
            1,
        ),
        (
            "the frozen fixture, seed 7",
            &fixture("smoke-withdep-0.2.0.zip"),
            7,
        ),
        // A seed whose low byte is zero must still flip a bit (`or 1`).
        ("ten zero bytes, seed 256", &[0u8; 10], 0x100),
        ("an odd length, seed 255", &odd, 255),
    ];

    for (label, source, seed) in corpus {
        let src = dir.path().join("src.zip");
        let dst = dir.path().join("dst.zip");
        fs::write(&src, source).expect("writing the source");
        run(
            dir.path(),
            &driver,
            &[path(&src), path(&dst), seed.to_string()],
            &[],
        );
        let shell = fs::read(&dst).expect("reading the tampered copy");
        diff(
            &format!("tamper_zip ({label})"),
            &shell,
            &tamper_zip(source, seed),
        );
    }
}

/// `frame` drives `u32le`, which is where the risk is: the four length
/// bytes are usually NULs, which is why the shell emitted them through
/// nested octal `printf` rather than a command substitution.
#[test]
fn frame_lays_out_the_same_bytes() {
    let Some(dir) = ready("frame", &["wc", "cat"]) else {
        return;
    };
    let script = original_script();
    let program = format!(
        "{}\n{}",
        shell_fn(&script, "u32le"),
        shell_fn(&script, "frame")
    );
    let driver = driver(&program, r#"frame "$1" "$2" "$3""#);
    let archive = fixture("smoke-withdep-0.2.0.zip");
    let corpus: [(&str, Vec<u8>, &[u8]); 4] = [
        (
            "the frozen pair",
            fixture("smoke-withdep-0.2.0.json"),
            &archive,
        ),
        // 256 and 65536 put a NUL in every length byte but one.
        ("a 256-byte metadata", vec![b'm'; 256], &archive),
        ("a 65536-byte metadata", vec![b'm'; 65536], &archive),
        ("two empty inputs", Vec::new(), &[]),
    ];

    for (label, metadata, archive) in corpus {
        let meta_path = dir.path().join("metadata");
        let archive_path = dir.path().join("archive");
        let out = dir.path().join("framed.bin");
        fs::write(&meta_path, &metadata).expect("writing the metadata");
        fs::write(&archive_path, archive).expect("writing the archive");
        run(
            dir.path(),
            &driver,
            &[path(&meta_path), path(&archive_path), path(&out)],
            &[],
        );
        let shell = fs::read(&out).expect("reading the framed body");
        diff(
            &format!("frame ({label})"),
            &shell,
            &frame(&metadata, archive),
        );
    }
}

#[test]
fn revision_of_takes_the_same_sixteen_hex() {
    let Some(dir) = ready("revision_of", &["shasum", "cut"]) else {
        return;
    };
    let program = shell_fn(&original_script(), "revision_of");
    let driver = driver(&program, r#"revision_of "$1""#);

    for (label, archive) in [
        ("the frozen fixture", fixture("smoke-withdep-0.2.0.zip")),
        ("an empty archive", Vec::new()),
    ] {
        let src = dir.path().join("archive");
        fs::write(&src, &archive).expect("writing the archive");
        let shell = run(dir.path(), &driver, &[path(&src)], &[]);
        // `$(revision_of ...)` drops the trailing newline the command
        // substitution stripped, which is what the port returns.
        let port = format!("{}\n", revision_of(&archive));
        diff(&format!("revision_of ({label})"), &shell, port.as_bytes());
    }
}

#[test]
fn retarget_hash_rewrites_the_same_two_spans() {
    let Some(dir) = ready("retarget_hash", &["sed"]) else {
        return;
    };
    let program = shell_fn(&original_script(), "retarget_hash");
    let driver = driver(&program, r#"retarget_hash "$1" "$2""#);
    // The second document names the 16-char prefix standalone, with no
    // full digest on that line: the prefix substitution has to reach it,
    // and the full-digest substitution must not corrupt it first.
    let standalone = format!(
        "path = \"artifacts/{}/pkg.zip\"\nchecksum = \"sha256:{FIXTURE_HASH}\"\n",
        &FIXTURE_HASH[..16]
    );
    let corpus = [
        ("the frozen metadata", fixture("smoke-withdep-0.2.0.json")),
        ("a standalone prefix", standalone.into_bytes()),
    ];

    for (label, document) in corpus {
        let shell = run(
            dir.path(),
            &driver,
            &[FIXTURE_HASH.to_owned(), NEW_HASH.to_owned()],
            &document,
        );
        let port = retarget_hash(&document, FIXTURE_HASH, NEW_HASH);
        diff(&format!("retarget_hash ({label})"), &shell, &port);
        assert!(
            !shell
                .windows(16)
                .any(|window| window == &FIXTURE_HASH.as_bytes()[..16]),
            "retarget_hash ({label}) left the old digest behind, so the corpus proves nothing"
        );
    }
}

#[test]
fn listing_entry_emits_the_same_json() {
    let Some(dir) = ready("listing_entry", &["node"]) else {
        return;
    };
    let program = shell_fn(&original_script(), "listing_entry");
    let driver = driver(&program, r#"listing_entry "$1" "$2" "$3" "$4""#);
    // Two decoys, a field order the port must not sort, and one nested
    // object: `JSON.stringify` emits insertion order at every depth.
    let listing = br#"{"versions":[
      {"name":"smoke/other","version":"0.2.0","revision":"dead","checksum":"c","published_at":"1","metadata":{}},
      {"name":"smoke/withdep","version":"0.1.0","revision":"beef","checksum":"c","published_at":"1","metadata":{}},
      {"name":"smoke/withdep","version":"0.2.0","revision":"1ca5e59a7e30f792",
       "checksum":"sha256:1ca5","published_at":"2026-01-02T03:04:05Z","yanked":false,
       "metadata":{"schema":1,"name":"smoke/withdep","links":{"withdep":"withdep-native"}}}
    ]}"#;
    let source = dir.path().join("pending.json");
    let shell_out = dir.path().join("shell-entry.json");
    let port_out = dir.path().join("port-entry.json");
    fs::write(&source, listing).expect("writing the listing");

    run(
        dir.path(),
        &driver,
        &[
            path(&source),
            "smoke/withdep".to_owned(),
            "0.2.0".to_owned(),
            path(&shell_out),
        ],
        &[],
    );
    listing_entry(listing, "smoke/withdep", "0.2.0", &port_out).expect("the port entry");

    diff(
        "listing_entry",
        &fs::read(&shell_out).expect("the shell entry"),
        &fs::read(&port_out).expect("the port entry"),
    );
}

/// L751-754.  The port's own unit test pins the MAC against a hand-taken
/// golden; this one re-derives it from the script's `openssl` line, so a
/// golden that was wrong when it was written cannot stay wrong.
#[test]
fn the_session_mac_matches_the_scripts_openssl() {
    let Some(dir) = ready("session_mac", &["openssl", "sed"]) else {
        return;
    };
    let statement = statement(&original_script(), MAC_STATEMENT, 2);

    for expires_at in [1_234_567_890u64, 1_900_000_000] {
        let driver = driver(
            &format!("session_payload=\"0:{expires_at}\"\n{statement}"),
            r#"printf '%s' "$session_mac""#,
        );
        let shell = run(dir.path(), &driver, &[], &[]);
        let cookie = session_cookie(0, expires_at);
        let port = cookie.rsplit('.').next().expect("the mac segment");
        diff(
            &format!("session_mac (expires_at {expires_at})"),
            &shell,
            port.as_bytes(),
        );
    }
}

/// The suite's own non-vacuity check: every extraction key still selects
/// exactly one block of the script.  A renamed or deleted program fails
/// here rather than silently reducing the corpus to nothing.
#[test]
fn every_program_is_extracted_exactly_once() {
    let script = original_script();
    let functions = PROGRAMS
        .iter()
        .filter(|name| !shell_fn(&script, name).is_empty())
        .count();
    let statements = usize::from(!statement(&script, MAC_STATEMENT, 2).is_empty());
    assert_eq!(
        functions + statements,
        PROGRAMS.len() + 1,
        "every embedded program must still be extractable from the script"
    );
}

/// The shell the port replaces, vendored verbatim at its deletion
/// (byte-identical to the blob the migration commit removed - sha256
/// fc06351668b64001e9b6b711e8fa105c74cdde0862f7f7baa7b693cb7b4df257).
/// A checked-in fixture rather than a `git show`: CI checkouts are
/// shallow, so history is not assumed anywhere.  Reference DATA, not
/// tooling - nothing in the repository executes it; these tests feed
/// fragments of it to interpreters explicitly.
fn original_script() -> String {
    include_str!("fixtures/smoke.sh.orig").to_owned()
}

/// The text of shell function `name`, from its `name() {` line through
/// the brace that closes it, asserted to be the only such definition.
fn shell_fn(script: &str, name: &str) -> String {
    let opener = format!("{name}() {{");
    let lines: Vec<&str> = script.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with(&opener))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "`{opener}` must appear exactly once in the script, found {}",
        starts.len()
    );
    let start = starts[0];
    if lines[start].ends_with('}') {
        return format!("{}\n", lines[start]);
    }
    let end = (start + 1..lines.len())
        .find(|&at| lines[at] == "}")
        .unwrap_or_else(|| panic!("`{opener}` is never closed"));
    format!("{}\n", lines[start..=end].join("\n"))
}

/// `count` lines of the script starting at the only line containing
/// `needle`: the selector for the statements that are not functions.
fn statement(script: &str, needle: &str, count: usize) -> String {
    let lines: Vec<&str> = script.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "`{needle}` must appear exactly once in the script, found {}",
        starts.len()
    );
    let start = starts[0];
    format!("{}\n", lines[start..start + count].join("\n"))
}

/// A runnable script: the extracted program verbatim, then one call.
fn driver(program: &str, call: &str) -> String {
    format!("set -euo pipefail\n{program}\n{call}\n")
}

/// Run `driver` under the real `bash` in `dir`, and return its stdout.
fn run(dir: &Path, driver: &str, argv: &[String], stdin: &[u8]) -> Vec<u8> {
    let script = dir.join("driver.sh");
    fs::write(&script, driver).expect("writing the driver");
    let mut child = Command::new("bash")
        .arg(&script)
        .args(argv)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("running bash");
    child
        .stdin
        .take()
        .expect("the driver's stdin")
        .write_all(stdin)
        .expect("writing the driver's stdin");
    let output = child.wait_with_output().expect("the driver's output");
    assert!(
        output.status.success(),
        "the shell driver failed ({}):\n{driver}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// The whole assertion: identical bytes, and a shell side that actually
/// produced some.
fn diff(program: &str, shell: &[u8], port: &[u8]) {
    assert!(
        !shell.is_empty(),
        "{program}: the shell side produced no bytes - the extraction matched the wrong block"
    );
    if shell == port {
        return;
    }
    let at = shell
        .iter()
        .zip(port)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| shell.len().min(port.len()));
    panic!(
        "{program}: the shell and the port differ at offset {at} \
         (shell {} bytes, port {} bytes)\n  shell: {:02x?}\n  port:  {:02x?}",
        shell.len(),
        port.len(),
        window(shell, at),
        window(port, at)
    );
}

/// The 16 bytes from the first difference, which is what a byte-level
/// mismatch is read from.
fn window(bytes: &[u8], at: usize) -> &[u8] {
    &bytes[at..bytes.len().min(at + 16)]
}

/// A hermetic directory for `program`, or `None` after announcing which
/// tool is missing.  `bash` gates every test: the shell side is run, not
/// re-implemented.
fn ready(program: &str, tools: &[&str]) -> Option<TempDir> {
    for tool in std::iter::once(&"bash").chain(tools) {
        if !which(tool) {
            eprintln!("skipping the {program} differential: no `{tool}` on PATH");
            return None;
        }
    }
    Some(TempDir::new().expect("a temporary directory"))
}

fn which(tool: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        dir.join(format!("{tool}{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    })
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> Vec<u8> {
    let path = repo().join("registry/tests/fixtures").join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn path(at: &Path) -> String {
    at.to_str().expect("a UTF-8 path").to_owned()
}
