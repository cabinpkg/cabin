# library-with-tests

A single Cabin package with a `library` target and two `test` targets
that exercise it. This is the example to read for `cabin test`.

A Cabin `test` target is an ordinary executable: it **passes when its
`main()` returns `0`** and fails otherwise. There is no framework,
macro, or attribute to learn. `cabin test` builds every `type = "test"`
target, runs each one, and reports a per-target `... ok` / `... FAILED`
line plus a summary. Tests run in a deterministic order — by package
name, then target name — so `calc_test` always runs before
`parity_test`.

Both tests depend on the `calc` library through `deps = ["calc"]` and
include its public header through `calc`'s `include_dirs = ["include"]`.

## Run the tests

```sh
cd examples/library-with-tests
cabin test
```

Expected output (the `Compiling …` build line goes to stderr and is
omitted here):

```
running 2 tests
test library-with-tests:calc_test ... ok
test library-with-tests:parity_test ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

A passing test is silent on stdout. The `check(...)` helper in
`tests/*.cc` only writes (to stderr) when an assertion fails, which
also makes that target exit non-zero so `cabin test` reports it as
`FAILED (exit N)` and the command exits non-zero.

This package has no `executable` target, so `cabin run` does not apply
here. A plain `cabin build` compiles only the `calc` library; the
`test` targets are dev-only, so `cabin test` is what builds *and* runs
the two test binaries.
