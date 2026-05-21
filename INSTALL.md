# Installation

[![Packaging status](https://repology.org/badge/vertical-allrepos/cabin-cpp-package-manager.svg)](https://repology.org/project/cabin-cpp-package-manager/versions)

The recommended way to get Cabin is from
[the docs site](https://docs.cabinpkg.com/installation). Building from
source is supported for users who need an unreleased revision or want
to verify a build locally.

If you intend to contribute back, read [CONTRIBUTING.md](CONTRIBUTING.md)
instead — it covers the development workflow on top of the steps here.

## Runtime Prerequisites

The Cabin binary itself has no C / C++ build-time dependency. The
C / C++ toolchains, Ninja, and the format / static-analysis helpers
are runtime requirements for `cabin build` / `cabin fmt` /
`cabin tidy` and are documented in
[Installation: Runtime Requirements](https://docs.cabinpkg.com/installation).

## Package Managers

### Linux

```console
# Arch Linux
paru -S cabin

# Nix/NixOS
nix-env -i cabinpkg

# Termux
pkg install cabin
```

### macOS

```console
# Homebrew
brew install cabin
```

### Windows

Unsupported, see [architecture](docs/architecture.md).

You may build from source with instructions found below.

## Manual Download

### Github Releases

You can download prebuilt binaries from the [github releases section](https://github.com/cabinpkg/cabin/releases).

## Building from Source

### Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install) on the
  stable channel
- `git`

### Cargo

You can build and install from source with Cargo
```console
cargo install --git https://github.com/cabinpkg/cabin.git cabin-cli
```

Cabin lands at `~/.cargo/bin/cabin`. Make sure it's in your `$PATH`.

### Source Code

```sh
git clone https://github.com/cabinpkg/cabin
cd cabin
cargo build --release
```

The built binary lands at `target/release/cabin`. Copy it onto a
directory on your `$PATH`, or run it directly:

```sh
./target/release/cabin --version
```

Updating is done with:

```sh
git pull
cargo build --release
```

`cargo build` reuses the incremental cache so subsequent builds are
fast.
