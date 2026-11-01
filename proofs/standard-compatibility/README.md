# Standard compatibility: Lean 4 model

This directory is a Lean 4 formal model of the resolver-level language-standard compatibility
rules: the requirement domain and its join, how a target's declarations become a requirement
(D9), propagation along public edges (D10), and edge compatibility and version viability (D13,
D14), with machine-checked lemmas and theorems about them.  Code comments cite its identifiers
(D2...D14, L1...L7, T1...T4, C1...C3, Examples 1-6).  The user-facing behavior is documented in
`docs/language-standards.md`.

The project is a standalone Lake package.  It is not part of the Cargo workspace or the main
build; CI checks it through the dedicated `Proofs` workflow
(`.github/workflows/proofs.yml`), which runs `lake build` when `proofs/` changes.

## Checking the proofs

```sh
cd proofs/standard-compatibility
lake build
```

`elan` reads `lean-toolchain` and installs the pinned toolchain (`leanprover/lean4:v4.31.0`)
automatically on first use.  There are no package dependencies (no mathlib); a successful
`lake build` means every theorem in the development is kernel-checked.  The development
contains no `sorry` and uses no axioms beyond Lean's standard `propext`, `Quot.sound`, and
(for one classical case split in C2) `Classical.choice`.

## Layout

| File | Content |
|---|---|
| `StandardCompatibility/Semilattice.lean` | Generic bounded join-semilattice (the algebra of L2), list joins, L7; `JoinLub` (join as least upper bound for an arbitrary preorder, feeding the denotational growth statements); a small `FinEnum` class so `decide` covers the finite domains |
| `StandardCompatibility/Graph.lean` | `R_L` as a fold over a finite DAG (D10), public reachability (D5), T1, T2, C1, C2, generic T4 |
| `StandardCompatibility/Spec.lean` | Concrete model: levels (D2), `Req` (D3, D4), `satisfies` (D11, D12), `ReqOf` (D9), consumers/edges/viability (D7, D13, D14), L1-L6, C3, per-language T4, T3 decidability |
| `StandardCompatibility/Examples.lean` | Six worked examples, kernel-computed; Example 3 runs end-to-end through the DAG fold |

## Identifier to Lean name mapping

Every Lean theorem is named after its identifier.  Finite-domain statements are proved by
`decide` (exhaustive kernel enumeration); graph-generic statements have general proofs, not
enumeration.

| Identifier | Lean names (`StdCompat.*`) |
|---|---|
| D2 levels | `CLevel`, `CxxLevel`, `CLevel.leB`, `CxxLevel.leB` (chronological rank order) |
| D3/D4 Req, order, join | `Req` (interval domain incl. `bounded`), `Req.valid`, `Req.leB` (denotational, via `Req.leB_spec`), `Req.join` (intersection with empty-collapse); `leC`/`leCxx`, `joinC`/`joinCxx`, `validC`/`validCxx` |
| D5 public reach | `DepRel`, `reachList`, `reachList_eq`, `mem_reachList_self` |
| D6 attributes | `Attrs`, `Kind`, `IfaceDecl` |
| D7 consumers | `Consumer` |
| D9 ReqOf | `reqOfC`, `reqOfCxx` (row-for-row the D9 decision table) |
| D10 effective requirement | `Rfun` (generic), `effectiveReqC`, `effectiveReqCxx` |
| D11/D12 satisfies / Sat | `Req.satisfies` (`satC`/`satCxx`); `Sat` membership is `satisfies = true` |
| D13 edge compatibility | `edgeCompat` (`satOptC`/`satOptCxx`: a language the consumer does not compile imposes nothing) |
| D14 viability | `viable` |
| L1 preorder / quotient / non-totality | `L1_refl_*`, `L1_trans_*`, `L1_antisymm_upto_sat_*`, `L1_antisymm_fails_on_shapes_*`, `L1_quotient_*` (denotational equality iff equal `canonC`/`canonCxx` canonical forms - the class list), `L1_not_total_*`, `L1_bot_*`, `L1_top_*` |
| L2 join-semilattice (on valid shapes) | `L2_closed_*`, `L2_assoc_*`, `L2_comm_*`, `L2_idem_*`, `L2_id_*`, `L2_absorb_*`, `L2_lub_*`; bundled as `SC`, `SCxx` (`JSL` over the valid subtype `VReqC`/`VReqCxx`); generic order facts in `JSL.*` |
| L3 strictness is denotational | `L3_denotational_*` (definitional after D3; bridge to the semilattice order: `SC_le_sat`, `SCxx_le_sat`, `join_eq_le_*`) |
| L4 join = intersection | `L4_inter_*`, `L4_empty_collapse_cxx` (the collapse is exact) |
| L5 antitonicity | `L5_c`, `L5_cxx` (option-lifted: `satOptC_antitone`, `satOptCxx_antitone`) |
| L6 convexity, not upward closure | `L6_convex_*`, `L6_upward_closed_iff_*` (the exact characterization: upward closure fails precisely for `upwardClosed*`-refuting capped bounded shapes, `cappedBelowTop*`), `L6_not_upward_closed_cxx`, `L6_upward_closed_min_only_cxx` |
| L7 join monotonicity | `JoinLub.L7_subset`, `JoinLub.L7_pointwise` (any join-compatible preorder, hence the denotational `⊑`: `L7_subset_denotational_*`, `L7_pointwise_denotational_*`); structural forms `JSL.L7_joinList_mono_subset`, `JSL.L7_joinList_mono_pointwise` |
| T1 well-definedness | `T1_exists`, `T1_unique`, `T1_closed_form`, `T1_order_independence` (confluence as invariance under re-enumeration; termination is the well-founded recursion itself) |
| T2 growth | `T2_growth_denotational_c`, `T2_growth_denotational_cxx` (hypothesis and conclusion in the denotational `⊑`, via `T2_growth_of` and `denotJoinLub_*`); `T2_growth` (structural-order form) |
| C1 / C2 / C3 | `C1_add_edge`, `C2_declare`, `C3_viable_shrink` |
| T3 decidability | `T3_satisfies_decidable`, `T3_edge_decidable`, `T3_viability_decidable` (see note below) |
| Assumption A / T4 | `T4_soundness` (generic, with A as the hypothesis `A` and antitonicity of the satisfaction predicate), `T4_soundness_c`, `T4_soundness_cxx` |
| Examples 1-6 | `Examples.lean` (`ex1*` ... `ex6*`) |

## Modeling notes

Two items need a modeling note:

- **T3 (complexity).**  The decidability half of T3 is mechanized (`T3_*_decidable`; every
  predicate in the development is a computable `Bool`).  The `O(1)` / `O(|V| + |E|)` complexity
  bounds are claims about an execution cost model, which plain Lean terms do not carry; they
  are not stated in Lean; this is a scope limit of the model.
- **"Finite DAG" hypothesis.**  The finite acyclic target graph enters Lean as
  well-foundedness of the public-dependency relation (`WellFounded (DepRel deps)`), which every
  finite DAG satisfies, and is the precise hypothesis T1's recursion needs.  `PubReach` is
  represented as a list (`reachList`); duplicate entries are harmless by idempotence
  (`JSL.joinList_congr_mem`), which is also how T1's order-independence is stated.

Three statements have shapes worth calling out:

- **Two orders, one join.**  The generic graph layer derives its order structurally
  (`JSL.le a b := join a b = b`), which is finer than the denotational `⊑` of D3
  (`atLeast bot` and `unconstrained` are `⊑`-equivalent but structurally distinct).  The
  growth statements are therefore mechanized against `JoinLub` - join as least upper bound
  for an arbitrary preorder - and instantiated with the denotational order
  (`T2_growth_denotational_*`, `L7_*_denotational_*`), so their hypotheses and conclusions
  are the denotational ones, not the finer structural ones.  C1/C2 need no such split: their
  `ReqOf` hypotheses are syntactic (unchanged / equal to `unconstrained`), and the structural
  conclusion transports to `⊑` through `SC_le_sat` / `SCxx_le_sat`.

- **The bounded-range invariant.**  The Lean `Req` type carries `bounded lo hi` without a
  baked-in `lo <= hi` proof; `Req.valid` states the invariant, `L2_closed_*` proves the join
  preserves it, `reqOf*_valid` proves D9 lands inside it (an empty *declared* range - which
  the manifest parser rejects - is defensively normalized to `forbidden`), and the
  semilattice instances `SC`/`SCxx` live on the valid subtype, mirroring the
  implementation's constructed-validated `Requirement`.
- **T4 / Assumption A** is mechanized with "compiles" as an abstract predicate and Assumption
  A as a hypothesis: Cabin checks the lattice arithmetic, the author promises the headers; the
  theorem discharges the obligation for everything publicly reachable.
