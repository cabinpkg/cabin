# Fuzz targets

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) targets
for the code that parses hostile input. Each target calls the production
API directly; the archive targets check a property, not just the absence
of a panic.

| Target | Input | Property |
|---|---|---|
| `apply_patch` | patch bytes, applied as a declared upstream patch to a fixed C and C++ tree | the tree stays directories and singly linked regular files in its root and a file beside it stays as it was; an accepted patch has a hunk, and nothing before its first `@@` line opens a C or C++ comment or raw string, splices lines, spells a trigraph, or starts a directive |
| `extract_tar_gz` | tar bytes, gzipped by the harness | the upstream materializer's extraction (symlinks skipped, with the seeds' `pkg/` prefix stripped and without), whether it accepts or refuses, leaves its archive intact, writes nothing beside its destination, and leaves only directories and singly linked regular files in it |
| `extract_zip` | zip bytes | the same, for zip |
| `inspect_zip` | archive bytes, listed with their own checksum and the metadata publish derives from their `cabin.toml` (or from the harness's fallback manifest when the client's reader finds none) | anything the registry verifier verifies opens in the client's zip reader as regular files with safe, portable, unique UTF-8 names, CRC-clean, with a root `cabin.toml` |
| `inspect_zip_wrong_digest` | archive bytes, listed with another archive's checksum | never verifies |
| `package_round_trip` | a package's `cabin.toml`, then its other files, each a NUL, its path, a newline, and its contents, written to a scratch directory and staged through the packager | a package that stages, unless it declares an upstream or exceeds an extraction cap `cabin package` leaves to consumers, verifies as listed with the metadata staging produced, opens in the client's zip reader as `inspect_zip` requires, and fetches through the client to exactly the archive's files, each the package's own file at that path |
| `parse_manifest` | `cabin.toml` text | parsing, the publishability check, and canonical-metadata derivation never panic |

`src/lib.rs` holds the shared harness. Its unit tests check that every
seed archive verifies and that one carries the fallback manifest, so the
`inspect_zip` property is reachable, that every extraction seed
extracts with its prefix stripped and without, so containment is checked
past the extractor's own refusals, that every patch seed applies, so the
grammar property is reached, and that every package seed stages, so the
round trip runs.

## Running

The targets build on the nightly pinned in `rust-toolchain.toml`, which
only applies when cargo runs from this directory:

```sh
cd fuzz
mkdir -p corpus/inspect_zip
cargo fuzz run inspect_zip corpus/inspect_zip seeds/inspect_zip -- -dict=dicts/inspect_zip.dict
```

libFuzzer writes new inputs to the first directory (`corpus/`, ignored by
git) and crash inputs to `artifacts/`. `inspect_zip_wrong_digest` runs on
the `inspect_zip` seeds and dictionary. Replay one input with
`cargo fuzz run <target> <file>`, minimize it with
`cargo fuzz tmin <target> <file>`, and measure coverage with
`cargo fuzz coverage <target> corpus/<target>`. `cargo test` here runs the
harness's own test.

`cargo fuzz-check`, from the repository root, is what CI runs: `cargo fmt`,
`clippy`, and `test` here, and a replay of every target's seeds with its
dictionary, so a target without either fails. It needs `cargo-fuzz`
(`cargo install cargo-fuzz --locked`); `cargo ci` does not run it, so run
it after changing an API a target calls.

`cargo fuzz-campaign`, from the repository root, is what the daily
`Fuzz campaign` workflow runs: five minutes of each target on
`corpus/<target>` and its seeds, then a libFuzzer `-merge=1` that
minimizes that corpus. The workflow keeps the corpus in the Actions
cache between runs. Only runs on this repository's branches write it,
never a pull request's, though any pull request can read it, and GitHub
drops an entry no run has restored for seven days. A failed target
prints only its name and loses its corpus, because both can hold the
crashing input. Nothing keeps that input, since a public repository's
logs, artifacts, and caches are all readable: reproduce the crash by
fuzzing that target locally.

`seeds/` is generated: `cargo fuzz-seeds`, from the repository root,
stages example packages (C and C++) and a package of the fallback
manifest through the real packager, writes the example packages' files
as round-trip specs, copies the published-archive fixture, re-wraps
those archives' files under a `pkg/` top directory as zip and tar for
the extraction targets, writes the patch seeds with `git diff` against
a copy of the harness's tree, and collects every `cabin.toml` under
`examples/` and `ports/`. `dicts/` holds the libFuzzer dictionaries.

The fuzz crate has its own `Cargo.lock`; Dependabot updates it
separately from the root lockfile, so shared dependencies (`zip`, `toml`,
the deflate backend, ...) can drift from the versions Cabin ships.

## OSS-Fuzz

`oss-fuzz/` holds the project files OSS-Fuzz would build Cabin from:
`project.yaml`, `Dockerfile`, and `build.sh`, which builds every target
with its seeds and dictionary. The `OSS-Fuzz build` workflow builds,
checks, and builds for coverage with OSS-Fuzz's own `infra/helper.py`
(at a pinned google/oss-fuzz commit; the images are OSS-Fuzz's current
ones), on pull requests that touch `fuzz/` and weekly. The check step
runs every fuzzer, so it prints nothing: a failure there is reproduced
locally.

The `Dockerfile` pins its own nightly, apart from `rust-toolchain.toml`
here: OSS-Fuzz's image defaults to one older than Cabin's MSRV, and its
coverage tools read only LLVM 22's profiles, which nightlies after
2026-08-05 no longer write. Move it to a later nightly only once
OSS-Fuzz's LLVM moves.

Cabin is not enrolled: enrolling submits these files to google/oss-fuzz
with a `primary_contact`, which is a maintainer decision, and with the
license header its presubmit requires. The same check locally (needs
Docker), from a google/oss-fuzz clone:

```sh
mkdir -p projects/cabin && cp <cabin>/fuzz/oss-fuzz/* projects/cabin/
python3 infra/helper.py build_image --pull cabin
python3 infra/helper.py build_fuzzers --clean --sanitizer address --engine libfuzzer cabin <cabin>
python3 infra/helper.py check_build --sanitizer address --engine libfuzzer cabin
```

## Crashes

A crash in a target can be a vulnerability in the client or the registry
verifier. Report it through [`SECURITY.md`](../SECURITY.md) and keep the
input out of public issues, pull requests, and logs until it is fixed.
The fix lands in the owning crate with a regression test there.
