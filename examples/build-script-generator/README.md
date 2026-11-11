# build-script-generator

A C program whose build script runs a code generator that the package declares as a build
dependency.

- `packages/embed` - an `executable` that writes a file's bytes as a C array.
- `packages/app` - an `executable`.  Its `[build-dependencies]` name `embed`, and its build script,
  `build.c`, runs that program on `assets/banner.txt` to generate `banner.h` in the directory
  `CABIN_OUT_DIR` names.

Cabin builds `embed` for the machine the build runs on and gives the script its path in
`CABIN_BIN_EMBED`.  The script runs exactly that file: a program named `embed` on `PATH` plays no
part.  The two `rerun-if-changed` lines it reports make it run again when `build.c` or the banner
changes; a newly built `embed` reruns it too.  See
[`docs/build-scripts.md`](../../docs/build-scripts.md) for what a build script is given and may
report.

## Build and run

```sh
cd examples/build-script-generator
cabin run
```

Expected output:

```
Hello from an embedded file
```
