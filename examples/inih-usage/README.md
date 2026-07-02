# inih-usage

A consumer example for the curated
[`crates/cabin-port/ports/inih/62.0.0/`](../../crates/cabin-port/ports/inih/62.0.0/)
foundation port.  The program links against the inih C core, parses
an in-memory INI document with `ini_parse_string`, and prints the
two values its handler captured.

Upstream tags releases as `r62`; the port spells that as the SemVer
`62.0.0`, which is why the dependency requirement reads `^62`.  The
port builds the C core (`ini.c`) only - the optional C++ `INIReader`
under `cpp/` is not part of it.

This is **not** itself a port and does not vendor or copy inih
sources.  It demonstrates depending on a curated C foundation port
from a normal Cabin package.  The first `cabin build` downloads the
upstream archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/inih-usage
cabin build
cabin run
```

Expected output:

```
inih parsed name: Cabin
inih parsed port: 8080
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::inih_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
