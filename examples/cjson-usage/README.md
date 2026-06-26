# cjson-usage

A consumer example for the curated
[`crates/cabin-port/ports/cJSON/1.7.18/`](../../crates/cabin-port/ports/cJSON/1.7.18/)
foundation port.  The program links against cJSON, parses a small
JSON document, and prints a field plus `cJSON_Version()`.

This is **not** itself a port and does not vendor or copy cJSON
sources.  It demonstrates depending on a curated foundation port from
a normal Cabin package.  The first `cabin build` downloads the
upstream archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/cjson-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
cJSON parsed name: Cabin
cJSON version: 1.7.18
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::cjson_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
