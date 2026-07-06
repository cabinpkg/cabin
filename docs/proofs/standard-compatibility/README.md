# Standard compatibility: Lean 4 mechanization

This directory mechanizes `docs/design/standard-compatibility/spec.md` in Lean 4.

**The Markdown specification remains the normative source.**  This Lean development is a
verification artifact: it machine-checks the spec's numbered lemmas and theorems, it does not
replace or reinterpret them.  Where the resolver implementation and the spec disagree, the spec
wins; where this development and the spec disagree, that is a bug to report, not a reason to
weaken either side silently.

The project is a standalone Lake package.  It is not part of the Cargo workspace or the main
build; CI checks it through the dedicated `Proofs` workflow
(`.github/workflows/proofs.yml`), which runs when `docs/proofs/` or the spec changes.  Alongside
`lake build`, that workflow fails if any numbered lemma/theorem/corollary in the spec has no
same-named Lean declaration, so adding or renumbering a spec item without mechanizing it is
caught even though a spec-only change cannot break the Lean build itself.

## Checking the proofs

```sh
cd docs/proofs/standard-compatibility
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
| `StandardCompatibility/Semilattice.lean` | Generic bounded join-semilattice (the algebra of spec L2), list joins, L7; a small `FinEnum` class so `decide` covers the finite spec domains |
| `StandardCompatibility/Graph.lean` | `R_L` as a fold over a finite DAG (D10), public reachability (D5), T1, T2, C1, C2, generic T4 |
| `StandardCompatibility/Spec.lean` | Concrete model: levels (D2), `Req` (D3, D4), `satisfies` (D11, D12), `ReqOf` (D9), consumers/edges/viability (D7, D13, D14), L1-L6, C3, per-language T4, T3 decidability |
| `StandardCompatibility/Examples.lean` | The spec appendix's five worked examples, kernel-computed; Example 3 runs end-to-end through the DAG fold |

## Spec item to Lean name mapping

Every Lean theorem is named after its spec identifier.  Finite-domain statements are proved by
`decide` (exhaustive kernel enumeration - exactly the appendix's "verifiable by exhaustive
enumeration" note); graph-generic statements have general proofs, not enumeration.

| Spec item | Lean names (`StdCompat.*`) |
|---|---|
| D2 levels | `CLevel`, `CxxLevel`, `CLevel.leB`, `CxxLevel.leB` (chronological rank order) |
| D3/D4 Req, order, join | `Req`, `Req.leB`, `Req.join` (`leC`/`leCxx`, `joinC`/`joinCxx`) |
| D5 public reach | `DepRel`, `reachList`, `reachList_eq`, `mem_reachList_self` |
| D6 attributes | `Attrs`, `Kind`, `IfaceDecl` |
| D7 consumers | `Consumer` |
| D9 ReqOf | `reqOfC`, `reqOfCxx` (row-for-row the spec's decision table) |
| D10 effective requirement | `Rfun` (generic), `effectiveReqC`, `effectiveReqCxx` |
| D11/D12 satisfies / Sat | `Req.satisfies` (`satC`/`satCxx`); `Sat` membership is `satisfies = true` |
| D13 edge compatibility | `edgeCompat` (`satOptC`/`satOptCxx`: a language the consumer does not compile imposes nothing) |
| D14 viability | `viable` |
| L1 finite chain | `L1_refl_*`, `L1_trans_*`, `L1_antisymm_*`, `L1_total_*`, `L1_bot_*`, `L1_top_*` |
| L2 join-semilattice | `L2_assoc_*`, `L2_comm_*`, `L2_idem_*`, `L2_id_*`, `L2_absorb_*`, `L2_lub_*`; bundled as `SC`, `SCxx` (`JSL`); generic order facts in `JSL.*` |
| L3 semantic characterization | `L3_sound_*`, `L3_complete_except_*` (the single degenerate pair carved out, as in the spec), `L3_exception_genuine_*`, `L3_sat_eq_iff_*` |
| L4 join = intersection | `L4_inter_*` |
| L5 antitonicity | `L5_c`, `L5_cxx` (option-lifted: `satOptC_antitone`, `satOptCxx_antitone`) |
| L6 upward closure | `L6_upward_*` |
| L7 join monotonicity | `JSL.L7_joinList_mono_subset`, `JSL.L7_joinList_mono_pointwise` |
| T1 well-definedness | `T1_exists`, `T1_unique`, `T1_closed_form`, `T1_order_independence` (confluence as invariance under re-enumeration; termination is the well-founded recursion itself) |
| T2 growth | `T2_growth` |
| C1 / C2 / C3 | `C1_add_edge`, `C2_declare`, `C3_viable_shrink` |
| T3 decidability | `T3_satisfies_decidable`, `T3_edge_decidable`, `T3_viability_decidable` (see note below) |
| Assumption A / T4 | `T4_soundness` (generic, with A as the hypothesis `A` and antitonicity of the satisfaction predicate), `T4_soundness_c`, `T4_soundness_cxx` |
| Appendix examples 1-5 | `Examples.lean` (`ex1*` ... `ex5*`) |

## Modeling notes and reported deviations

Nothing in the spec was weakened.  Two items need a modeling note:

- **T3 (complexity).**  The decidability half of T3 is mechanized (`T3_*_decidable`; every
  predicate in the development is a computable `Bool`).  The `O(1)` / `O(|V| + |E|)` complexity
  bounds are claims about an execution cost model, which plain Lean terms do not carry; they
  are not stated in Lean.  This is a scope limit of the mechanization, reported here rather
  than silently dropped - the spec's pen-and-paper argument stands on its own.
- **"Finite DAG" hypothesis.**  The spec's finite acyclic graph enters Lean as
  well-foundedness of the public-dependency relation (`WellFounded (DepRel deps)`), which is
  exactly what the spec's longest-path height function provides on a finite DAG, and is the
  precise hypothesis T1's recursion needs.  `PubReach` is represented as a list
  (`reachList`); duplicate entries are harmless by idempotence
  (`JSL.joinList_congr_mem`), which is also how T1's order-independence is stated.

Two spec statements have shapes worth calling out (both faithful, not weakened):

- **L3's completeness direction** is proved with the same single degenerate pair the spec
  carves out (`[bottom level]` vs `unconstrained`); `L3_exception_genuine_*` proves the
  exception is real, and `L3_sat_eq_iff_*` proves it is the only one.
- **T4 / Assumption A** is mechanized with "compiles" as an abstract predicate and Assumption
  A as a hypothesis, mirroring the spec: Cabin checks the lattice arithmetic, the author
  promises the headers; the theorem discharges the obligation for everything publicly
  reachable.
