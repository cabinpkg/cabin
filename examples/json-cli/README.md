# json-cli

A miniature manifest-inspector built on the curated header-only
[`nlohmann_json`](../../crates/cabin-port/ports/nlohmann_json) foundation port.  Where
[`nlohmann-json-usage/`](../nlohmann-json-usage) is the minimal consumption smoke test, this
example walks a realistic JSON round trip: parse a document, read typed values out of nested
objects and arrays, and serialize a derived summary back to JSON.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` downloads
the upstream archive (URL and SHA-256 pinned by the port recipe), verifies its checksum, extracts
it under Cabin's cache, and then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/json-cli
cabin build
cabin run
```

Expected output:

```
package: json-cli v0.1.0
dependency count: 3
  dep: fmt
  dep: spdlog
  dep: sqlite3
summary: {"deps":["fmt","spdlog","sqlite3"],"name":"json-cli"}
```

For the C equivalent of this use case, see [`cjson-usage/`](../cjson-usage), which consumes the
cJSON port from a `.c` source.

## Offline

If you have no network the first time, the build fails with a clear "cannot download port" error.
Once the archive is already cached, subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::json_cli_builds_and_runs`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `github.com:443`, so a CI runner without
outbound network does not fail the suite.
