# json-cli

A miniature manifest-inspector built on the header-only `cabin-ports/nlohmann_json` registry
package, published from the curated
[`crates/cabin-port/ports/nlohmann_json/`](../../crates/cabin-port/ports/nlohmann_json) recipe.
Where [`nlohmann-json-usage/`](../nlohmann-json-usage) is the minimal consumption smoke test, this
example walks a realistic JSON round trip: parse a document, read typed values out of nested
objects and arrays, and serialize a derived summary back to JSON.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` resolves
the pinned dependency against the registry index, downloads the published archive, verifies its
checksum, extracts it under Cabin's cache, and then builds normally; subsequent builds reuse the
cache.

## Build and run

```sh
cd examples/json-cli
cabin build
cabin run
```

Expected output:

```
package: json-cli v0.1.0
dependency count: 3
  dep: fmt
  dep: spdlog
  dep: sqlite3
summary: {"deps":["fmt","spdlog","sqlite3"],"name":"json-cli"}
```

For the C equivalent of this use case, see [`cjson-usage/`](../cjson-usage), which consumes the
`cabin-ports/cjson` package from a `.c` source.

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
(`crates/cabin/tests/cabin_examples.rs::json_cli_builds_and_runs`) is
`#[ignore = "requires external network"]`: it stages the committed recipes into a local file
registry through the publisher pipeline and builds this example against it with `--index-path`.
