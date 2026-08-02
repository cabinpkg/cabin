# sqlite3-usage

A consumer example for the `cabin-ports/sqlite3` registry package, published from the curated
[`ports/sqlite3/3.53.2/`](../../ports/sqlite3/3.53.2/)
package directory.
The program links against SQLite (the single-file amalgamation), opens an in-memory database, runs
a query, and prints the library version and thread-safety mode.

This is **not** itself a port and does not vendor or copy SQLite sources.  It demonstrates depending
on a published registry package from a normal Cabin package.  The first `cabin build` resolves
`"cabin-ports/sqlite3" = "=3.53.2"` against the registry index, downloads the published package
archive, verifies its checksum, extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/sqlite3-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved package pins):

```
sqlite version: 3.53.2
sqlite threadsafe: 1
sqlite query result: 42
```

## Threading mode is a feature

The package builds threadsafe (serialized) SQLite by default.  On Unix that needs
`-lpthread -ldl -lm`, which the package declares as propagating `link-libs` so this consumer links
them automatically.

To compile a single-threaded SQLite instead - dropping the threading layer via `SQLITE_THREADSAFE=0`
- enable the package's `single-threaded` feature on the dependency:

```toml
[dependencies]
"cabin-ports/sqlite3" = { version = "=3.53.2", features = ["single-threaded"] }
```

`sqlite3_threadsafe()` then reports `0`.

## Offline

Registry dependencies resolve through the hosted registry by default; verified packages download
without an account or token - `cabin login` is only needed to
publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::sqlite3_usage_builds_and_runs`) runs only with `--ignored`
and needs outbound network: it stages the committed ports into a local file registry through the
publisher pipeline and builds against that with `--index-path`, downloading the pinned upstream
archives on the way.
