# simd-dispatch

Demonstrates per-source instruction sets.  `src/max_sse41.c` uses SSE4.1's `_mm_max_epi32`, so its
target's `instruction-sets` table compiles that one source with `-msse4.1` (GCC, Clang, `clang-cl`;
`cl.exe` needs no flag).  `src/main.c` stays on the baseline: on x86 it checks the CPU at run time
and calls the SSE4.1 variant only when the CPU has it.  Elsewhere the x86-only source is not built,
through a `[target.'cfg(...)'.sources]` table, and the generic loop runs.

## Build and run

```sh
cd examples/simd-dispatch
cabin run
```

Expected output on an x86 CPU with SSE4.1:

```
max = 42 (sse4.1)
```

and on any other host:

```
max = 42 (generic)
```
