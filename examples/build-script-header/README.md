# build-script-header

A C package whose build script generates a header.  `build.c`, in the package's root, is the build
script: Cabin compiles it for the machine the build runs on and runs it before compiling the
package.  The script writes `build_info.h` into the directory `CABIN_OUT_DIR` names and reports one
line through the file `CABIN_RESULT_FILE` names, which adds that directory to the target's include
directories.  `src/main.c` includes the generated header.

The package is C only, so the build needs no C++ compiler.  See
[`docs/build-scripts.md`](../../docs/build-scripts.md) for what a build script is given and may
report.

## Build and run

```sh
cd examples/build-script-header
cabin run
```

Expected output on Linux (the last word names the platform the build is for):

```
built with the dev profile for linux
```
