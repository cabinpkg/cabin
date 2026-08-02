# libpng-usage

A consumer example for the `cabin-ports/libpng` registry package, published from the curated
[`ports/libpng/1.6.50/`](../../ports/libpng/1.6.50/)
package directory.
The program creates a libpng read struct, prints the libpng version, and prints the zlib version -
the latter reached only through libpng's own dependency on `cabin-ports/zlib`.

This is **not** itself a port and does not vendor or copy libpng or zlib sources.  It demonstrates
depending on a registry package that itself depends on another registry package.

## A transitive registry dependency

libpng depends on zlib.  The app declares only libpng:

```toml
[dependencies]
"cabin-ports/libpng" = "=1.6.50"
```

The published libpng package declares `"cabin-ports/zlib" = "^1.3"`, so the resolver pulls zlib in
transitively. zlib's headers and its compiled archive propagate through the libpng edge, which is
why `src/main.c` can `#include <zlib.h>` and call `zlibVersion()` even though the app never names
zlib.  The dependency tree shows the nesting:

```
$ cabin tree
libpng-usage v0.1.0 (workspace)
`-- cabin-ports/libpng v1.6.50 [normal] (registry, sha256:...)
    `-- cabin-ports/zlib v1.3.1 [normal] (registry, sha256:...)
```

## Build and run

```sh
cd examples/libpng-usage
cabin build
cabin run
```

Expected output (versions are whatever the resolved packages pin):

```
libpng version: 1.6.50
zlib version (via libpng edge): 1.3.1
```

## Prebuilt configuration, no configure step

libpng normally generates `pnglibconf.h` during its build.  Cabin never runs a port's upstream build
system, so the libpng package uses a declarative `[[package.upstream.copy]]` step to place the
upstream **prebuilt**
config (`scripts/pnglibconf.h.prebuilt`) at `pnglibconf.h`.  The hand-written SIMD optimizations
(ARM NEON, Intel SSE) are compiled out (`PNG_ARM_NEON_OPT=0`, `PNG_INTEL_SSE_OPT=0`) so the portable
source set links on every architecture.

## Caching and offline

Registry dependencies resolve through the hosted registry by default; verified packages download
without an account or token - `cabin login` is only needed to
publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  The first `cabin build` downloads both
the libpng and zlib package archives, verifies their checksums, extracts them under Cabin's cache,
and builds.  Subsequent builds reuse the cached archives without re-fetching them.  Resolving still
consults the registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for the `cabin vendor` +
`--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::libpng_usage_builds_and_runs`) runs only with `--ignored`
and needs outbound network: it stages the committed ports into a local file registry through the
publisher pipeline and builds against that with `--index-path`, downloading the pinned upstream
archives on the way.  The hermetic cache-lifecycle coverage lives in
`crates/cabin/tests/cli/registry_ports.rs`.
