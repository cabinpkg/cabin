# png-info

A tiny PNG inspector built on the `cabin-ports/libpng` registry package, published from the
curated [`ports/libpng/`](../../ports/libpng) package
directory.  The
program encodes a 2x2 RGBA image to an in-memory PNG with libpng's simplified write API, decodes
it back, and prints the dimensions, channel count, and encoded size - the information a `png-info`
tool would report for a file on disk.

The point of the example is the **transitive C dependency**: the published libpng package declares
`"cabin-ports/zlib" = "^1.3"` (see [`zlib`](../../ports/zlib)), and this package
declares only `cabin-ports/libpng`.  The DEFLATE stream inside the PNG is produced and consumed by
zlib code whose archive joins the final link through the `libpng -> zlib` dependency edge.  That
edge is private - `png.h` never includes `zlib.h` - so this package compiles against libpng's
headers alone.  Where [`libpng-usage/`](../libpng-usage) calls zlib directly, this example pushes
real image data through both libraries without naming zlib.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` resolves
both packages against the registry index, downloads the published archives, verifies their
checksums, extracts them under Cabin's cache, and then builds normally; subsequent builds reuse
the cache.

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
```

## Offline

The first `cabin build` needs the registry.  Reads resolve through the hosted registry by default,
and verified packages download
without an account or token - `cabin login` is only needed to publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::png_info_builds_and_runs`) is
`#[ignore = "requires external network"]`: it stages the committed ports into a local file
registry through the publisher pipeline and builds this example against it with `--index-path`.
