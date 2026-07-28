# unit-test-gtest

A `library` target unit-tested with GoogleTest through `cabin test`.  Where
[`library-with-tests/`](../library-with-tests) shows Cabin's framework-free test contract (a test
target passes when `main()` returns `0`), this example is the one to read for testing with a real
framework: a fixture (`TEST_F`), value assertions (`EXPECT_DOUBLE_EQ`), and exception assertions
(`EXPECT_THROW`) against a small statistics library, linked against the `cabin-ports/googletest`
registry package, published from the curated
[`crates/cabin-port/ports/googletest/`](../../crates/cabin-port/ports/googletest) recipe.

One command does everything:

```sh
cd examples/unit-test-gtest
cabin test
```

`cabin test` resolves the dependency (the first run downloads the published archive into Cabin's
cache), builds the `stats` library and the `stats_gtest` test target, runs the produced binary,
and folds the result into its own summary.  GoogleTest's per-test output goes to the test binary's
stdout; Cabin reports the target as a whole:

```
running 1 test
test unit-test-gtest:stats_gtest ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Because `"cabin-ports/googletest" = "=1.17.0"` is a `[dev-dependencies]` entry, a plain
`cabin build` compiles just the `stats` library and touches nothing from the package - only
`cabin test` activates dev dependencies as real graph edges.

The googletest package ships no `gtest_main`, so `tests/stats_gtest.cc` supplies its own `main`
that calls `::testing::InitGoogleTest` and `RUN_ALL_TESTS()`.

## Offline

The first `cabin test` needs the registry.  Reads resolve through the hosted registry by default,
and while it is in private alpha they are authenticated, so run `cabin login` first (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later runs reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline run needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::unit_test_gtest_runs_tests`) is
`#[ignore = "requires external network"]`: it stages the committed recipes into a local file
registry through the publisher pipeline and runs this example against it with `--index-path`.
