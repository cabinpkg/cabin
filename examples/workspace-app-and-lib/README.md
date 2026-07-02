# workspace-app-and-lib

A Cabin workspace whose internal library carries an external dependency:

- `packages/greeter` - a `library` that depends on the curated
  [`fmt`](../../crates/cabin-port/ports/fmt) foundation port and formats its greeting with
  `fmt::format`.
- `packages/app` - an `executable` that depends on `greeter` through a path dependency and prints
  the greeting.

Where [`workspace-basic/`](../workspace-basic) shows workspace mechanics with path dependencies
alone, this example adds the mixed-dependency shape real projects have: `app` declares only the
internal `greeter` library, and the fmt port's headers and archive propagate to `app`'s compile
and link transitively through the `app -> greeter -> fmt` chain.

The workspace root is a *virtual* manifest (no `[package]`) with `members = ["packages/*"]`;
`default-members = ["packages/app"]` makes `cabin run` launch the app without `-p`.

The first workspace build downloads the fmt archive (URL and SHA-256 pinned by the port recipe),
verifies its checksum, extracts it under Cabin's cache, and then builds normally; subsequent
builds reuse the cache.

## Build and run

```sh
cd examples/workspace-app-and-lib

# Build every member
cabin build --workspace

# Run the app (selected by default-members)
cabin run
```

Expected output:

```
Hello, Cabin! (formatted by fmt 120200)
```

## Offline

If you have no network the first time, the build fails with a clear "cannot download port" error.
Once the archive is already cached, subsequent builds work offline.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::workspace_app_and_lib_builds_and_runs`) skips cleanly when
`CABIN_NET_OFFLINE` is set or when the host cannot reach `github.com:443`, so a CI runner without
outbound network does not fail the suite.
