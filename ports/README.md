# Cabin foundation ports

This directory holds **curated foundation ports**: Cabin packages that adapt important existing
C/C++ libraries - libraries that do not yet ship a native `cabin.toml` - to Cabin's build model.

Every `<name>/<version>/` directory is a single `cabin.toml` carrying the canonical scoped identity
(`cabin-ports/<name>`) and a complete `[package.upstream]` block, plus its `patches/` when it
declares any and a `tests/` consumer test when it has one.  The manifest and patches publish
verbatim: editing a byte of them - a comment included - changes the published archive.  `tests/`
is never published.  See [`docs/foundation-ports.md`](../docs/foundation-ports.md).

Ports reach consumers as published `cabin-ports/*` registry packages; nothing in this directory is
consumed from a user's manifest directly.  To publish one, the tool downloads the archive, verifies
its pinned checksum, safely extracts it, applies the declared `[[package.upstream.copy]]` steps and then
the declared `[package.upstream].patches` as byte-exact unified diffs.

### Placing prebuilt files with `[[package.upstream.copy]]`

Some libraries ship a build-time file under a name the compiler does not expect - for example,
libpng ships its configuration as `scripts/pnglibconf.h.prebuilt`, which its build normally copies
to `pnglibconf.h`.  A port declares that placement declaratively:

```toml
[[package.upstream.copy]]
from = "scripts/pnglibconf.h.prebuilt"
to = "pnglibconf.h"
```

Each step copies one already-present file in the extracted source to a second in-tree location.
Both paths are validated as relative paths inside the source tree, the copy runs after extraction
and before the manifest is written (so the committed `cabin.toml` always wins), and the source file
is covered by the archive's pinned SHA-256.  This is a **static file copy**, not a build script: it
cannot run commands, generate content, or read anything outside the extracted archive.

### Correcting sources with `[package.upstream].patches`

Small corrections that no copy step can express - build-system fixes, portability tweaks - are
declared as unified-diff files under the port's `patches/` directory:

```toml
[package.upstream]
# ... url / checksum / format / strip-prefix ...
patches = ["patches/0001-fix-msvc-build.patch"]
```

Patches apply in declared order, after every copy step, and application is byte-exact:
fixed `-p1` strip, no fuzz, no offset search, no newline normalization, text diffs only.  Like
the copy steps, this is declarative transformation, never a script; a patch that does not apply
fails the materialization.  See [`foundation-ports.md`](../docs/foundation-ports.md) for the
full rules.

Retiring a specific *version* of a foundation port removes `ports/<name>/<version>/`; retiring an
entire port removes the whole `ports/<name>/` directory.

## What ports are not

- They are **not Cabin's public registry**.
- They are **not a submission queue** for arbitrary C/C++ libraries; this directory is curated.
- They are **not** a mechanism for distributing pre-built binaries or compiled artifacts.
- They are **not** a workaround for missing build-script support - they only describe libraries
  whose source layout fits Cabin's existing target model (a list of sources plus include
  directories), optionally placing a prebuilt file with a static `[[package.upstream.copy]]` step
  or correcting one with a declared byte-exact patch.  Ports that need code generation or a configure run do not
  belong here until the build graph runs generated actions.

## Policy

- Sources must be pinned by URL and SHA-256.  Floating references (`latest`, branches, tag-only
  without integrity) are rejected.
- No upstream build-system invocation today.  Cabin does not run CMake, Autotools, Meson, Make, or
  upstream `configure` scripts.
- Patches under `patches/` (if any) should be limited to what is strictly required to make a port
  build through Cabin: build-system fixes and portability tweaks, never feature changes or
  divergence from upstream behavior.
- A `library` target that embodies a well-known native library claims its identity with
  [`links`](../docs/manifest.md#links) (`zlib` claims `"z"`), judged against upstream
  conventions.  Header-only ports claim nothing, and an alternative implementation with its own
  symbol namespace claims its own identity (`miniz` claims `"miniz"`, not `"z"`).
- A foundation port should be **retired** once its upstream project ships and maintains a native
  `cabin.toml`.

## Publishing ports as `cabin-ports/*` registry packages

`cargo port-publish` publishes every port here, verbatim, under the `cabin-ports` scope.  Its
`--dry-run` mode runs the full local preflight with no remote mutation:

```console
$ cargo build -p cabinpkg
$ cargo port-publish --dry-run
```

See [`foundation-ports.md`](../docs/foundation-ports.md#publishing-ports-as-registry-packages) for
the publish rules, [publish automation](../docs/foundation-ports.md#publish-automation), and which
corrections may ship as a [packaging revision](../docs/foundation-ports.md#packaging-revisions).
