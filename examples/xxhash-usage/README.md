# xxhash-usage

A consumer example for the `cabin-ports/xxhash` registry package,
published from the curated
[`ports/xxhash/0.8.3/`](../../ports/xxhash/0.8.3/)
package directory.  The program links against xxHash and prints
`XXH_versionNumber()` plus the `XXH64` digest of a short
string.

This is **not** itself a port and does not vendor or copy xxHash
sources.  It demonstrates depending on a published registry package
from a normal Cabin package.  The first `cabin build` resolves
`"cabin-ports/xxhash" = "=0.8.3"` against the registry index,
downloads the published package archive, verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/xxhash-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved package pins):

```
xxHash version: 803
XXH64("Cabin") = ...
```

## Offline

Registry dependencies resolve through the hosted registry by
default; verified packages download
without an account or token - `cabin login` is only needed to
publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).
Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::xxhash_usage_builds_and_runs`)
runs only with `--ignored` and needs outbound network: it stages
the committed ports into a local file registry through the
publisher pipeline and builds against that with `--index-path`,
downloading the pinned upstream archives on the way.
