# uthash-usage

A consumer example for the `cabin-ports/uthash` registry package,
published from the curated
[`crates/cabin-port/ports/uthash/2.4.0/`](../../crates/cabin-port/ports/uthash/2.4.0/)
recipe.  The program includes the header-only uthash macro library,
adds one entry to a hash table keyed by a string field, looks it up,
and prints the value plus the library version.

The upstream tarball ships a convenience `include -> src` symlink;
the extraction policy that stages the package from that tarball
skips symlink entries (nothing is materialized for them) and the
recipe's overlay points straight at `src/`, so the package publishes
cleanly.

This is **not** itself a port and does not vendor or copy uthash
sources.  It demonstrates depending on a curated header-only C
library through an ordinary scoped registry package.  The first
`cabin build` resolves the dependency against the registry index,
downloads the published package archive, verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/uthash-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved package pins):

```
uthash lookup: cabin = 42
uthash version: 2.4.0
```

## Offline

The first `cabin build` needs the registry.  Reads resolve through
the hosted registry by default, and while it is in private alpha
they are authenticated, so run `cabin login` first (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::uthash_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed recipes into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
