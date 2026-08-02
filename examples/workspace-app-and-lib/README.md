# workspace-app-and-lib

A Cabin workspace whose internal library carries an external dependency:

- `packages/greeter` - a `library` that depends on the `cabin-ports/fmt` registry package,
  published from the curated [`ports/fmt/`](../../ports/fmt)
  package directory, and formats its greeting with `fmt::format`.
- `packages/app` - an `executable` that depends on `greeter` through a path dependency and prints
  the greeting.

Where [`workspace-basic/`](../workspace-basic) shows workspace mechanics with path dependencies
alone, this example adds the mixed-dependency shape real projects have: `app` declares only the
internal `greeter` library, and the fmt package's headers and archive propagate to `app`'s compile
and link transitively through the `app -> greeter -> fmt` chain.

The workspace root is a *virtual* manifest (no `[package]`) with `members = ["packages/*"]`;
`default-members = ["packages/app"]` makes `cabin run` launch the app without `-p`.

The first workspace build resolves `"cabin-ports/fmt" = "=12.2.0"` against the registry index,
downloads the published archive, verifies its checksum, extracts it under Cabin's cache, and then
builds normally; subsequent builds reuse the cache.

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

The first build needs the registry.  Reads resolve through the hosted registry by default, and
verified packages download without an account or token - `cabin login` is only needed to publish (see
[`docs/remote-registry.md`](../../docs/remote-registry.md)).  Once the package is cached, later builds reuse the downloaded
archive without re-fetching it.  Resolving still consults the
registry index, so a fully offline build needs a local index; see
[`docs/vendoring-offline.md`](../../docs/vendoring-offline.md) for
the `cabin vendor` + `--offline --index-path` workflow.

The integration test for this example
(`crates/cabin/tests/cabin_examples.rs::workspace_app_and_lib_builds_and_runs`) is
`#[ignore = "requires external network"]`: it stages the committed ports into a local file
registry through the publisher pipeline and builds this example against it with `--index-path`.
