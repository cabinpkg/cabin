# workspace-basic

A small Cabin workspace with two members:

- `packages/util` — a `cpp_library` exporting `util::doubled(int)`.
- `packages/cli` — a `cpp_executable` that depends on `util` through a
  path dependency and prints `doubled(21)`.

The workspace root is a *virtual* manifest (no `[package]`) and lists
its members through a glob: `members = ["packages/*"]`.
`default-members = ["packages/cli"]` controls which member is selected
when you run `cabin build` with no `--workspace` / `-p` flag.

## Build and run

```sh
cd examples/workspace-basic

# Build every member (glob discovery)
cabin build --workspace

# Build a single member by name
cabin build -p cli

# Run the cli member explicitly
cabin run -p cli
```

Expected output from `cabin run -p cli`:

```
doubled(21) = 42
```
