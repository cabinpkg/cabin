# sqlite-todo

A miniature todo-list app on the curated
[`sqlite3`](../../crates/cabin-port/ports/sqlite3) foundation port (the amalgamation).  Where
[`sqlite3-usage/`](../sqlite3-usage) is the minimal consumption smoke test, this example walks the
shape of a real SQLite program from C: open a database, run DDL and DML through `sqlite3_exec`,
then iterate a `SELECT` with the prepare/step/finalize statement API.

The database lives in `:memory:`, so every run is deterministic and leaves no files behind.  To
persist between runs, open a file path instead of `:memory:`.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` downloads
the upstream amalgamation (URL and SHA-256 pinned by the port recipe), verifies its checksum,
extracts it under Cabin's cache, and then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/sqlite-todo
cabin build
cabin run
```

Expected output:

```
todo list:
  [x] #1 write the manifest
  [ ] #2 add a lockfile
  [ ] #3 ship v0.1.0
open todos: 2
```

## Offline

If you have no network the first time, the build fails with a clear "cannot download port" error.
Once the archive is already cached, subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::sqlite_todo_builds_and_runs`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `www.sqlite.org:443`, so a CI runner
without outbound network does not fail the suite.
