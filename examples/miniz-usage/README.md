# miniz-usage

A consumer example for the `cabin-ports/miniz` registry package,
published from the curated
[`crates/cabin-port/ports/miniz/3.1.2/`](../../crates/cabin-port/ports/miniz/3.1.2/)
package directory.  The program links against miniz, round-trips a string
through `mz_compress` / `mz_uncompress`, and prints the decompressed
text plus `mz_version()`.

The package carries upstream's official **amalgamated release
sources** - the repository's split sources need a CMake-generated
`miniz_export.h`, which the port never generates.  (`mz_version()`
reports miniz's internal zlib-style version string, `11.3.2` for
release 3.1.2; the two numberings differ upstream on purpose.)

This is **not** itself a port and does not vendor or copy miniz
sources.  It demonstrates depending on a curated C library through
an ordinary scoped registry package.  The first `cabin build`
resolves the dependency against the registry index, downloads the
published package archive, verifies its checksum, extracts it under
Cabin's cache, and then builds normally; subsequent builds reuse the
cache.

## Build and run

```sh
cd examples/miniz-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved package pins):

```
miniz roundtrip: Cabin compresses with miniz
miniz version: 11.3.2
```

## Offline

The first `cabin build` needs the registry.  Reads resolve through
the hosted registry by default, and verified packages download
without an account or token - `cabin login` is only needed to publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::miniz_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed ports into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
