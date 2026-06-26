# hello-c

The smallest useful C project under Cabin.  One `executable` target
with a single `.c` source.  The compile driver is picked per source
(here: the C compiler, because `src/main.c` ends in `.c`).

## Build and run

`cabin run` builds and launches the package's `executable` target.
You can also build and invoke the produced binary directly; Cabin
writes it under `build/<profile>/packages/<package>/` (the default
profile is `dev`).

```sh
cd examples/hello-c
cabin run
# or:
cabin build
./build/dev/packages/hello-c/hello-c
```

Expected output:

```
Hello from Cabin (C)
```
