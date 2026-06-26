# tinyxml2-usage

A consumer example for the curated
[`crates/cabin-port/ports/tinyxml2/11.0.0/`](../../crates/cabin-port/ports/tinyxml2/11.0.0/)
foundation port.  The program links against tinyxml2 (a C++
library), parses a small XML document, and prints an element's text
plus the compiled-in tinyxml2 version.

This is **not** itself a port and does not vendor or copy tinyxml2
sources.  It demonstrates depending on a curated C++ foundation port
from a normal Cabin package.  The first `cabin build` downloads the
upstream archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/tinyxml2-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
tinyxml2 parsed to: Cabin
tinyxml2 version: 11.0.0
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::tinyxml2_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
