# inih-usage

A consumer example for the `cabin-ports/inih` registry package,
published from the curated
[`ports/inih/62.0.0/`](../../ports/inih/62.0.0/)
package directory.  The program links against the inih C core, parses an
in-memory INI document with `ini_parse_string`, and prints the two
values its handler captured.

Upstream tags releases as `r62`; the port spells that as the
SemVer `62.0.0`, which is the version the package is published
under.  The package builds the C core (`ini.c`) only - the optional
C++ `INIReader` under `cpp/` is not part of it.

This is **not** itself a port and does not vendor or copy inih
sources.  It demonstrates depending on a curated C library through
an ordinary scoped registry package.  The first `cabin build`
resolves the dependency against the registry index, downloads the
published package archive, verifies its checksum, extracts it under
Cabin's cache, and then builds normally; subsequent builds reuse the
cache.

## Build and run

```sh
cd examples/inih-usage
cabin build
cabin run
```

Expected output:

```
inih parsed name: Cabin
inih parsed port: 8080
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
(`crates/cabin/tests/cabin_examples.rs::inih_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed ports into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
