# unit-test-gtest

A `library` target unit-tested with GoogleTest through `cabin test`.  Where
[`library-with-tests/`](../library-with-tests) shows Cabin's framework-free test contract (a test
target passes when `main()` returns `0`), this example is the one to read for testing with a real
framework: a fixture (`TEST_F`), value assertions (`EXPECT_DOUBLE_EQ`), and exception assertions
(`EXPECT_THROW`) against a small statistics library, linked against the curated
[`googletest`](../../crates/cabin-port/ports/googletest) foundation port.

One command does everything:

```sh
cd examples/unit-test-gtest
cabin test
```

`cabin test` prepares the port (first run downloads the pinned archive into Cabin's cache), builds
the `stats` library and the `stats_gtest` test target, runs the produced binary, and folds the
result into its own summary.  GoogleTest's per-test output goes to the test binary's stdout;
Cabin reports the target as a whole:

```
running 1 test
test unit-test-gtest:stats_gtest ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Because only the `test` target references `googletest`, a plain `cabin build` compiles just the
`stats` library and touches nothing from the port.

The googletest port ships no `gtest_main`, so `tests/stats_gtest.cc` supplies its own `main` that
calls `::testing::InitGoogleTest` and `RUN_ALL_TESTS()`.

## Offline

If you have no network the first time, `cabin test` fails with a clear "cannot download port"
error.  Once the archive is already cached, subsequent runs work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::unit_test_gtest_runs_tests`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `github.com:443`, so a CI runner without
outbound network does not fail the suite.
