# uthash-usage

A consumer example for the curated
[`crates/cabin-port/ports/uthash/2.4.0/`](../../crates/cabin-port/ports/uthash/2.4.0/)
foundation port.  The program includes the header-only uthash macro
library, adds one entry to a hash table keyed by a string field,
looks it up, and prints the value plus the library version.

The upstream tarball ships a convenience `include -> src` symlink;
Cabin's port extraction skips symlink entries (nothing is
materialized for them) and the port's overlay points straight at
`src/`, so the archive prepares cleanly.

This is **not** itself a port and does not vendor or copy uthash
sources.  It demonstrates depending on a curated header-only C
foundation port from a normal Cabin package.  The first `cabin build`
downloads the upstream archive (URL and SHA-256 pinned by the port
recipe), verifies its checksum, extracts it under Cabin's cache, and
then builds normally; subsequent builds reuse the cache.

## Build and run

```sh
cd examples/uthash-usage
cabin build
cabin run
```

Expected output (the version is whatever the resolved port pins):

```
uthash lookup: cabin = 42
uthash version: 2.4.0
```

## Offline

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::uthash_usage_builds_and_runs`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
