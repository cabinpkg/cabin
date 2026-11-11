# build-script-helper-lib

A C program whose build script is written in C++ and uses a helper library.

- `packages/tablegen` - a C++ `library` that computes the CRC-32 lookup table.
- `packages/app` - a C `executable`.  Its `[build-dependencies]` name `tablegen`, and its build
  script, `build.cpp`, links that library, writes the table as `crc32_table.c` into the directory
  `CABIN_OUT_DIR` names, and reports the file as a further source of the `app` target.

A build dependency is built for the machine the build runs on, and only the build script links it.
The `app` executable links no C++: it is compiled and linked as C.  See
[`docs/build-scripts.md`](../../docs/build-scripts.md) for what a build script is given and may
report.

## Build and run

```sh
cd examples/build-script-helper-lib
cabin run
```

Expected output:

```
crc32("cabin") = ce4ce919
```
