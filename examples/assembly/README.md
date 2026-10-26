# assembly

Demonstrates assembly sources in a C target.  `src/main.c` calls `cabin_answer`, which is written
in assembly: GNU assembly (`src/answer.S`) on Linux and macOS, assembled by the GCC/Clang driver,
and MASM (`src/answer.asm`) on Windows, assembled by MSVC's x64 `ml64.exe`.  Each host picks its
file through a `[target.'cfg(...)'.sources]` table, and the target's `CABIN_ANSWER` define reaches
both assemblers.

## Build and run

```sh
cd examples/assembly
cabin run
```

Expected output:

```
Assembled answer: 42
```
