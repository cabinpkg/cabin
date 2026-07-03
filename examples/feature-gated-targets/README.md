# feature-gated-targets

A workspace showing package features gating an optional library target:

- `packages/netlib` - declares a `tls` feature and two libraries: `net` (always available) and
  `tls`, whose `required-features = ["tls"]` makes it buildable only when the `tls` feature is
  enabled.
- `packages/app` - an `executable` that enables the feature on its dependency edge
  (`netlib = { path = "../netlib", features = ["tls"] }`) and links both libraries explicitly
  with `deps = ["netlib:net", "netlib:tls"]`.

The two steps are deliberately separate: enabling a feature only makes the gated target
*available* - it never adds a link edge to any consumer.  A consumer that wants the optional
target both enables the feature and names the target in `deps`.  Dropping either half is a hard
error: without the feature the planner reports the missing `required-features`, and without the
`deps` entry nothing links `tls`.

Inside `netlib`, `[target.tls]` also shows the bare-name shorthand: its `deps = ["net"]` names a
same-package target directly.

## Build and run

```sh
cd examples/feature-gated-targets

# Build every member
cabin build --workspace

# Run the app (selected by default-members)
cabin run
```

Expected output:

```
GET example.org
TLS GET example.org
```

Building `netlib` alone with its default (empty) feature set skips the gated target: `cabin build
-p netlib` compiles only `net`.
