# googletest-usage

A consumer example for the `cabin-ports/googletest` registry
package, published from the curated
[`crates/cabin-port/ports/googletest/1.17.0/`](../../crates/cabin-port/ports/googletest/1.17.0/)
recipe.  The package has a small `calc` library and one `test`
target that links GoogleTest.  The dependency is a
`[dev-dependencies]` entry: `cabin test` activates dev dependencies
for the selected packages, so the `test` target links it while
`cabin build` ignores it entirely.

The package builds the GoogleTest library only (no `gtest_main`, no
GoogleMock), so the test source supplies its own two-line `main`
calling `InitGoogleTest` + `RUN_ALL_TESTS`.

This is **not** itself a port and does not vendor or copy GoogleTest
sources.  The first `cabin test` resolves the dependency against the
registry index, downloads the published package archive, verifies
its checksum, extracts it under Cabin's cache, and then builds
normally; subsequent runs reuse the cache.

## Run the tests

```sh
cd examples/googletest-usage
cabin test
```

Expected output ends with:

```
test googletest-usage:calc_gtest ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ...
```

## Offline

The first `cabin test` needs the registry.  Reads resolve through
the hosted registry by default, and while it is in private alpha
they are authenticated, so run `cabin login` first (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later runs reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline run needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::googletest_usage_runs_tests`)
is `#[ignore = "requires external network"]`: it stages the
committed recipes into a local file registry through the publisher
pipeline and runs this example against it with `--index-path`.
