# library-and-app

A single Cabin package with a library target consumed by an executable
target.  Demonstrates `include-dirs` propagation: because `greet` is a
`library` with `include-dirs = ["include"]`, any target that depends
on `greet` automatically gets `include/` on its include path.

## Build and run

```sh
cd examples/library-and-app
cabin build
cabin run
```

Expected output:

```
Hello, Cabin!
```
