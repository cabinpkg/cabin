# xxhash-usage

A consumer example for the curated
[`crates/cabin-port/ports/xxhash/0.8.3/`](../../crates/cabin-port/ports/xxhash/0.8.3/)
foundation port. The program links against xxHash and prints
`XXH_versionNumber()` plus the `XXH64` digest of a short string.

This is **not** itself a port and does not vendor or copy xxHash
sources. It demonstrates depending on a curated foundation port from
a normal Cabin package. The first `cabin build` downloads the
upstream archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/xxhash-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
xxHash version: 803
XXH64("Cabin") = ...
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error. Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::xxhash_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
