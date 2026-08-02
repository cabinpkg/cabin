# spdlog-usage

A consumer example for the `cabin-ports/spdlog` registry package,
published from the curated
[`ports/spdlog/1.17.0/`](../../ports/spdlog/1.17.0/)
package directory.  The program uses spdlog in its upstream-default
header-only form, logs one message through `spdlog::info`, and
prints the compiled-in spdlog version.

This is **not** itself a port and does not vendor or copy spdlog
sources.  It demonstrates depending on a published header-only C++
registry package from a normal Cabin package.  The first
`cabin build` resolves `"cabin-ports/spdlog" = "=1.17.0"` against
the registry index, downloads the published package archive,
verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/spdlog-usage
cabin build
cabin run
```

Expected output (the log line carries a timestamp prefix; the version
is whatever the resolved package pins):

```
[2026-07-02 12:34:56.789] [info] Hello from spdlog!
spdlog version: 1.17.0
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
(`crates/cabin/tests/cabin_examples.rs::spdlog_usage_builds_and_runs`)
runs only with `--ignored` and needs outbound network: it stages
the committed ports into a local file registry through the
publisher pipeline and builds against that with `--index-path`,
downloading the pinned upstream archives on the way.
