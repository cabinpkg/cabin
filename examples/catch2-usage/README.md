# catch2-usage

A consumer example for the curated
[`crates/cabin-port/ports/catch2/3.15.1/`](../../crates/cabin-port/ports/catch2/3.15.1/)
foundation port.  The package has a small `calc` library and one
`test` target whose source only defines `TEST_CASE`s - the port's
amalgamated translation unit supplies Catch2's default `main()`.

The port carries Catch2's upstream amalgamation
(`extras/catch_amalgamated.cpp` / `.hpp`), so tests include
`<catch_amalgamated.hpp>` rather than the split `<catch2/...>`
headers (those need a CMake-generated configuration header, which
foundation ports never generate).  A consumer that wants its own
entry point enables the port's `custom-main` feature:

```toml
catch2 = { port = true, version = "^3.15", features = ["custom-main"] }
```

This is **not** itself a port and does not vendor or copy Catch2
sources.  The first `cabin test` downloads the upstream archive (URL
and SHA-256 pinned by the port recipe), verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent runs reuse the cache.

## Run the tests

```sh
cd examples/catch2-usage
cabin test
```

Expected output ends with:

```
test catch2-usage:calc_catch2 ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ...
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent runs work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::catch2_usage_runs_tests`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
