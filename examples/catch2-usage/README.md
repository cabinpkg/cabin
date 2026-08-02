# catch2-usage

A consumer example for the `cabin-ports/catch2` registry package,
published from the curated
[`ports/catch2/3.15.1/`](../../ports/catch2/3.15.1/)
package directory.  The package has a small `calc` library and one `test`
target whose source only defines `TEST_CASE`s - the package's
amalgamated translation unit supplies Catch2's default `main()`.

The package carries Catch2's upstream amalgamation
(`extras/catch_amalgamated.cpp` / `.hpp`), so tests include
`<catch_amalgamated.hpp>` rather than the split `<catch2/...>`
headers (those need a CMake-generated configuration header, which
the port never generates).  A consumer that wants its own entry
point enables the package's `custom-main` feature:

```toml
"cabin-ports/catch2" = { version = "=3.15.1", features = ["custom-main"] }
```

This is **not** itself a port and does not vendor or copy Catch2
sources.  The first `cabin test` resolves the dependency against the
registry index, downloads the published package archive, verifies
its checksum, extracts it under Cabin's cache, and then builds
normally; subsequent runs reuse the cache.

## Run the tests

```sh
cd examples/catch2-usage
cabin test
```

Expected output ends with:

```
test catch2-usage:calc_catch2 ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ...
```

## Offline

The first `cabin test` needs the registry.  Reads resolve through
the hosted registry by default, and verified packages download
without an account or token - `cabin login` is only needed to publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later runs reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline run needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::catch2_usage_runs_tests`)
is `#[ignore = "requires external network"]`: it stages the
committed ports into a local file registry through the publisher
pipeline and runs this example against it with `--index-path`.
