# Vendoring and offline mode

`cabin vendor` materializes the external versioned dependencies
the selected packages need into a deterministic local
file-registry directory. The output is just an ordinary Cabin
file registry — the same shape `cabin publish --registry-dir`
writes — so every existing read-path command knows how to
consume it without any new protocol.

The core offline-consumption workflow is:

```sh
cabin vendor                                   # populate ./vendor
cabin build  --offline --index-path ./vendor   # build with no network
cabin fetch  --offline --index-path ./vendor   # populate the cache offline
```

`cabin test --offline --index-path ./vendor` can use the
vendored registry only when the selected tests do not require
additional registry packages from `[dev-dependencies]`; the
vendor sweep follows the same selection policy as `cabin build`
and does not include dev-only dependencies.

## What `cabin vendor` produces

The default vendor directory is `vendor/` next to the project
manifest. Pass `--vendor-dir <path>` to override it.

The directory layout is the existing Cabin file registry:

```
vendor/
  config.json                                 # {"schema":1, "kind":"file-registry", ...}
  packages/<name>.json                        # per-package version index
  artifacts/<name>/<name>-<version>.tar.gz    # verified source archives
  cabin-vendor.json                           # deterministic summary of this run
```

`cabin-vendor.json` records the `(name, version, checksum,
artifact)` set this invocation wrote, in stable
`(name, version)` order. Re-running `cabin vendor` against the
same inputs produces a byte-identical summary.

Every archive is re-verified against the checksum recorded in
the source index before the byte stream is written; a checksum
mismatch surfaces as an explicit error and never overwrites the
destination.

## Selection

`cabin vendor` accepts the same selection bundle as
`cabin build` / `cabin fetch`:

```sh
cabin vendor --workspace
cabin vendor -p app
cabin vendor --exclude lldb
cabin vendor --features simd,ssl
cabin vendor --no-default-features
cabin vendor --no-patches
```

Only the packages reachable from the resolved selection are
vendored. A workspace member you did not pick is ignored.
`cabin vendor` matches the `cabin build` policy: dev
dependencies are *not* vendored unless they are in the
selection's build closure.

## Locked, frozen, and offline semantics

| Flag         | Lockfile       | Artifact cache | Vendor directory          | Network / HTTP index source |
| ------------ | -------------- | -------------- | ------------------------- | --------------------------- |
| (default)    | written        | populated      | written                   | not used; HTTP index rejected |
| `--locked`   | required, kept | populated      | written                   | not used; HTTP index rejected |
| `--frozen`   | required, kept | not populated  | written (explicit output) | not used; HTTP index rejected |
| `--offline`  | written        | populated      | written                   | **forbidden** |

`cabin vendor` requires a local file-registry index
(`--index-path` or `[registry] index-path`) so the per-package
metadata it copies into the vendor directory is byte-stable. An
HTTP index URL supplied by a `[registry] index-url` config
setting — or one reached via `[source-replacement]` — is
rejected before any network attempt. Already-cached artifacts
and local file-registry indexes are unaffected.

`cabin vendor --frozen` is the documented "rebuild a vendor
directory from the existing artifact cache without touching the
lockfile or refreshing the cache" flow. The vendor directory is
the *explicit user-requested output* of the command, so it is
still written under `--frozen`; the lockfile and the artifact
cache are not.

`--offline` and `--frozen` compose: `cabin vendor --offline
--frozen` succeeds when the closure is fully cached and the
lockfile is current, and fails with an actionable diagnostic
otherwise.

## Patches and source replacement

`cabin vendor` honors every patch / override mechanism:

- **Manifest `[patch]`** entries point at a local working copy.
  Patched packages do not flow through the resolver and are
  therefore not vendored — the vendor directory is for the
  registry-supplied versions only. Consumers of the vendor
  directory still see the patched local copy because the
  manifest's `[patch]` table is read from the consuming
  project, not from the vendor directory.
- **Config `[patch]` / `[source-replacement]`** is honored
  identically.
- **`--no-patches`** disables the entire local-policy layer for
  the vendor invocation — the resolver picks the registry
  versions and Cabin vendors those.

Local path dependencies (`dep = { path = "../local" }`) are
**not** copied into the vendor directory. They remain local
references; the consuming project must continue to provide them
on disk. `cabin vendor` does not invent a packaging path for
local sources because publishing them would require a different
identity contract (no checksum / no index entry).

## C / C++ first-class support

Vendored builds preserve every C/C++ contract documented in
`docs/toolchains.md` and `docs/targets.md`:

- CC and CXX remain separate;
- `cflags` / `cxxflags` / `ldflags` keep their argv-space
  separation;
- C and C++ standard flags do not leak across language slots;
- the link driver is selected per target by the planner
  regardless of how the dependency's archive was sourced;
- public include directories propagate from a vendored
  dependency to its consumers exactly as they would from a
  registry consumer;
- system dependency declarations are not vendored; later build /
  test / metadata commands still probe active `system = true`
  entries through `pkg-config`.

Because vendoring re-uses the existing artifact pipeline, every
fingerprint input documented in `docs/toolchains.md` continues
to participate in `BuildConfiguration::fingerprint` for vendor
builds.

## Scope

`cabin vendor` operates on the registry-package closure of the
resolved selection. Local path dependencies are not copied into
the vendor directory, and the command mirrors `cabin build`'s
closure policy (dev dependencies are not swept).

## Direct `ninja` invocation

`cabin build --offline --index-path ./vendor` regenerates
`build.ninja` and `compile_commands.json` from the vendored
inputs every time. Direct `ninja -C <build-dir>` invocations
still work for incremental rebuilds (Cabin does not interfere
with that flow), but Ninja does not re-read `cabin.toml`,
config files, or environment changes. Run `cabin build` after
manifest / config / toolchain edits so the build configuration
fingerprint and the generated commands reflect the new inputs.

## Environment variables

`cabin vendor` shares the fetch / index / artifact-cache /
offline environment policy used by the dependency-fetching
pipeline. It does not resolve a compiler toolchain or build
anything, so `CC`, `CXX`, `AR`, `CFLAGS`, `CXXFLAGS`,
`LDFLAGS`, and `LD` are not consumed by `cabin vendor` itself.
Subsequent build commands that use the vendored index still
honor their normal toolchain and profile environment policy.

## Troubleshooting

| Symptom                                                                         | Likely cause                                                               | Fix                                                                                       |
| ------------------------------------------------------------------------------- | -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `--offline forbids network access, but the resolved index source is the URL …` | A `[registry] index-url` config setting is active.                         | Pass `--index-path <vendor-dir>` or remove the URL setting from the active config.        |
| `cabin vendor requires a local --index-path source …`                          | The resolved index source is a URL (config or `[source-replacement]`).      | Switch to a local `--index-path` source or adjust the offending config entry.             |
| `vendor directory already contains <path> with checksum sha256:… which does not match …` | A previous run left a stale archive on disk.                               | Delete the offending file (or the whole `vendor/artifacts/<name>/` subdir) and re-run.    |
| `checksum mismatch while vendoring …`                                           | The artifact in the cache no longer matches the index's recorded checksum. | Clear the cache (`--cache-dir <new-dir>`) or refresh the index, then re-run `cabin vendor`. |
| `vendoring requires the source index to expose packages/<name>.json at …`       | The local `--index-path` is missing per-package metadata.                  | Re-publish the package with `cabin publish --registry-dir <dir>` so the file appears.       |
