# Standard compatibility: registry index metadata

## 1. Status and scope

This document designs the package-index schema addition that carries standard-compatibility
metadata.  It is a companion to `spec.md` (the normative compatibility model, cited here by its
identifiers D1-D14): the spec defines the per-target requirement inputs, this document defines
where those inputs live in the index so that index consumers - the resolver's preference mode
(`preference-mode.md`), publish lints (`publish-lints.md`), and registry UIs - can evaluate them
without downloading source archives.

**Why the schema is designed before the registry ships.**  Retrofitting is the expensive path.
The index loader rejects unknown fields (`docs/package-index.md`, "Validation"), so a field added
after public registries exist forces a lockstep migration: every published index document and
every deployed loader must move together.  Designed now - while `docs/registry-design.md` still
scopes Cabin to the local file registry and the read-only sparse HTTP client - the field is part
of the first public registry format and acceptance ships in loaders before any registry emits it.
A package-level summary is listed only as a fallback if index size ever becomes a problem
(section 7); it is not the design.

## 2. Schema

The metadata is one optional field, `standards`, inside each version's metadata in
`packages/<name>.json` - a sibling of `dependencies`, `yanked`, `checksum`, and `source`.  The
same document shape serves the local file index and the sparse HTTP read path, per the transport
boundary of `docs/registry-design.md`.

```json
{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "11.0.0": {
      "dependencies": {},
      "yanked": false,
      "standards": {
        "targets": {
          "fmt": {
            "interface": {
              "c": "none",
              "c++": { "min": "c++17" }
            }
          },
          "fmt-header-only": {
            "header-only": true,
            "interface": {
              "c": "none",
              "c++": { "min": "c++20", "inferred": true }
            }
          }
        }
      }
    }
  }
}
```

`standards.targets` is the full table of (package version x target x language) -> requirement:

- Keys of `targets` are the version's **library-like** target names (`library` and `header-only`
  kinds).  Executables, tests, and examples never constrain consumers
  (`docs/language-standards.md`) and do not appear.
- `interface` maps a language key (`"c"`, `"c++"` - the languages of spec D1) to the target's
  **evaluated effective requirement**: $\mathrm{ReqOf}(t, L)$ of spec D9, computed by
  `cabin publish` from the resolved manifest attributes of D6 (target-over-package precedence and
  workspace inheritance already applied), joined with the requirements propagated along the
  version's **intra-package** public edges (spec D10, restricted to the version's own targets -
  everything the publisher can see).  All six D9 rows are baked in at publish time: explicit
  declarations (rows 1-2), header-only inference (row 3), the compiled no-declaration case
  (row 4), and both cross-language defaults (rows 5-6).  Public edges that cross packages cannot
  be folded in - their contribution depends on which dependency version a future resolution
  picks - so they are recorded instead (`public-deps` below).

Each requirement value encodes one element of the spec's requirement domain $\mathrm{Req}_L$ (D3):

| Value | $\mathrm{Req}_L$ element (D3) |
| --- | --- |
| language key omitted | $\textsf{unconstrained}$ |
| `{ "min": "<level>", "inferred": true? }` | $[m]$, a minimum |
| `"none"` | $\textsf{forbidden}$ |

`min` is an ISO level of spec D2 in its manifest spelling (`c89`...`c23`, `c++98`...`c++26`).  The
object form is the `{min, max?}` pair: `max` is **reserved and never written in v1**, mirroring
the reserved range slot of the manifest layer (spec D4 remark; `docs/language-standards.md`,
"Accepted values").  v1 loaders reject a populated `max` with an error saying range requirements
are reserved for a future version, the same dedicated diagnostic the manifest parser gives
range-like inputs.  Populating it later is a domain swap, not a schema change (spec D4 remark).
A cell is therefore either the literal string `"none"` or a `{min, max?}` object - the two
shapes encode **different** requirement kinds, never the same one.  A bare level string
(`"c++17"`) is not a valid cell: writers must use the object form for minima, and loaders
reject the bare spelling with an error naming the object form.  Parsing a string-or-object
union has loader precedent in dependency entries (bare requirement string vs full table);
unlike that precedent, there is deliberately no normalization between the two shapes here.

`inferred` (boolean, default `false`, written only when `true`) records **provenance**: the
serialized minimum is attained only by header-only inference (spec D9 row 3) - the target's own,
or that of an intra-package public dependency whose propagated requirement the join of section 2
carries - and by no explicit declaration of equal strength.  Provenance survives the
intra-package join on purpose: a wrapper target publicly re-exporting a header-only sibling
inherits the sibling's `inferred` marker along with its minimum, so the marker is trustworthy on
whichever row a diagnostic ends up citing.  When an explicit declaration (again, own or
propagated) attains the same minimum, the value is a promise and the marker is off.  The spec
keeps behaviorally equal requirements distinct precisely because diagnostics report provenance
(the remark after L3); without this marker, an explicit `interface-cxx-standard = "c++17"` and a
row-3-inferred `c++17` would serialize identically and held-back reports could not present
inferred minima as inferred (`preference-mode.md`).  The publish lint PL2 warns where inference
originates (`publish-lints.md`).

### Per-target flags

Two boolean flags, both optional, both defaulting to `false`:

- `header-only` - the target kind of spec D6.  It never enters the satisfaction predicate (D11
  consumes only the requirement), but index consumers need it as a fact about the target: UIs
  surface header-only-ness without archive downloads, and the publish lints of
  `publish-lints.md` give it context (a header-only target is where inference, marked by the
  per-cell `inferred` flag above, can occur at all - D9 row 3).
- `gnu-extensions` - the target's lowering-time dialect flag.  Invariant I1 (spec D8) holds
  verbatim in this schema: nothing in `interface` depends on it and it never participates in
  compatibility.  It is carried because index consumers need it anyway - an MSVC-dialect build
  rejects `gnu-extensions` targets outright (`docs/language-standards.md`, "Flag lowering"), so
  registry UIs and toolchain-aware tooling can surface per-target buildability without fetching
  the archive.

### Cross-package public re-exports

Requirements propagate along **public** edges (spec D10), and a public edge can cross packages:
a wrapper target that publicly re-exports a stricter dependency imposes that dependency's
requirement on its own consumers ($R_L$, D10; edge compatibility checks $R_L$, D13).
Intra-package public edges are already folded into the `interface` cells above, but a
cross-package edge's contribution depends on which dependency version a resolution picks, so the
publisher cannot fold it in.  It is recorded instead:

- `public-deps` (array of strings, omitted when empty, sorted) - the target's **effective**
  cross-package public references: every cross-package dependency reference declared
  `public = true` (in the manifest's qualified `package:target` spelling, `docs/targets.md`)
  on this target *or on any sibling reachable from it along intra-package public edges* - the
  same closure the `interface` cells fold.  The closure is required, not a nicety: folding
  intra-package edges into the cells erases the path to a sibling's cross-package edge.  If
  `api` publicly re-exports sibling `core`, and `core` publicly re-exports `bar:bar`, then
  `bar:bar` appears in `api`'s `public-deps` - otherwise an index consumer at `api` could
  never discover `bar`'s contribution to `api`'s effective requirement.

A target's row and its `public-deps` are thus a self-contained frontier: the cells carry
everything the version itself contributes to $R_L$ (D10 restricted to the version), and
`public-deps` names exactly where propagation continues in other packages.  An index consumer
can complete D10 across packages by walking candidate versions' tables from that frontier - how
far a consumer traverses is its own trade-off (`preference-mode.md`).  On entries that omit the field (all pre-`standards` entries), the
advertised cells are a **lower bound** on the true effective requirement, which is consistent
with the advertisement framing of section 3: selection metadata may under-promise, the
build-time enforcement recomputes the truth.

## 3. Absence means unconstrained

Absence encodes $\textsf{unconstrained}$ (the least element of D3) at every granularity:

- A version with no `standards` field: every (target, language) pair is unconstrained.  **Every
  existing index entry is therefore already a valid instance of this schema, unchanged.**
- A target missing from `targets`: unconstrained in both languages.
- A language key missing from a target's `interface`: unconstrained for that language.
- A missing flag: `false`.

This is deliberately weaker than spec D9's strict C default (row 6, $\textsf{forbidden}$).  The
published table is *evaluated*, so a row-6 $\textsf{forbidden}$ is always written explicitly as
`"none"`; absence never has to mean forbidden.  A metadata-free entry gives the resolver nothing
to prefer or filter - preference mode deprioritizes such candidates but never drops them
(`preference-mode.md`), and the post-resolution build-time interface enforcement of
`docs/language-standards.md` remains the correctness backstop.  Index metadata is advertisement
consumed for version selection; the fetched archive's manifest stays the ground truth the build
enforces.

## 4. Determinism

Generated index documents stay deterministic, per the contributor guardrails and the
file-registry writer contract of `docs/registry-design.md`:

- `targets` is serialized in sorted target-name order (the in-memory model mirrors the index
  crate's `BTreeMap` convention).
- Language keys appear in the fixed order `"c"`, `"c++"` - C stays first-class alongside C++.
- `cabin publish` writes every library-like target of the version (an entry whose requirements
  are all unconstrained and whose flags are false serializes as `{}` - the target existing and
  imposing nothing is itself information), omits unconstrained language keys, and omits every
  default-valued member (`false` flags, `inferred: false`, empty `public-deps`).
- `public-deps` arrays are sorted lexicographically.

## 5. Composition with the sparse HTTP layout

The table rides inside `packages/<name>.json`, so it composes with the sparse HTTP index of
`docs/registry-design.md` and `docs/package-index.md` with **zero new requests**: it arrives in
the existing step-2 `GET <url>/<config.packages>/<name>.json`, one request per package.
`config.json`, the artifact URL scheme, checksum verification, and the frozen/offline limits are
all untouched.  That is the point of index-level metadata: preference mode can rank every
candidate version of every package from metadata alone, where fetching archives to read manifests
would cost one download per candidate version.

Size: at most two languages times a handful of library-like targets, a few bytes per cell -
negligible next to each version's `dependencies` map.

## 6. Migration

- The field is additive and optional; the document stays `schema = 1`, no version bump.
- Because loaders reject unknown fields, **loader acceptance of `standards` must ship before any
  registry emits it**.  No public registry exists yet, so shipping acceptance first costs
  nothing - this ordering is the retrofit-cost rationale of section 1 in concrete form.  (Once
  the writer lands, a local file registry produced by a newer Cabin is unreadable by older
  Cabins; acceptable pre-1.0, to be called out in release notes.)
- The existing `language` passthrough on version metadata (the `[package]`-level manifest fields,
  preserved as-is, round-trip only, never consumed by the resolver) is unchanged and stays.
  `standards` is the typed, evaluated, per-target consumption form; the passthrough remains
  archival.

## 7. Fallback: package-level summary (not chosen)

If per-target tables ever dominate index size, the fallback is one row per version and language:
the join (spec D4) of the version's per-target requirements, i.e. the strictest.  It is lossy in
exactly the way the per-target table is not: the strictest target dominates, over-constraining
consumers that only use a milder target, and the provenance a held-back report needs (which
target, inferred or declared) is erased.  It would be adopted only under demonstrated size
pressure, never as the default.
