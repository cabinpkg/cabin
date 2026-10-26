# platform-cfg

Demonstrates Cabin's `[target.'cfg(...)']` platform conditions.  The `cabin.toml` defines a
different preprocessor macro per platform - `CABIN_ON_WINDOWS` on Windows, `CABIN_ON_UNIX` on Linux
and macOS - and `src/main.cc` prints which one Cabin selected.  On Windows the define is passed to
MSVC as `/DCABIN_ON_WINDOWS`; elsewhere it is passed to GCC/Clang as `-DCABIN_ON_UNIX`.

It also selects a C source per platform with `[target.'cfg(...)'.sources]`: Windows compiles
`src/platform_windows.c` and Linux and macOS compile `src/platform_unix.c`.  Both files define the
function `main.cc` calls to print which one was compiled, so only one of them can be in the build.

## Build and run

```sh
cd examples/platform-cfg
cabin build
cabin run
```

Expected output on Linux and macOS:

```
Hello from Cabin on Unix
Compiled src/platform_unix.c
```

Expected output on Windows:

```
Hello from Cabin on Windows
Compiled src/platform_windows.c
```
