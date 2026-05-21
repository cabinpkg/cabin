# Cabin foundation ports

This directory holds **curated foundation ports**: Cabin recipes
that adapt important existing C/C++ libraries — libraries that
do not yet ship a native `cabin.toml` — to Cabin's build model.

A foundation port consists of:

- `port.toml` — pins a single upstream release archive by URL
  and SHA-256, optionally with a `strip_prefix` for the
  archive's root directory.
- `cabin.toml` — a Cabin overlay manifest that describes the
  upstream sources as ordinary Cabin C/C++ targets.

When a Cabin package declares a dependency of the form
`{ port = "path/to/port" }`, Cabin downloads the archive,
verifies the SHA-256, safely extracts it, copies the overlay
manifest into the extracted source tree, and treats the result
as a normal Cabin path dependency.

## What ports are not

- They are **not Cabin's public registry**.
- They are **not a submission queue** for arbitrary C/C++
  libraries; this directory is curated.
- They are **not** a mechanism for distributing pre-built
  binaries or compiled artifacts.
- They are **not** a workaround for missing build-script
  support — they only describe libraries whose source layout
  fits Cabin's existing target model (a list of sources plus
  include directories).

## Policy

- Sources must be pinned by URL and SHA-256. Floating
  references (`latest`, branches, tag-only without integrity)
  are rejected.
- No upstream build-system invocation. Cabin never runs CMake,
  Autotools, Meson, Make, or upstream `configure` scripts.
- Patches under `patches/` (if any) should be limited to
  what is strictly required to make a port build through Cabin.
- A foundation port should be **retired** once its upstream
  project ships and maintains a native `cabin.toml`.

## Available ports

- [`zlib/`](zlib/) — the zlib compression library, pinned to
  a single upstream release.
