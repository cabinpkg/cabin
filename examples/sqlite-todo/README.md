# sqlite-todo

A miniature todo-list app on the `cabin-ports/sqlite3` registry package (the amalgamation),
published from the curated
[`crates/cabin-port/ports/sqlite3/`](../../crates/cabin-port/ports/sqlite3) package
directory.  Where
[`sqlite3-usage/`](../sqlite3-usage) is the minimal consumption smoke test, this example walks the
shape of a real SQLite program from C: open a database, run DDL and DML through `sqlite3_exec`,
then iterate a `SELECT` with the prepare/step/finalize statement API.

The database lives in `:memory:`, so every run is deterministic and leaves no files behind.  To
persist between runs, open a file path instead of `:memory:`.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` resolves
the pinned dependency against the registry index, downloads the published archive, verifies its
checksum, extracts it under Cabin's cache, and then builds normally; subsequent builds reuse the
cache.

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

The first `cabin build` needs the registry.  Reads resolve through the hosted registry by default,
and verified packages download
without an account or token - `cabin login` is only needed to publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::sqlite_todo_builds_and_runs`) is
`#[ignore = "requires external network"]`: it stages the committed ports into a local file
registry through the publisher pipeline and builds this example against it with `--index-path`.
