# hello-c

The smallest useful C project under Cabin. One `c_executable` target,
one `.c` source.

## Build and run

`cabin run` is reserved for `cpp_executable` targets, so for a
`c_executable` you build and then invoke the produced binary
directly. Cabin writes it under `build/<profile>/packages/<package>/`
(the default profile is `dev`).

```sh
cd examples/hello-c
cabin build
./build/dev/packages/hello-c/hello-c
```

Expected output:

```
Hello from Cabin (C)
```
