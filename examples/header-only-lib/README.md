# header-only-lib

A package that **authors** a header-only library as Cabin's dedicated `header-only` target kind: a
`geometry` target that declares `include-dirs` and nothing to compile, consumed by an executable
in the same package through `deps = ["geometry"]`.

A `header-only` target produces no archive and never reaches the link line; it exists purely in
the dependency graph so its `include-dirs` (and, when declared, its `interface-c-standard` /
`interface-cxx-standard`) propagate transitively to every dependent target.  Declaring `sources`
on a `header-only` target is rejected at manifest-load time.  This is the same mechanism the
curated header-only foundation ports are built on - see [`nlohmann-json-usage/`](../nlohmann-json-usage),
[`cli11-usage/`](../cli11-usage), and [`stb-usage/`](../stb-usage) for consuming them.

Everything is local to this package, so the example builds without network access.

## Build and run

```sh
cd examples/header-only-lib
cabin build
cabin run
```

Expected output:

```
circle area (r = 2): 12.57
rectangle area (3 x 4): 12.00
```
