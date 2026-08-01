# cli-with-spdlog

A small command-line app that combines three `cabin-ports/*` registry packages in one binary:

- [`cabin-ports/cli11`](../../crates/cabin-port/ports/CLI11) parses `--name` / `--count` flags.
- [`cabin-ports/fmt`](../../crates/cabin-port/ports/fmt) formats the greeting lines.
- [`cabin-ports/spdlog`](../../crates/cabin-port/ports/spdlog) logs what the app is about to do.

Each one is an ordinary scoped registry package, published to the Cabin registry from the curated
port directory it links to by the repository tool `cabin-port-publish`.  A port still committed as
a recipe is also reachable as a bundled `port = true` dependency; one already migrated to a package
(CLI11 and {fmt} here) is registry-only.  See
[`docs/foundation-ports.md`](../../docs/foundation-ports.md).

The spdlog package is header-only and defaults to its **bundled** {fmt} copy.  Cabin propagates
include dirs across dependency edges but not defines, so the opt-in to the external fmt package
happens in this package's own manifest: `defines = ["SPDLOG_FMT_EXTERNAL"]` on the executable
target reaches every translation unit that includes spdlog's headers, and all three libraries end
up sharing the single fmt package.

This is **not** itself a port and does not vendor any sources.  The first `cabin build` resolves
the three pinned dependencies against the registry index, downloads the published archives,
verifies their checksums, extracts them under Cabin's cache, and then builds normally; subsequent
builds reuse the cache.

## Build and run

```sh
cd examples/cli-with-spdlog
cabin build
cabin run
```

Expected output (the log line carries a timestamp prefix):

```
[2026-07-02 12:34:56.789] [info] preparing 2 greeting(s) for Cabin
1/2: Hello, Cabin!
2/2: Hello, Cabin!
spdlog version: 1.17.0
fmt version (external): 120200
```

Pass flags through `cabin run` after `--`:

```sh
cabin run -- --name you --count 3
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
(`crates/cabin/tests/cabin_examples.rs::cli_with_spdlog_builds_and_runs`) is
`#[ignore = "requires external network"]`: it stages the committed ports into a local file
registry through the publisher pipeline and builds this example against it with `--index-path`.
