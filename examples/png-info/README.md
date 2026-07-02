# png-info

A tiny PNG inspector built on the curated [`libpng`](../../crates/cabin-port/ports/libpng)
foundation port.  The program encodes a 2x2 RGBA image to an in-memory PNG with libpng's
simplified write API, decodes it back, and prints the dimensions, channel count, and encoded size
- the information a `png-info` tool would report for a file on disk.

The point of the example is the **transitive C dependency**: libpng's port depends on the bundled
[`zlib`](../../crates/cabin-port/ports/zlib) port, and this package declares only `libpng`.  The
DEFLATE stream inside the PNG is produced and consumed by zlib code that arrives - headers and
archive both - through the `libpng -> zlib` port edge, and the final `zlibVersion()` call compiles
and links purely through that edge.  Where [`libpng-usage/`](../libpng-usage) proves the edge with
single symbol calls, this example pushes real image data through both libraries.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` downloads
both upstream archives (URL and SHA-256 pinned by each port recipe), verifies checksums, extracts
them under Cabin's cache, and then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/png-info
cabin build
cabin run
```

Expected output (the encoded byte count may vary with the zlib version):

```
png-info: 2x2, 4 channel(s), 84 byte(s) encoded
roundtrip pixels match: yes
libpng version: 1.6.50
zlib version (transitive port edge): 1.3.1
```

## Offline

If you have no network the first time, the build fails with a clear "cannot download port" error.
Once the archives are already cached, subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::png_info_builds_and_runs`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `downloads.sourceforge.net:443` (libpng)
or `github.com:443` (zlib), so a CI runner without outbound network does not fail the suite.
