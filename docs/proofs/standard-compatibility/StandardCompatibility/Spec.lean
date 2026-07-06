/-
The concrete model of docs/design/standard-compatibility/spec.md:
level types (D2), the requirement domain and its order/join (D3, D4),
satisfies (D11, D12), ReqOf (D9), consumers and edge compatibility
(D7, D13), viability (D14), and every finite-domain lemma L1-L6 by
`decide` (the spec's appendix notes all these are checkable by exhaustive
enumeration; `decide` is exactly that, kernel-checked).

Statements quantified over the level or requirement domains use the
Boolean order `leB` / `satisfies`; the bridge lemmas `SC_le_iff` /
`SCxx_le_iff` connect them to the semilattice order used by the generic
graph layer.
-/

import StandardCompatibility.Graph

namespace StdCompat

/-! ## D2: levels (chronological order; `98 < 11` for C++, `89 < 11` for C) -/

inductive CLevel : Type
  | c89 | c99 | c11 | c17 | c23
  deriving DecidableEq, Repr

inductive CxxLevel : Type
  | cxx98 | cxx11 | cxx14 | cxx17 | cxx20 | cxx23 | cxx26
  deriving DecidableEq, Repr

/-- Position in the chronological chain (the spec's L1 isomorphism target). -/
def CLevel.rank : CLevel → Nat
  | .c89 => 0 | .c99 => 1 | .c11 => 2 | .c17 => 3 | .c23 => 4

def CxxLevel.rank : CxxLevel → Nat
  | .cxx98 => 0 | .cxx11 => 1 | .cxx14 => 2 | .cxx17 => 3
  | .cxx20 => 4 | .cxx23 => 5 | .cxx26 => 6

def CLevel.leB (a b : CLevel) : Bool := decide (a.rank ≤ b.rank)
def CxxLevel.leB (a b : CxxLevel) : Bool := decide (a.rank ≤ b.rank)

def CLevel.maxL (a b : CLevel) : CLevel := if a.leB b then b else a
def CxxLevel.maxL (a b : CxxLevel) : CxxLevel := if a.leB b then b else a

instance : FinEnum CLevel where
  elems := [.c89, .c99, .c11, .c17, .c23]
  complete := by intro x; cases x <;> decide

instance : FinEnum CxxLevel where
  elems := [.cxx98, .cxx11, .cxx14, .cxx17, .cxx20, .cxx23, .cxx26]
  complete := by intro x; cases x <;> decide

/-- D2: the order is chronological, not numeric (`c++98 < c++11`), and
`c11 < c17` strictly - no equivalence special case. -/
example : CxxLevel.leB .cxx98 .cxx11 = true := by decide
example : CLevel.leB .c11 .c17 = true ∧ CLevel.leB .c17 .c11 = false := by decide

/-! ## D3, D4: the requirement domain, strictness order, join -/

inductive Req (alpha : Type) : Type
  | unconstrained
  | atLeast (m : alpha)
  | forbidden
  deriving DecidableEq, Repr

namespace Req

/-- D3: the strictness order (Boolean form). -/
def leB (leL : alpha → alpha → Bool) : Req alpha → Req alpha → Bool
  | .unconstrained, _ => true
  | _, .forbidden => true
  | .atLeast a, .atLeast b => leL a b
  | _, _ => false

/-- D4: the join (strictness maximum). -/
def join (maxL : alpha → alpha → alpha) : Req alpha → Req alpha → Req alpha
  | .forbidden, _ => .forbidden
  | _, .forbidden => .forbidden
  | .unconstrained, r => r
  | r, .unconstrained => r
  | .atLeast a, .atLeast b => .atLeast (maxL a b)

/-- D11: `satisfies` at a consumer level. -/
def satisfies (leL : alpha → alpha → Bool) (lvl : alpha) : Req alpha → Bool
  | .unconstrained => true
  | .atLeast m => leL m lvl
  | .forbidden => false

end Req

instance {alpha : Type} [FinEnum alpha] : FinEnum (Req alpha) where
  elems := .unconstrained :: .forbidden :: (FinEnum.elems.map .atLeast)
  complete := by
    intro x
    cases x with
    | unconstrained => exact List.mem_cons_self ..
    | forbidden => exact List.mem_cons_of_mem _ (List.mem_cons_self ..)
    | atLeast m =>
      exact List.mem_cons_of_mem _ (List.mem_cons_of_mem _
        (List.mem_map.mpr ⟨m, FinEnum.complete m, rfl⟩))

/-! Per-language abbreviations. -/

abbrev leC : Req CLevel → Req CLevel → Bool := Req.leB CLevel.leB
abbrev leCxx : Req CxxLevel → Req CxxLevel → Bool := Req.leB CxxLevel.leB
abbrev joinC : Req CLevel → Req CLevel → Req CLevel := Req.join CLevel.maxL
abbrev joinCxx : Req CxxLevel → Req CxxLevel → Req CxxLevel := Req.join CxxLevel.maxL
abbrev satC (lvl : CLevel) (r : Req CLevel) : Bool := Req.satisfies CLevel.leB lvl r
abbrev satCxx (lvl : CxxLevel) (r : Req CxxLevel) : Bool := Req.satisfies CxxLevel.leB lvl r

/-! ## L1: `(Req_L, leB)` is a finite chain -/

theorem L1_refl_c : ∀ r : Req CLevel, leC r r = true := by decide
theorem L1_trans_c : ∀ r s t : Req CLevel,
    leC r s = true → leC s t = true → leC r t = true := by decide
theorem L1_antisymm_c : ∀ r s : Req CLevel,
    leC r s = true → leC s r = true → r = s := by decide
theorem L1_total_c : ∀ r s : Req CLevel, leC r s = true ∨ leC s r = true := by decide
theorem L1_bot_c : ∀ r : Req CLevel, leC .unconstrained r = true := by decide
theorem L1_top_c : ∀ r : Req CLevel, leC r .forbidden = true := by decide

theorem L1_refl_cxx : ∀ r : Req CxxLevel, leCxx r r = true := by decide
theorem L1_trans_cxx : ∀ r s t : Req CxxLevel,
    leCxx r s = true → leCxx s t = true → leCxx r t = true := by decide
theorem L1_antisymm_cxx : ∀ r s : Req CxxLevel,
    leCxx r s = true → leCxx s r = true → r = s := by decide
theorem L1_total_cxx : ∀ r s : Req CxxLevel, leCxx r s = true ∨ leCxx s r = true := by decide
theorem L1_bot_cxx : ∀ r : Req CxxLevel, leCxx .unconstrained r = true := by decide
theorem L1_top_cxx : ∀ r : Req CxxLevel, leCxx r .forbidden = true := by decide

/-! ## L2: bounded join-semilattice (associative, commutative, idempotent,
identity, absorbing) -/

theorem L2_assoc_c : ∀ a b c : Req CLevel,
    joinC (joinC a b) c = joinC a (joinC b c) := by decide
theorem L2_comm_c : ∀ a b : Req CLevel, joinC a b = joinC b a := by decide
theorem L2_idem_c : ∀ a : Req CLevel, joinC a a = a := by decide
theorem L2_id_c : ∀ a : Req CLevel, joinC .unconstrained a = a := by decide
theorem L2_absorb_c : ∀ a : Req CLevel, joinC .forbidden a = .forbidden := by decide
theorem L2_lub_c : ∀ a b c : Req CLevel,
    leC (joinC a b) c = (leC a c && leC b c) := by decide

theorem L2_assoc_cxx : ∀ a b c : Req CxxLevel,
    joinCxx (joinCxx a b) c = joinCxx a (joinCxx b c) := by decide
theorem L2_comm_cxx : ∀ a b : Req CxxLevel, joinCxx a b = joinCxx b a := by decide
theorem L2_idem_cxx : ∀ a : Req CxxLevel, joinCxx a a = a := by decide
theorem L2_id_cxx : ∀ a : Req CxxLevel, joinCxx .unconstrained a = a := by decide
theorem L2_absorb_cxx : ∀ a : Req CxxLevel, joinCxx .forbidden a = .forbidden := by decide
theorem L2_lub_cxx : ∀ a b c : Req CxxLevel,
    leCxx (joinCxx a b) c = (leCxx a c && leCxx b c) := by decide

/-- The semilattice instances feeding the generic graph layer (L2). -/
def SC : JSL (Req CLevel) :=
  ⟨joinC, .unconstrained, L2_assoc_c, L2_comm_c, L2_idem_c, L2_id_c⟩

def SCxx : JSL (Req CxxLevel) :=
  ⟨joinCxx, .unconstrained, L2_assoc_cxx, L2_comm_cxx, L2_idem_cxx, L2_id_cxx⟩

/-- Bridge: the semilattice-derived order coincides with D3's `leB`. -/
theorem SC_le_iff : ∀ r s : Req CLevel, SC.le r s ↔ leC r s = true := by
  have h : ∀ r s : Req CLevel, joinC r s = s ↔ leC r s = true := by decide
  exact h

theorem SCxx_le_iff : ∀ r s : Req CxxLevel, SCxx.le r s ↔ leCxx r s = true := by
  have h : ∀ r s : Req CxxLevel, joinCxx r s = s ↔ leCxx r s = true := by decide
  exact h

/-! ## L3: semantic characterization (with its single degenerate pair) -/

theorem L3_sound_c : ∀ r s : Req CLevel, leC r s = true →
    ∀ lvl : CLevel, satC lvl s = true → satC lvl r = true := by decide
theorem L3_complete_except_c : ∀ r s : Req CLevel,
    (∀ lvl : CLevel, satC lvl s = true → satC lvl r = true) →
    leC r s = true ∨ (r = .atLeast .c89 ∧ s = .unconstrained) := by decide
theorem L3_exception_genuine_c :
    (∀ lvl : CLevel, satC lvl .unconstrained = satC lvl (.atLeast .c89)) ∧
      leC (.atLeast .c89) .unconstrained = false := by decide
theorem L3_sat_eq_iff_c : ∀ r s : Req CLevel,
    (∀ lvl : CLevel, satC lvl r = satC lvl s) ↔
      (r = s ∨ (r = .unconstrained ∧ s = .atLeast .c89) ∨
        (r = .atLeast .c89 ∧ s = .unconstrained)) := by decide

theorem L3_sound_cxx : ∀ r s : Req CxxLevel, leCxx r s = true →
    ∀ lvl : CxxLevel, satCxx lvl s = true → satCxx lvl r = true := by decide
theorem L3_complete_except_cxx : ∀ r s : Req CxxLevel,
    (∀ lvl : CxxLevel, satCxx lvl s = true → satCxx lvl r = true) →
    leCxx r s = true ∨ (r = .atLeast .cxx98 ∧ s = .unconstrained) := by decide
theorem L3_exception_genuine_cxx :
    (∀ lvl : CxxLevel, satCxx lvl .unconstrained = satCxx lvl (.atLeast .cxx98)) ∧
      leCxx (.atLeast .cxx98) .unconstrained = false := by decide
theorem L3_sat_eq_iff_cxx : ∀ r s : Req CxxLevel,
    (∀ lvl : CxxLevel, satCxx lvl r = satCxx lvl s) ↔
      (r = s ∨ (r = .unconstrained ∧ s = .atLeast .cxx98) ∨
        (r = .atLeast .cxx98 ∧ s = .unconstrained)) := by decide

/-! ## L4: join is intersection of satisfaction sets -/

theorem L4_inter_c : ∀ r s : Req CLevel, ∀ lvl : CLevel,
    satC lvl (joinC r s) = (satC lvl r && satC lvl s) := by decide
theorem L4_inter_cxx : ∀ r s : Req CxxLevel, ∀ lvl : CxxLevel,
    satCxx lvl (joinCxx r s) = (satCxx lvl r && satCxx lvl s) := by decide

/-! ## L5: antitonicity of `satisfies` -/

theorem L5_c : ∀ r s : Req CLevel, ∀ lvl : CLevel,
    leC r s = true → satC lvl s = true → satC lvl r = true := by decide
theorem L5_cxx : ∀ r s : Req CxxLevel, ∀ lvl : CxxLevel,
    leCxx r s = true → satCxx lvl s = true → satCxx lvl r = true := by decide

/-! ## L6: satisfaction sets are upward closed -/

theorem L6_upward_c : ∀ r : Req CLevel, ∀ lvl lvl' : CLevel,
    CLevel.leB lvl lvl' = true → satC lvl r = true → satC lvl' r = true := by decide
theorem L6_upward_cxx : ∀ r : Req CxxLevel, ∀ lvl lvl' : CxxLevel,
    CxxLevel.leB lvl lvl' = true → satCxx lvl r = true → satCxx lvl' r = true := by decide

/-! ## D6, D9: target attributes and `ReqOf` -/

inductive Kind : Type
  | compiled | headerOnly
  deriving DecidableEq, Repr

inductive IfaceDecl (alpha : Type) : Type
  | declaredMin (m : alpha)
  | declaredNone
  | absent
  deriving DecidableEq, Repr

/-- D6: the resolved per-target attributes.  `implC` / `implCxx` obey D6's
population contract: they are `some` exactly when the target itself
implements the language (source-backed for compiled targets, target-declared
for header-only ones) - a package-level implementation default alone never
populates them.  The manifest layer guarantees this; the model takes the
attributes as given. -/
structure Attrs where
  kind : Kind
  implC : Option CLevel := none
  implCxx : Option CxxLevel := none
  declC : IfaceDecl CLevel := .absent
  declCxx : IfaceDecl CxxLevel := .absent
  deriving DecidableEq, Repr

/-- D9 for `L = C`: rows 1-4, then row 6 (the strict C++-to-C default). -/
def reqOfC (a : Attrs) : Req CLevel :=
  match a.declC, a.implC, a.kind with
  | .declaredNone, _, _ => .forbidden
  | .declaredMin m, _, _ => .atLeast m
  | .absent, some m, .headerOnly => .atLeast m
  | .absent, some _, .compiled => .unconstrained
  | .absent, none, _ => .forbidden

/-- D9 for `L = C++`: rows 1-4, then row 5 (the permissive C-to-C++
default). -/
def reqOfCxx (a : Attrs) : Req CxxLevel :=
  match a.declCxx, a.implCxx, a.kind with
  | .declaredNone, _, _ => .forbidden
  | .declaredMin m, _, _ => .atLeast m
  | .absent, some m, .headerOnly => .atLeast m
  | .absent, some _, .compiled => .unconstrained
  | .absent, none, _ => .unconstrained

/-! ## D7, D13, D14: consumers, edge compatibility, viability

A consumer's `none` in a language means it does not compile that language;
D13's conjunction ranges over compiled languages only, so `none` imposes
nothing (`satOpt` is `true`).  A header-only consumer has both levels
`none` and satisfies every edge vacuously (D7/D13; see
`headerOnly_consumer_vacuous` in Examples.lean). -/

structure Consumer where
  lvlC : Option CLevel := none
  lvlCxx : Option CxxLevel := none
  deriving DecidableEq, Repr

def satOptC : Option CLevel → Req CLevel → Bool
  | none, _ => true
  | some lvl, r => satC lvl r

def satOptCxx : Option CxxLevel → Req CxxLevel → Bool
  | none, _ => true
  | some lvl, r => satCxx lvl r

/-- D13: edge compatibility, the conjunction over the consumer's
languages, applied to the dependency's effective requirements. -/
def edgeCompat (c : Consumer) (rc : Req CLevel) (rcxx : Req CxxLevel) : Bool :=
  satOptC c.lvlC rc && satOptCxx c.lvlCxx rcxx

/-- D14: a package version is viable iff every edge resolving to it is
compatible; `edges` lists the (consumer, dependency-target) edges that
resolve to the version, and `RC`/`RCxx` are the effective requirements. -/
def viable {T : Type} (RC : T → Req CLevel) (RCxx : T → Req CxxLevel)
    (edges : List (Consumer × T)) : Bool :=
  edges.all fun e => edgeCompat e.1 (RC e.2) (RCxx e.2)

/-- D10 instantiated: the effective requirement of every target, per
language, as the DAG fold of the generic layer. -/
def effectiveReqC {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) : T → Req CLevel :=
  Rfun SC deps wf fun t => reqOfC (attrs t)

def effectiveReqCxx {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) : T → Req CxxLevel :=
  Rfun SCxx deps wf fun t => reqOfCxx (attrs t)

/-! ## T3: decidability (complexity is not modeled; see README) -/

def T3_satisfies_decidable (lvl : CLevel) (r : Req CLevel) :
    Decidable (satC lvl r = true) := inferInstance
def T3_edge_decidable (c : Consumer) (rc : Req CLevel) (rcxx : Req CxxLevel) :
    Decidable (edgeCompat c rc rcxx = true) := inferInstance
def T3_viability_decidable {T : Type} (RC : T → Req CLevel)
    (RCxx : T → Req CxxLevel) (edges : List (Consumer × T)) :
    Decidable (viable RC RCxx edges = true) := inferInstance

/-! ## C3: viable versions can only shrink -/

theorem satOptC_antitone (o : Option CLevel) (r r' : Req CLevel)
    (h : leC r r' = true) (hs : satOptC o r' = true) : satOptC o r = true := by
  cases o with
  | none => rfl
  | some lvl => exact L5_c r r' lvl h hs

theorem satOptCxx_antitone (o : Option CxxLevel) (r r' : Req CxxLevel)
    (h : leCxx r r' = true) (hs : satOptCxx o r' = true) : satOptCxx o r = true := by
  cases o with
  | none => rfl
  | some lvl => exact L5_cxx r r' lvl h hs

/-- Spec C3: if every effective requirement grows (primed assignment), every
version viable under the primed assignment was already viable before -
growing requirements can only shrink the viable set. -/
theorem C3_viable_shrink {T : Type} (RC RC' : T → Req CLevel)
    (RCxx RCxx' : T → Req CxxLevel) (edges : List (Consumer × T))
    (hC : ∀ d, leC (RC d) (RC' d) = true)
    (hCxx : ∀ d, leCxx (RCxx d) (RCxx' d) = true)
    (hv : viable RC' RCxx' edges = true) : viable RC RCxx edges = true := by
  unfold viable at hv ⊢
  rw [List.all_eq_true] at hv ⊢
  intro e he
  have h' := hv e he
  unfold edgeCompat at h' ⊢
  rw [Bool.and_eq_true] at h' ⊢
  exact ⟨satOptC_antitone _ _ _ (hC e.2) h'.1,
    satOptCxx_antitone _ _ _ (hCxx e.2) h'.2⟩

/-! ## T4: conditional semantic soundness, per language -/

theorem T4_soundness_c {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) (lvl : CLevel) (Compiles : T → Prop) (d : T)
    (A : ∀ u ∈ reachList deps wf d, satC lvl (reqOfC (attrs u)) = true → Compiles u)
    (hcompat : satC lvl (effectiveReqC deps wf attrs d) = true) :
    ∀ u ∈ reachList deps wf d, Compiles u :=
  T4_soundness SC deps wf _ (fun r => satC lvl r = true)
    (fun {a b} hle hb => L5_c a b lvl ((SC_le_iff a b).mp hle) hb) Compiles d A hcompat

theorem T4_soundness_cxx {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) (lvl : CxxLevel) (Compiles : T → Prop) (d : T)
    (A : ∀ u ∈ reachList deps wf d, satCxx lvl (reqOfCxx (attrs u)) = true → Compiles u)
    (hcompat : satCxx lvl (effectiveReqCxx deps wf attrs d) = true) :
    ∀ u ∈ reachList deps wf d, Compiles u :=
  T4_soundness SCxx deps wf _ (fun r => satCxx lvl r = true)
    (fun {a b} hle hb => L5_cxx a b lvl ((SCxx_le_iff a b).mp hle) hb) Compiles d A hcompat

end StdCompat
