# cli11-usage

A consumer example for the curated
[`crates/cabin-port/ports/CLI11/2.6.2/`](../../crates/cabin-port/ports/CLI11/2.6.2/)
foundation port.  The program includes the header-only CLI11 command
line parser, declares one option with a default, parses `argv`, and
prints the parsed value plus the compiled-in CLI11 version.

This is **not** itself a port and does not vendor or copy CLI11
sources.  It demonstrates depending on a curated header-only C++
foundation port from a normal Cabin package.  The first `cabin build`
downloads the upstream archive (URL and SHA-256 pinned by the port
recipe), verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/cli11-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
CLI11 parsed count: 3
CLI11 version: 2.6.2
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::cli11_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
