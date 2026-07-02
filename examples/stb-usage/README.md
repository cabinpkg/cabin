# stb-usage

A consumer example for the curated
[`crates/cabin-port/ports/stb/2026.4.15/`](../../crates/cabin-port/ports/stb/2026.4.15/)
foundation port.  The program includes `stb_sprintf.h` with
`STB_SPRINTF_IMPLEMENTATION` defined - the stb single-file pattern,
where exactly one translation unit hosts the function bodies - and
prints a formatted string.

stb publishes no tagged releases, so the port pins one upstream
commit by its immutable tarball URL and SHA-256; the port version is
that commit's date spelled as SemVer (`2026.4.15`), which is why the
dependency requirement reads `^2026`.

This is **not** itself a port and does not vendor or copy stb
sources.  It demonstrates depending on a curated header-only C
foundation port from a normal Cabin package.  The first `cabin build`
downloads the upstream archive (URL and SHA-256 pinned by the port
recipe), verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/stb-usage
cabin build
cabin run
```

Expected output:

```
stb_sprintf formatted: Cabin scores 42
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::stb_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
