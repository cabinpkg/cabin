/-
The spec's appendix, mechanized: the six worked examples, checked by
kernel computation (`decide` / `rfl`), plus Example 3 run end-to-end
through the DAG fold `Rfun`.
-/

import StandardCompatibility.Spec

namespace StdCompat

/-! ## Example 1: C++23 implementation, c++17 interface, consumed from c++17 -/

def ex1Z : Attrs := { kind := .compiled, implCxx := some .cxx23, declCxx := .declaredMin .cxx17 }

/-- D9 row 2: the explicit declaration wins over the implementation standard. -/
example : reqOfCxx ex1Z = .atLeast .cxx17 := rfl

/-- The edge from a c++17 consumer is compatible. -/
example : satCxx .cxx17 (reqOfCxx ex1Z) = true := by decide

/-- Contrast: with the declaration absent, this compiled target would impose
nothing (D9 row 4). -/
example : reqOfCxx { ex1Z with declCxx := .absent } = .unconstrained := rfl

/-! ## Example 2: diamond - consumers at c++17 and c++23 share one dependency -/

/-- The c++23 consumer satisfies `[c++20]`, the c++17 consumer does not;
viability is the conjunction over both edges, so the shared version is not
viable. -/
example :
    satCxx .cxx23 (.atLeast .cxx20) = true ∧
      satCxx .cxx17 (.atLeast .cxx20) = false := by decide

example :
    viable (T := Unit) (fun _ => .unconstrained) (fun _ => .atLeast .cxx20)
      [({ lvlCxx := some .cxx17 }, ()), ({ lvlCxx := some .cxx23 }, ())] = false := by
  decide

/-! ## Example 3: `"none"` on a transitive public dependency poisons the root

Run end-to-end through the DAG fold: `Root -> A -> B`, both edges public,
`B` declares `interface-cxx-standard = "none"`. -/

inductive Ex3T : Type
  | root | a | b
  deriving DecidableEq, Repr

instance : FinEnum Ex3T where
  elems := [.root, .a, .b]
  complete := by intro x; cases x <;> decide

def ex3deps : Ex3T → List Ex3T
  | .root => [.a]
  | .a => [.b]
  | .b => []

def ex3attrs : Ex3T → Attrs
  | .root => { kind := .compiled, implCxx := some .cxx26 }
  | .a => { kind := .compiled, implCxx := some .cxx17 }
  | .b => { kind := .compiled, implCxx := some .cxx17, declCxx := .declaredNone }

def ex3rank : Ex3T → Nat
  | .root => 2 | .a => 1 | .b => 0

theorem ex3sub : ∀ d t : Ex3T, d ∈ ex3deps t → ex3rank d < ex3rank t := by decide

theorem ex3wf : WellFounded (DepRel ex3deps) :=
  Subrelation.wf (fun {d t} h => ex3sub d t h) (measure ex3rank).wf

/-- The lifted `ReqOf` feeding the subtype fold. -/
def ex3vreq (u : Ex3T) : VReqCxx := ⟨reqOfCxx (ex3attrs u), reqOfCxx_valid (ex3attrs u)⟩

/-- `B`'s opt-out is `forbidden` (D9 row 1) and propagates through the
public chain to `A`: the absorbing element of L2 in action. -/
theorem ex3_a_forbidden : effectiveReqCxx ex3deps ex3wf ex3attrs .a = .forbidden := by
  have hb : Rfun SCxx ex3deps ex3wf ex3vreq .b = ⟨.forbidden, rfl⟩ := by
    rw [T1_exists]; rfl
  show (Rfun SCxx ex3deps ex3wf ex3vreq .a).val = .forbidden
  rw [T1_exists]
  simp only [ex3deps, List.map_cons, List.map_nil, JSL.joinList_cons, JSL.joinList_nil, hb]
  rfl

/-- Even at `c++26`, the newest level there is, the root's edge onto `A` is
incompatible: `Sat(forbidden)` is empty. -/
theorem ex3_root_edge_incompatible :
    edgeCompat { lvlCxx := some .cxx26 } .unconstrained
      (effectiveReqCxx ex3deps ex3wf ex3attrs .a) = false := by
  rw [ex3_a_forbidden]
  decide

/-- Had the edge `A -> B` been private (dropped from the public graph),
`A` would impose nothing. -/
def ex3depsPriv : Ex3T → List Ex3T
  | .root => [.a]
  | _ => []

theorem ex3subPriv : ∀ d t : Ex3T, d ∈ ex3depsPriv t → ex3rank d < ex3rank t := by decide

theorem ex3wfPriv : WellFounded (DepRel ex3depsPriv) :=
  Subrelation.wf (fun {d t} h => ex3subPriv d t h) (measure ex3rank).wf

def ex3vreqPriv (u : Ex3T) : VReqCxx := ⟨reqOfCxx (ex3attrs u), reqOfCxx_valid (ex3attrs u)⟩

theorem ex3_private_unaffected :
    effectiveReqCxx ex3depsPriv ex3wfPriv ex3attrs .a = .unconstrained := by
  show (Rfun SCxx ex3depsPriv ex3wfPriv ex3vreqPriv .a).val = .unconstrained
  rw [T1_exists]; rfl

/-! ## Header-only consumers (D7/D13)

A target that compiles nothing (`langs(c)` empty: both levels `none`) is
vacuously compatible on every outgoing edge; its dependencies constrain
downstream compiling consumers through public-edge propagation (D10)
instead. -/

theorem headerOnly_consumer_vacuous :
    ∀ (rc : Req CLevel) (rcxx : Req CxxLevel), edgeCompat {} rc rcxx = true := by decide

/-! ## Example 4: mixed-language consumer -/

def ex4M : Consumer := { lvlC := some .c11, lvlCxx := some .cxx20 }
def ex4W : Attrs := { kind := .compiled, implC := some .c17, declC := .declaredMin .c17 }

example : reqOfC ex4W = .atLeast .c17 := rfl
/-- D9 row 5: no C++ implementation, no declaration - the permissive
C-to-C++ default. -/
example : reqOfCxx ex4W = .unconstrained := rfl

/-- The C conjunct fails (`c11 < c17`), so the edge is incompatible even
though the C++ conjunct holds. -/
example : edgeCompat ex4M (reqOfC ex4W) (reqOfCxx ex4W) = false := by decide

/-- A C++-only consumer takes only the C++ conjunct and passes. -/
example : edgeCompat { lvlCxx := some .cxx20 } (reqOfC ex4W) (reqOfCxx ex4W) = true := by
  decide

/-- The strict opposite direction (D9 row 6): a compiled C++ library with no
C interface declaration is forbidden to C consumers at every C level. -/
def ex4V : Attrs := { kind := .compiled, implCxx := some .cxx20 }
example : reqOfC ex4V = .forbidden := rfl
example : ∀ lvl : CLevel, satC lvl (reqOfC ex4V) = false := by decide

/-! ## Example 5: header-only inference -/

def ex5H : Attrs := { kind := .headerOnly, implCxx := some .cxx20 }

/-- D9 row 3: the implementation standard is inferred as the interface
minimum. -/
example : reqOfCxx ex5H = .atLeast .cxx20 := rfl
example : satCxx .cxx17 (reqOfCxx ex5H) = false := by decide

/-- The explicit declaration wins over inference (D9 row 2 preempts row 3),
and the move is a relaxation - it widens the accepted set. -/
def ex5H' : Attrs := { ex5H with declCxx := .declaredMin .cxx17 }
example : reqOfCxx ex5H' = .atLeast .cxx17 := rfl
example : satCxx .cxx17 (reqOfCxx ex5H') = true := by decide
example : leCxx (reqOfCxx ex5H') (reqOfCxx ex5H) = true := by decide

/-! ## Example 6: a bounded interface and the empty intersection -/

/-- A library whose public headers use a construct C++17 removed: the
author declares the honest bounded range `{ min = "c++11", max = "c++14" }`
(D9 row 2, bounded shape). -/
def ex6G : Attrs :=
  { kind := .compiled, implCxx := some .cxx14, declCxx := .declaredRange .cxx11 .cxx14 }

example : reqOfCxx ex6G = .bounded .cxx11 .cxx14 := rfl

/-- A c++17 consumer sits above the cap - and raising cannot help, the
reversal L6 records; a consumer inside the range passes. -/
example : satCxx .cxx17 (reqOfCxx ex6G) = false := by decide
example : satCxx .cxx26 (reqOfCxx ex6G) = false := by decide
example : satCxx .cxx14 (reqOfCxx ex6G) = true := by decide

/-- An aggregator joining a c++20 floor with the c++11..c++14 cap hits the
empty intersection: `forbidden`, unsatisfiable at every level (D4). -/
example : joinCxx (.atLeast .cxx20) (reqOfCxx ex6G) = .forbidden := by decide
example : ∀ lvl : CxxLevel,
    satCxx lvl (joinCxx (.atLeast .cxx20) (reqOfCxx ex6G)) = false := by decide

/-- Defensive normalization: an (unrepresentable-in-manifests) empty
declared range lands as `forbidden`, keeping `ReqOf` total and valid. -/
example : reqOfCxx { kind := .compiled, declCxx := .declaredRange .cxx20 .cxx11 }
    = .forbidden := rfl

end StdCompat
