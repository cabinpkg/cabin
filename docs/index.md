# Cabin Docs

**Cabin is a Cargo-inspired package manager and build system for C and C++.** It treats C/C++ as
first-class siblings: a single project can mix `.c` and `.cc` translation units, the C/C++ standards
stay separate, `CC` and `CXX` are kept independent, and the resolved build configuration gets a
deterministic fingerprint.

Cabin is *Cargo-inspired*, not *Cargo-compatible*: it borrows Cargo's vocabulary where the semantics
line up and deliberately diverges where C/C++ semantics demand it.  See
[Cargo-inspired interface](cargo-inspired-interface.md) for the full audit.

## Where to start

If you are new to Cabin, the following pages cover the most common surfaces:

- [`cabin new` and `cabin init`](new-and-init.md) - scaffold a new binary or library package.
- [`cabin.toml` reference](manifest.md) - the manifest schema.
- [Configuration files](config.md) - `.cabin/config.toml` precedence and discovery.
- [Targets](targets.md) - how `library`, `executable`, `test`, and friends are declared.
- [Dependency kinds](dependency-kinds.md) - the three dependency kinds and how they activate.
- [Build profiles](profiles.md) - `dev`, `release`, and user- declared profiles.
- [Environment variables](environment-variables.md) - the `CABIN_*` read, run, and test env surface.

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
cxx-standard = "c++17"

[target.hello]
type = "executable"
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
cxx-standard = "c++17"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
```

```toml
# lib/cabin.toml
[package]
name = "lib"
version = "0.1.0"
cxx-standard = "c++17"

[target.lib]
type = "library"
sources = ["src/lib.cc"]
```
