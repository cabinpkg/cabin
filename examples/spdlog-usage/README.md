# spdlog-usage

A consumer example for the curated
[`crates/cabin-port/ports/spdlog/1.17.0/`](../../crates/cabin-port/ports/spdlog/1.17.0/)
foundation port.  The program uses spdlog in its upstream-default
header-only form, logs one message through `spdlog::info`, and
prints the compiled-in spdlog version.

This is **not** itself a port and does not vendor or copy spdlog
sources.  It demonstrates depending on a curated header-only C++
foundation port from a normal Cabin package.  The first `cabin build`
downloads the upstream archive (URL and SHA-256 pinned by the port
recipe), verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/spdlog-usage
cabin build
cabin run
```

Expected output (the log line carries a timestamp prefix; the version
is whatever the resolved port pins):

```
[2026-07-02 12:34:56.789] [info] Hello from spdlog!
spdlog version: 1.17.0
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::spdlog_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
