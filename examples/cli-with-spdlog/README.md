# cli-with-spdlog

A small command-line app that combines three curated foundation ports in one binary:

- [`CLI11`](../../crates/cabin-port/ports/CLI11) parses `--name` / `--count` flags.
- [`fmt`](../../crates/cabin-port/ports/fmt) formats the greeting lines.
- [`spdlog`](../../crates/cabin-port/ports/spdlog) logs what the app is about to do.

The spdlog port is header-only and defaults to its **bundled** {fmt} copy.  Cabin propagates
include dirs across dependency edges but not defines, so the opt-in to the external fmt port
happens in this package's own manifest: `defines = ["SPDLOG_FMT_EXTERNAL"]` on the executable
target reaches every translation unit that includes spdlog's headers, and all three libraries end
up sharing the single fmt port.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` downloads
the three upstream archives (URL and SHA-256 pinned by each port recipe), verifies checksums,
extracts them under Cabin's cache, and then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/cli-with-spdlog
cabin build
cabin run
```

Expected output (the log line carries a timestamp prefix):

```
[2026-07-02 12:34:56.789] [info] preparing 2 greeting(s) for Cabin
1/2: Hello, Cabin!
2/2: Hello, Cabin!
spdlog version: 1.17.0
fmt version (external): 120200
```

Pass flags through `cabin run` after `--`:

```sh
cabin run -- --name you --count 3
```

## Offline

If you have no network the first time, the build fails with a clear "cannot download port" error.
Once the archives are already cached, subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::cli_with_spdlog_builds_and_runs`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `github.com:443`, so a CI runner without
outbound network does not fail the suite.
