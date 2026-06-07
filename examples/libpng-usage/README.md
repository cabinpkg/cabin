# libpng-usage

A consumer example for the curated
[`crates/cabin-port/ports/libpng/1.6.50/`](../../crates/cabin-port/ports/libpng/1.6.50/)
foundation port. The program creates a libpng read struct, prints the
libpng version, and prints the zlib version — the latter reached only
through libpng's own dependency on the bundled zlib port.

This is **not** itself a port and does not vendor or copy libpng or
zlib sources. It demonstrates depending on a curated foundation port
that itself depends on another foundation port.

## A transitive port dependency

libpng depends on zlib. The app declares only libpng:

```toml
[dependencies]
libpng = { port = true, version = "^1.6" }
```

The bundled libpng overlay declares `zlib = { port = true }`, so port
discovery pulls zlib in transitively. zlib's headers and its compiled
archive propagate through the libpng edge, which is why `src/main.c`
can `#include <zlib.h>` and call `zlibVersion()` even though the app
never names zlib. The dependency tree shows the nesting:

```
$ cabin tree
libpng-usage v0.1.0 (workspace)
└── libpng v1.6.50 [normal] (port)
    └── zlib v1.3.1 [normal] (port)
```

## Build and run

```sh
cd examples/libpng-usage
cabin build
cabin run
```

Expected output (versions are whatever the resolved ports pin):

```
libpng version: 1.6.50
zlib version (via libpng port edge): 1.3.1
```

## Prebuilt configuration, no configure step

libpng normally generates `pnglibconf.h` during its build. Cabin never
runs a port's upstream build system, so the libpng recipe uses a
declarative `[[copy]]` step to place the upstream **prebuilt** config
(`scripts/pnglibconf.h.prebuilt`) at `pnglibconf.h`. The hand-written
SIMD optimizations (ARM NEON, Intel SSE) are compiled out
(`PNG_ARM_NEON_OPT=0`, `PNG_INTEL_SSE_OPT=0`) so the portable source
set links on every architecture.

## Caching and offline

The first `cabin build` downloads both the libpng and zlib archives
(URLs and SHA-256s pinned by the port recipes), verifies their
checksums, extracts them under Cabin's cache, and builds. Subsequent
builds reuse the cache and work offline (`cabin build --offline`). On a
pristine cache, `cabin build --frozen` fails with a clear "cannot
prepare port" error rather than downloading.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::libpng_usage_cache_lifecycle_builds_and_runs`)
walks that whole cache lifecycle and skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach
`downloads.sourceforge.net:443`, so a CI runner without outbound
network does not fail the suite.
