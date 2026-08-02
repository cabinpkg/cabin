# nlohmann-json-usage

A consumer example for the `cabin-ports/nlohmann_json` registry
package, published from the curated
[`ports/nlohmann_json/3.12.0/`](../../ports/nlohmann_json/3.12.0/)
package directory.  The program includes the header-only JSON for Modern C++
library, parses a small document, and prints two fields plus the
compiled-in library version.

This is **not** itself a port and does not vendor or copy any
sources.  It demonstrates depending on a curated header-only C++
library through an ordinary scoped registry package.  The first
`cabin build` resolves the dependency against the registry index,
downloads the published package archive, verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/nlohmann-json-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved package pins):

```
json parsed name: Cabin
json parsed answer: 42
nlohmann_json version: 3.12.0
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
(`crates/cabin/tests/cabin_examples.rs::nlohmann_json_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed ports into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
