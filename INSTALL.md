# Installing Cabin from Source

The recommended way to get Cabin is from [the docs site](https://cabinpkg.com/docs/installation/),
which covers the `cargo install cabinpkg` path (the installed command is `cabin`).  Building from
source is supported for users who need an unreleased revision or want to verify a build locally.

## Prerequisites

- A [Rust toolchain](https://www.rust-lang.org/tools/install) (1.95 or newer) on the stable channel.
- A C compiler and archiver for the build scripts of two dependencies that compile bundled C
  sources: `ring` (TLS, every platform) and, on Linux, `libdbus-sys` (the Secret Service credential
  store, built from vendored sources, so no system D-Bus headers or `pkg-config` are needed).  GCC
  or Clang with `ar` on Linux, the Xcode Command Line Tools on macOS, or the Visual Studio Build
  Tools on Windows (plus Clang on Windows on Arm, which `ring` requires there) - mostly the tools
  the Rust toolchain already needs for linking.
- `git`.

These build Cabin itself.  The C/C++ toolchains, Ninja, and the format / static-analysis helpers
that `cabin build` / `cabin fmt` / `cabin tidy` drive are separate runtime requirements, documented
in [Installation: Runtime Requirements](https://cabinpkg.com/docs/installation/).

## Build

```sh
git clone https://github.com/cabinpkg/cabin
cd cabin
cargo build --release
```

The built binary lands at `target/release/cabin`.  Copy it onto a directory on your `$PATH`, or run
it directly:

```sh
./target/release/cabin --version
```

## Updating

```sh
git pull
cargo build --release
```

`cargo build` reuses the incremental cache so subsequent builds are fast.
