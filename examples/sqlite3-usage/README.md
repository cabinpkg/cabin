# sqlite3-usage

A consumer example for the curated
[`crates/cabin-port/ports/sqlite3/3.53.2/`](../../crates/cabin-port/ports/sqlite3/3.53.2/)
foundation port. The program links against SQLite (the single-file
amalgamation), opens an in-memory database, runs a query, and prints
the library version and thread-safety mode.

This is **not** itself a port and does not vendor or copy SQLite
sources. It demonstrates depending on a curated foundation port from
a normal Cabin package. The first `cabin build` downloads the
upstream amalgamation archive (URL and SHA-256 pinned by the port
recipe) from `sqlite.org`, verifies its checksum, extracts it under
Cabin's cache, and then builds normally; subsequent builds reuse the
cache.

## Build and run

```sh
cd examples/sqlite3-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
sqlite version: 3.53.2
sqlite threadsafe: 1
sqlite query result: 42
```

## Threading mode is a feature

The port builds threadsafe (serialized) SQLite by default. On Unix
that needs `-lpthread -ldl -lm`, which the port declares as
propagating `link-libs` so this consumer links them automatically.

To compile a single-threaded SQLite instead — dropping the threading
layer via `SQLITE_THREADSAFE=0` — enable the port's `single-threaded`
feature on the dependency:

```toml
[dependencies]
sqlite3 = { port = true, version = "^3", features = ["single-threaded"] }
```

`sqlite3_threadsafe()` then reports `0`.

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error. Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::sqlite3_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host cannot
reach `www.sqlite.org:443`, so a CI runner without outbound network
does not fail the suite.
