# stb-usage

A consumer example for the `cabin-ports/stb` registry package,
published from the curated
[`crates/cabin-port/ports/stb/2026.4.15/`](../../crates/cabin-port/ports/stb/2026.4.15/)
recipe.  The program includes `stb_sprintf.h` with
`STB_SPRINTF_IMPLEMENTATION` defined - the stb single-file pattern,
where exactly one translation unit hosts the function bodies - and
prints a formatted string.

stb publishes no tagged releases, so the recipe pins one upstream
commit by its immutable tarball URL and SHA-256, and the published
package's version is that commit's date spelled as SemVer
(`2026.4.15`).

This is **not** itself a port and does not vendor or copy stb
sources.  It demonstrates depending on a curated header-only C
library through an ordinary scoped registry package.  The first
`cabin build` resolves the dependency against the registry index,
downloads the published package archive, verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent builds reuse the cache.

## Build and run

```sh
cd examples/stb-usage
cabin build
cabin run
```

Expected output:

```
stb_sprintf formatted: Cabin scores 42
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
(`crates/cabin/tests/cabin_examples.rs::stb_usage_builds_and_runs`)
is `#[ignore = "requires external network"]`: it stages the
committed recipes into a local file registry through the publisher
pipeline and builds this example against it with `--index-path`.
