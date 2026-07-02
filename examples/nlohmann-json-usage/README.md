# nlohmann-json-usage

A consumer example for the curated
[`crates/cabin-port/ports/nlohmann_json/3.12.0/`](../../crates/cabin-port/ports/nlohmann_json/3.12.0/)
foundation port.  The program includes the header-only JSON for
Modern C++ library, parses a small document, and prints two fields
plus the compiled-in library version.

This is **not** itself a port and does not vendor or copy any
sources.  It demonstrates depending on a curated header-only C++
foundation port from a normal Cabin package.  The first `cabin build`
downloads the upstream archive (URL and SHA-256 pinned by the port
recipe), verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/nlohmann-json-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
json parsed name: Cabin
json parsed answer: 42
nlohmann_json version: 3.12.0
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::nlohmann_json_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
