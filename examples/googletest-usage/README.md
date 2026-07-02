# googletest-usage

A consumer example for the curated
[`crates/cabin-port/ports/googletest/1.17.0/`](../../crates/cabin-port/ports/googletest/1.17.0/)
foundation port.  The package has a small `calc` library and one
`test` target that links GoogleTest from the port.  (The port is a
normal `[dependencies]` entry: Cabin's dev path/port dependencies
are declaration-only today and never enter the package graph, so a
`test` target cannot link them.)

The port builds the GoogleTest library only (no `gtest_main`, no
GoogleMock), so the test source supplies its own two-line `main`
calling `InitGoogleTest` + `RUN_ALL_TESTS`.

This is **not** itself a port and does not vendor or copy GoogleTest
sources.  The first `cabin test` downloads the upstream archive (URL
and SHA-256 pinned by the port recipe), verifies its checksum,
extracts it under Cabin's cache, and then builds normally;
subsequent runs reuse the cache.

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

If you have no network the first time, the build fails with a clear
"cannot download port" error.  Once the archive is already cached,
subsequent runs work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::googletest_usage_runs_tests`)
skips cleanly when `CABIN_NET_OFFLINE` is set or when the host
cannot reach `github.com:443`, so a CI runner without outbound
network does not fail the suite.
