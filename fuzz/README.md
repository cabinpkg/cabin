# Fuzz targets

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) targets
for the code that parses hostile input. Each target calls the production
API directly; the archive targets check a property, not just the absence
of a panic.

| Target | Input | Property |
|---|---|---|
| `inspect_zip` | archive bytes, listed with their own checksum and the metadata publish derives from their `cabin.toml` (or from the harness's fallback manifest when the client's reader finds none) | anything the registry verifier verifies opens in the client's zip reader as regular files with safe, portable, unique UTF-8 names, CRC-clean, with a root `cabin.toml` |
| `inspect_zip_wrong_digest` | archive bytes, listed with another archive's checksum | never verifies |
| `parse_manifest` | `cabin.toml` text | parsing, the publishability check, and canonical-metadata derivation never panic |

`src/lib.rs` holds the shared harness. Its unit tests check that every
seed archive verifies and that one carries the fallback manifest, so the
`inspect_zip` property is reachable.

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

`seeds/` is generated: `cargo fuzz-seeds`, from the repository root,
stages example packages (C and C++) and a package of the fallback
manifest through the real packager, copies the published-archive fixture,
and collects every `cabin.toml` under `examples/` and `ports/`. `dicts/` holds the libFuzzer dictionaries.

`Cargo.lock` was seeded from the root lockfile, so the shared
dependencies (`zip`, `toml`, the deflate backend, ...) started at the
versions Cabin ships; Dependabot updates the two lockfiles separately.

## Crashes

A crash in a target can be a vulnerability in the client or the registry
verifier. Report it through [`SECURITY.md`](../SECURITY.md) and keep the
input out of public issues, pull requests, and logs until it is fixed.
The fix lands in the owning crate with a regression test there.
