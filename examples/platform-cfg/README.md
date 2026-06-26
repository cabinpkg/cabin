# platform-cfg

Demonstrates Cabin's `[target.'cfg(...)']` platform conditions.  The `cabin.toml` defines a
different preprocessor macro per platform - `CABIN_ON_WINDOWS` on Windows, `CABIN_ON_UNIX` on Linux
and macOS - and the single `src/main.cc` prints which one Cabin selected.  On Windows the define is
passed to MSVC as `/DCABIN_ON_WINDOWS`; elsewhere it is passed to GCC/Clang as `-DCABIN_ON_UNIX`.

## Build and run

```sh
cd examples/platform-cfg
cabin build
cabin run
```

Expected output on Linux and macOS:

```
Hello from Cabin on Unix
```

Expected output on Windows:

```
Hello from Cabin on Windows
```
