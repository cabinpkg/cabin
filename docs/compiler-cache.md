# Compiler wrappers

Cabin can prefix every C and C++ compile command with an executable such as
`ccache`, `sccache`, or `icecc`. The wrapper sits in front of the selected
compiler:

```text
<compiler-wrapper> <actual-compiler> <compiler-args...>
```

Link and archive commands are never wrapped. `compile_commands.json` also
keeps the underlying compiler first so clangd and other IDE tooling can
recognize the command.

## Configuration

The workspace root manifest may select a wrapper in `[build]`:

```toml
[build]
compiler-wrapper = "ccache"
```

The value may be any executable name or path:

```toml
[build]
compiler-wrapper = "/opt/tools/sccache"
```

The value is one executable, not a shell command. Cabin trims surrounding
whitespace, rejects empty or whitespace-only values, and does not shell-split
the string. A bare name is searched on `PATH`; a path is probed directly.

`[build] compiler-wrapper` is workspace-root-only. Member and path-dependency
manifests cannot choose a different wrapper for the same build invocation.
Target-conditional wrapper selection is not supported.

Cabin config files use the same direct field:

```toml
[build]
compiler-wrapper = "sccache"
```

## Selection precedence

Cabin selects one wrapper for the build, from highest to lowest precedence:

1. `--compiler-wrapper <executable>` or `--no-compiler-wrapper`
2. `CABIN_COMPILER_WRAPPER`
3. merged config-file `[build] compiler-wrapper`
4. workspace-root manifest `[build] compiler-wrapper`
5. no wrapper

`none`, `off`, and `disabled` explicitly disable wrapping. An explicit disable
at a higher layer prevents lower layers from selecting a wrapper.

Examples:

```sh
cabin build --compiler-wrapper ccache
cabin build --compiler-wrapper /opt/tools/icecc
cabin build --no-compiler-wrapper
```

## Detection, metadata, and fingerprints

After resolving the executable, Cabin runs `<wrapper> --version`. The detected
identity is exposed under `toolchain.compiler_wrapper` in `cabin metadata`,
including the executable family, requested spec, selected source, and parsed
version when available.

Build commands treat a missing wrapper or failed version probe as an error.
`cabin metadata` is fail-soft: it reports a warning and continues without a
resolved wrapper block.

The build-configuration fingerprint includes the resolved wrapper kind, spec,
source, and detected version. Selecting, changing, or disabling a wrapper
therefore changes the fingerprint.

`cabin package` and the file registry preserve the manifest-declared
`[build] compiler-wrapper` value. CLI, environment, and local config selections
are local policy and are not written into package metadata.

## Wrapper-specific settings

Cabin only selects and invokes the wrapper executable. Configure wrapper
behavior through the wrapper's own files or environment variables, such as
`CCACHE_DIR`, `CCACHE_MAXSIZE`, and `SCCACHE_*`.
