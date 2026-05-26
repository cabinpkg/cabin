# Cabin Docs

**Cabin is a Cargo-inspired package manager and build system for C
and C++.** It treats C and C++ as first-class siblings: a single
project can mix `.c` and `.cc` translation units, the C and C++
standards stay separate, `CC` and `CXX` are kept independent, and
the resolved build configuration gets a deterministic fingerprint.

Cabin is *Cargo-inspired*, not *Cargo-compatible*: it borrows
Cargo's vocabulary where the semantics line up and deliberately
diverges where C/C++ semantics demand it. See
[Cargo-inspired interface](cargo-inspired-interface.md) for the
full audit.

## Where to start

If you are new to Cabin, the following pages cover the most
common surfaces:

- [`cabin new` and `cabin init`](new-and-init.md) — scaffold a
  new binary or library package.
- [`cabin.toml` reference](manifest.md) — the manifest schema.
- [Configuration files](config.md) — `.cabin/config.toml`
  precedence and discovery.
- [Targets](targets.md) — how `cpp_library`, `cpp_executable`,
  `cpp_test`, and friends are declared.
- [Dependency kinds](dependency-kinds.md) — the two dependency
  kinds and how they activate.
- [Build profiles](profiles.md) — `dev`, `release`, and user-
  declared profiles.
- [Environment variables](environment-variables.md) — the
  `CABIN_*` read, run, and test env surface.

## Reference by topic

### Manifest and configuration

- [`cabin.toml` reference](manifest.md)
- [Configuration files](config.md)
- [Environment variables](environment-variables.md)
- [Build profiles](profiles.md)
- [Toolchains and conditional build flags](toolchains.md)

### Targets and building

- [Targets](targets.md)
- [Compiler-cache wrappers](compiler-cache.md)
- [Testing with `cabin test`](testing.md)

### Dependencies

- [Dependency kinds](dependency-kinds.md)
- [Target / platform-specific dependencies](target-dependencies.md)
- [Features](features.md)
- [`cabin.lock` reference](lockfile.md)
- [Patch, override, and source replacement](patch-overrides.md)
- [Vendoring and offline mode](vendoring-offline.md)
- [Source artifacts](artifacts.md)

### Workspaces and observability

- [Workspaces](workspaces.md)
- [Metadata, tree, and explain](metadata-tree-explain.md)

### Distribution and registry interface

- [Package archive and canonical metadata](package-format.md)
- [Local JSON package index](package-index.md)
- [CLI distribution artifacts](distribution.md)

## Quickstart

```sh
# 1. Create a fresh single-package project.
cabin new hello
cd hello

# 2. Build it.
cabin build

# 3. Run it. Forward args after `--`.
cabin run -- world

# 4. Inspect the resolved state.
cabin metadata
cabin tree
cabin explain package hello
```

A minimal `cabin.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]
```

A two-package workspace:

```toml
# cabin.toml at the workspace root
[workspace]
members = ["app", "lib"]
```

```toml
# app/cabin.toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
```

```toml
# lib/cabin.toml
[package]
name = "lib"
version = "0.1.0"

[target.lib]
type = "cpp_library"
sources = ["src/lib.cc"]
```
