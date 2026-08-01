# picohttpparser-usage

A consumer example for the `cabin-ports/picohttpparser` registry
package, published from the curated
[`crates/cabin-port/ports/picohttpparser/2026.4.6/`](../../crates/cabin-port/ports/picohttpparser/2026.4.6/)
package directory.  The program links against picohttpparser, parses an HTTP
request from an in-memory buffer with `phr_parse_request`, and
prints the method, path, and header count.

picohttpparser publishes no tagged releases, so the port pins one
upstream commit by its immutable tarball URL and SHA-256, and the
published package's version is that commit's date spelled as SemVer
(`2026.4.6`).

This is **not** itself a port and does not vendor or copy
picohttpparser sources.  It demonstrates depending on a curated C
library through an ordinary scoped registry package.  The first
`cabin build` resolves the dependency against the registry index,
downloads the published package archive, verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/picohttpparser-usage
cabin build
cabin run
```

Expected output:

```
picohttpparser method: GET
picohttpparser path: /hello
picohttpparser headers: 1
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
(`crates/cabin/tests/cabin_examples.rs::picohttpparser_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed ports into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
