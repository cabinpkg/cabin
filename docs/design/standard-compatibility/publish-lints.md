# Standard compatibility: publish-time lints

## 1. Status and scope

This document designs the standard-compatibility checks `cabin publish` (and `publish --dry-run`)
runs after manifest load and packaging, before any registry write.  A rejecting lint fails the
publish before anything is written, so the atomic-write contract of `docs/registry-design.md`
holds trivially: a rejected publish leaves the registry exactly as it was.  The lints evaluate
the resolved manifest attributes of spec D6 (where declaration provenance is fully known)
together with the `standards` table of `registry-index.md` in exactly the form it would be
serialized - what is linted is what would be published.  Spec identifiers (D1-D14, C1-C3, T1-T4,
Example n) refer to `spec.md`.

Three lints, one impossibility:

| Lint | Severity | Condition |
| --- | --- | --- |
| PL1 | error | implementation standard outside the same language's declared interface range |
| PL2 | warning | header-only target's interface left to inference for an implemented language |
| PL3 | warning | declared interface requirement narrowed in a patch release |
| - | none, by construction | GNU-extension interface requirement (unrepresentable, section 5) |

## 2. PL1 (error): implementation standard outside the declared interface range

For every published library-like target $t$ and language $L$, PL1 checks the **direct declared
pair**: if $t$'s explicit interface declaration $\mathrm{decl}_L(t)$ is a range $(m, M)$ (a
declared range, not `"none"`) and its own implementation standard $\mathrm{impl}_L(t)$ is
present but falls outside it - $m > \mathrm{impl}_L(t)$, or $M < \mathrm{impl}_L(t)$ for a
bounded declaration - **reject the publish**.  The stored `standards` table of
`registry-index.md` records each target's own declared $\mathrm{ReqOf}$ uncomposed and does not
carry $\mathrm{impl}_L(t)$, so PL1 evaluates the pair from the resolved manifest attributes
(spec D6) it is already linting - the declaration exactly as it would be serialized, checked
against the implementation standard the same manifest resolves.

- **Compiled targets.**  This is the same predicate as the load-time
  `cabin::language::interface_standard_contradiction` (`docs/language-standards.md`,
  "Precedence"), re-asserted at the publish boundary - defense in depth - so no registry entry
  can carry the contradiction regardless of how the manifest reached `cabin publish`.  A
  compiled target's own translation units include its own public headers, so a declared minimum
  newer than the standard those units compile at - or a declared maximum older than it -
  describes a target that cannot build itself under its own published contract.
- **Header-only targets.**  Exempt from the load-time check (no translation units of their own
  to contradict), so PL1 is the only layer that catches their direct pair: a header-only target
  populates $\mathrm{impl}_L$ only through a target-level implementation declaration (D6
  population contract), and pairing that with a newer explicit interface minimum publishes a
  promise that consumers need more than the headers were written against.  In a shared index
  that inflated minimum propagates along public edges (D10) and shrinks every downstream viable
  set (C3) with no compensating correctness gain.  A local build may tolerate the odd pairing;
  the public record does not.

PL1 is a per-target, per-declaration check and does **not** fold intra-package public edges: the
stored table records declarations and consumers compose (`registry-index.md`, "Composition is
the consumer's job"), because composition depends on which dependency versions a consumer
resolves.  A target compiling at its own implementation standard while publicly re-exporting a
stricter sibling is therefore not a PL1 case here - that sibling's requirement reaches consumers
through the ordinary effective-requirement recursion (spec D10) at resolution time, not through a
folded publish-time cell.  $\textsf{forbidden}$ cells are outside PL1: `"none"` is a deliberate
consumer opt-out, not a contradiction (the post-resolution compatibility check enforces it,
with the per-edge override - spec section 1; `docs/language-standards.md`).

## 3. PL2 (warning): header-only interface left to inference

For a header-only target $t$ and language $L$ with $\mathrm{impl}_L(t)$ present and
$\mathrm{decl}_L(t) = \bot$: the published requirement is the inferred
$[\mathrm{impl}_L(t)]$ (spec D9 row 3).  **Warn**, recommending an explicit
`interface-c-standard` / `interface-cxx-standard`.

Inference is sound - it can only over-constrain, never under-constrain - but the implementation
standard is merely an upper bound on what the headers need.  Spec Example 5 is the canonical
case: headers written under `c++20` that audit down to `c++17`, halving away compatible consumers
until the author declares the audited minimum.  An explicit declaration is that audit made
durable; the warning asks for it at the moment the requirement becomes a public record.

The manifest layer already requires a header-only target to declare at least one interface
standard (`docs/language-standards.md`, "Precedence"), so PL2 fires on the residual per-language
case: a target declaring an interface for one language while leaving an implemented second
language to inference.  $\mathrm{decl}_L$ is the post-precedence value of D6, so a package-level
interface declaration counts as declaring; only genuine D9 row-3 inference warns.  The published
table does not distinguish an inferred minimum from a declared one (`registry-index.md`), so PL2
identifies the inference from the manifest attributes it is already reading (D6/D9), not from the
index.  It warns once, at the origin target, because that is where the audit and the fix live.

## 4. PL3 (warning): interface requirement narrowed in a patch release

**Baseline.**  The greatest already-published version strictly below the new version that shares
its `major.minor` - the release the new version patches.  If none exists (an `x.y.0`, or the
first publish of a line), the new version is not a patch release and PL3 does not apply.
Pre-release versions neither trigger the lint nor serve as baseline; their contract is explicitly
unstable.

**Narrowing.**  Some (target, language) pair present in both versions whose new requirement
**excludes a consumer level the baseline accepted**:
$\mathrm{Sat}(\text{old}) \not\subseteq \mathrm{Sat}(\text{new})$ (spec D12).  The
strictness order is only partial (spec D3), so the lint is semantic rather than order-based;
it covers a raised minimum, a lowered or newly added maximum, a range shifted sideways (which
both adds and drops levels - the dropped ones are what breaks existing consumers), a first
declaration on a previously $\textsf{unconstrained}$ cell that excludes anything, and a flip
to `"none"` ($\mathrm{Sat} = \emptyset$ excludes everything).  A reshaping that keeps the
accepted set intact - e.g. $\textsf{unconstrained} \to [\bot_L, {\uparrow}]$ - does not
warn, and neither does any pure widening.  A target absent from the baseline's table is an
addition, not a narrowing - `cabin publish` writes a row for every library-like target, so a
target genuinely present in the baseline is never missing from its table.  The compared cells
are the published **declared** requirements of `registry-index.md` (each target's own
$\mathrm{ReqOf}$, uncomposed), so PL3 catches a narrowing of a target's own declaration.

**Limitation - effective narrowings PL3 cannot see.**  A patch can narrow consumers' *effective*
requirements without changing any target's own declared cell: adding (or flipping to public) a
public dependency edge - intra- or cross-package - imposes the re-exported dependency's
requirements on every consumer, and adding a public edge never lowers $R_L$ (spec C1); likewise
changing the version requirement on an existing public dependency can pull in a stricter
transitive requirement.  Because this index stores declared, uncomposed cells and no public-edge
structure (`registry-index.md`, "Composition is the consumer's job"), the declared-cell
comparison cannot detect these narrowings - the imposed requirement depends on which dependency
version a future resolution picks, which the publisher cannot know.  PL3 deliberately scopes to
declared-cell narrowings; the held-back reports of `preference-mode.md` carry the
per-resolution consequences of the composed ones.  Removing a declaration or widening a range
is a relaxation and is never linted.

**Limitation - baselines with no recorded table.**  A baseline whose index entry stores no
`standards` table - a version published before the field existed, or one with no library-like
targets - offers no rows to compare, so PL3 makes no comparison against it.  Reading an absent
table as $\textsf{unconstrained}$ everywhere instead would flag a package's *first* library-like
target added in a patch release as a narrowing, a false positive:
that target is an addition, not a tightened declaration, and the two are indistinguishable from a
baseline that records nothing.  PL3 therefore skips an unrecorded baseline; the only miss is a
requirement narrowed in the very first patch published after the `standards` field landed - a
transient migration-window gap on a warning-only check, weighed against a permanent false
positive on every legitimate first-library addition.

**Warn, citing the policy: requirement narrowings are treated as minor incompatibilities -
allowed in minor releases, discouraged in patches.**  A narrowing shrinks the set of consumers
whose edges remain compatible (spec C3 covers the pure-tightening direction; a sideways shift
still breaks every consumer sitting on a dropped level), and caret-style requirements pull
patch releases in automatically, so a patch-level narrowing breaks consumers who changed
nothing.  PL3 stays a warning rather than an error because the registry
cannot see consumers and a fix may legitimately need the narrowing; preference mode
(`preference-mode.md`) then softens the blast radius by holding non-satisfying consumers back
with a report instead of failing them.

Widening a requirement is never linted, in any release type: a pointwise relaxation only grows
the viable set (spec T2 with the assignments swapped; remark after C3).

## 5. No GNU-extension lint, by construction

There is no lint for "interface requires a GNU dialect" because the state is unrepresentable:
**interface requirements are ISO-only in the data model.**  The level chains of spec D2 contain
no GNU spellings (`gnu++20` is not a level; manifests reject GNU dialect strings as unknown
values, `docs/language-standards.md`), and Invariant I1 (spec D8) keeps `gnu-extensions` out of
every compatibility input.  The flag reaches the index only as the per-target display and
toolchain-viability flag of `registry-index.md`, never as a requirement.

## 6. Mechanics

- Findings are reported in deterministic order - by target name, then language (`c` before
  `c++`) - so publish output is stable for CI logs.
- PL1 fails the publish before any registry artifact or index write.  PL2 and PL3 print to
  stderr and let the publish proceed.
- PL1 and PL2 need only the resolved manifest, so they run on every publish and every
  `--dry-run` - including the staging-only dry-run without `--registry-dir`, which still stages
  the archive and canonical metadata document as documented (`docs/package-format.md`); the
  no-write guarantee of section 1 is about registry state, not the staging output.
- PL3 is registry-backed: its baseline is the `<name>.json` the file-registry writer would
  splice into (`docs/registry-design.md`), so it runs exactly when a registry is in reach -
  `--registry-dir`, with or without `--dry-run`.  A staging-only dry-run has no registry to
  read; it **skips PL3 and says so** in its output, rather than inventing an empty baseline
  that would make every patch release look clean.  A future hosted registry runs the same check
  against its own index, server- or client-side; that control plane is outside the local-core
  boundary and outside this document.
