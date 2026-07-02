# miniz-usage

A consumer example for the curated
[`crates/cabin-port/ports/miniz/3.1.2/`](../../crates/cabin-port/ports/miniz/3.1.2/)
foundation port.  The program links against miniz, round-trips a
string through `mz_compress` / `mz_uncompress`, and prints the
decompressed text plus `mz_version()`.

The port pins upstream's official **amalgamated release zip** - the
repository's split sources need a CMake-generated `miniz_export.h`,
which foundation ports never generate.  (`mz_version()` reports
miniz's internal zlib-style version string, `11.3.2` for release
3.1.2; the two numberings differ upstream on purpose.)

This is **not** itself a port and does not vendor or copy miniz
sources.  It demonstrates depending on a curated C foundation port
from a normal Cabin package.  The first `cabin build` downloads the
upstream archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/miniz-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
miniz roundtrip: Cabin compresses with miniz
miniz version: 11.3.2
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::miniz_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
