# Installing Cabin from Source

The recommended way to get Cabin is from
[the docs site](https://docs.cabinpkg.com/installation). Building from
source is supported for users who need an unreleased revision or want
to verify a build locally.

If you intend to contribute back, read [CONTRIBUTING.md](CONTRIBUTING.md)
instead — it covers the development workflow on top of the steps here.

## Prerequisites

- A [Rust toolchain](https://www.rust-lang.org/tools/install) on the
  stable channel.
- `git`.

The Cabin binary itself has no C / C++ build-time dependency. The
C / C++ toolchains, Ninja, and the format / static-analysis helpers
are runtime requirements for `cabin build` / `cabin fmt` /
`cabin tidy` and are documented in
[Installation: Runtime Requirements](https://docs.cabinpkg.com/installation).

## Build

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

## Updating

```sh
git pull
cargo build --release
```

`cargo build` reuses the incremental cache so subsequent builds are
fast.
