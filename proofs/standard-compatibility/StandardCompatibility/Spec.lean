/-
The concrete model of docs/design/standard-compatibility/spec.md:
level types (D2), the interval requirement domain with its denotational
strictness preorder and intersection join (D3, D4), satisfies (D11, D12),
ReqOf (D9), consumers and edge compatibility (D7, D13), viability (D14),
and every finite-domain lemma L1-L6 by `decide` (the spec's appendix notes
all these are checkable by exhaustive enumeration; `decide` is exactly
that, kernel-checked).

The requirement type contains bounded shapes `bounded a b` without a
baked-in `a <= b` invariant (the manifest layer rejects empty declared
ranges); `valid` captures the invariant, the join preserves it
(`L2_closed_*`), and the semilattice instances feeding the generic graph
layer live on the subtype of valid requirements, mirroring the
implementation's constructed-validated `Requirement`.

Statements quantified over the level or requirement domains use the
Boolean denotational order `leB` / `satisfies`; the bridge lemmas
`SC_le_sat` / `SCxx_le_sat` connect the semilattice order used by the
generic graph layer back to it.
-/

import StandardCompatibility.Graph

set_option maxRecDepth 8192

namespace StdCompat

/-! ## D2: levels (chronological order; `98 < 11` for C++, `89 < 11` for C) -/

inductive CLevel : Type
  | c89 | c99 | c11 | c17 | c23
  deriving DecidableEq, Repr

inductive CxxLevel : Type
  | cxx98 | cxx11 | cxx14 | cxx17 | cxx20 | cxx23 | cxx26
  deriving DecidableEq, Repr

/-- Position in the chronological chain. -/
def CLevel.rank : CLevel → Nat
  | .c89 => 0 | .c99 => 1 | .c11 => 2 | .c17 => 3 | .c23 => 4

def CxxLevel.rank : CxxLevel → Nat
  | .cxx98 => 0 | .cxx11 => 1 | .cxx14 => 2 | .cxx17 => 3
  | .cxx20 => 4 | .cxx23 => 5 | .cxx26 => 6

def CLevel.leB (a b : CLevel) : Bool := decide (a.rank ≤ b.rank)
def CxxLevel.leB (a b : CxxLevel) : Bool := decide (a.rank ≤ b.rank)

def CLevel.maxL (a b : CLevel) : CLevel := if a.leB b then b else a
def CxxLevel.maxL (a b : CxxLevel) : CxxLevel := if a.leB b then b else a

def CLevel.minL (a b : CLevel) : CLevel := if a.leB b then a else b
def CxxLevel.minL (a b : CxxLevel) : CxxLevel := if a.leB b then a else b

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

/-! ## D3, D4: the interval requirement domain, denotation, strictness, join -/

inductive Req (alpha : Type) : Type
  | unconstrained
  | atLeast (m : alpha)
  | bounded (lo hi : alpha)
  | forbidden
  deriving DecidableEq, Repr

namespace Req

/-- D3's manifest invariant: a bounded shape carries a non-empty range.
The parser rejects empty declared ranges; the model carries the invariant
as a predicate because the inductive type is invariant-free. -/
def valid (leL : alpha → alpha → Bool) : Req alpha → Bool
  | .bounded lo hi => leL lo hi
  | _ => true

/-- D11: `satisfies` at a consumer level - membership in the denotation. -/
def satisfies (leL : alpha → alpha → Bool) (lvl : alpha) : Req alpha → Bool
  | .unconstrained => true
  | .atLeast m => leL m lvl
  | .bounded lo hi => leL lo lvl && leL lvl hi
  | .forbidden => false

/-- D3: the strictness preorder, denotationally - `r ⊑ s` iff every level
satisfying `s` satisfies `r` (reverse inclusion of denotations). -/
def leB [FinEnum alpha] (leL : alpha → alpha → Bool) (r s : Req alpha) : Bool :=
  (FinEnum.elems (alpha := alpha)).all fun lvl =>
    !(satisfies leL lvl s) || satisfies leL lvl r

/-- D4: the structural join - intersect the accepted ranges, collapsing an
empty intersection to `forbidden`. -/
def join (leL : alpha → alpha → Bool) (maxL minL : alpha → alpha → alpha) :
    Req alpha → Req alpha → Req alpha
  | .forbidden, _ => .forbidden
  | _, .forbidden => .forbidden
  | .unconstrained, r => r
  | r, .unconstrained => r
  | .atLeast a, .atLeast b => .atLeast (maxL a b)
  | .atLeast a, .bounded lo hi =>
    if leL (maxL a lo) hi then .bounded (maxL a lo) hi else .forbidden
  | .bounded lo hi, .atLeast a =>
    if leL (maxL a lo) hi then .bounded (maxL a lo) hi else .forbidden
  | .bounded lo hi, .bounded lo' hi' =>
    if leL (maxL lo lo') (minL hi hi') then .bounded (maxL lo lo') (minL hi hi')
    else .forbidden

/-- The denotational order, unfolded: `leB r s` iff every level
satisfying `s` satisfies `r`.  Used to derive the larger-domain lemmas
without kernel enumeration. -/
theorem leB_spec {alpha : Type} [FinEnum alpha] (leL : alpha → alpha → Bool)
    (r s : Req alpha) :
    leB leL r s = true ↔
      ∀ lvl : alpha, satisfies leL lvl s = true → satisfies leL lvl r = true := by
  unfold leB
  rw [List.all_eq_true]
  constructor
  · intro h lvl hs
    have h' := h lvl (FinEnum.complete lvl)
    rw [hs] at h'
    simpa using h'
  · intro h lvl _
    cases hs : satisfies leL lvl s with
    | false => simp
    | true => simp [h lvl hs]

end Req

instance {alpha : Type} [FinEnum alpha] : FinEnum (Req alpha) where
  elems :=
    .unconstrained :: .forbidden :: (FinEnum.elems.map .atLeast)
      ++ FinEnum.elems.flatMap fun lo => FinEnum.elems.map (Req.bounded lo)
  complete := by
    intro x
    cases x with
    | unconstrained => exact List.mem_cons_self ..
    | forbidden => exact List.mem_cons_of_mem _ (List.mem_cons_self ..)
    | atLeast m =>
      exact List.mem_cons_of_mem _ (List.mem_cons_of_mem _
        (List.mem_append.mpr (.inl (List.mem_map.mpr ⟨m, FinEnum.complete m, rfl⟩))))
    | bounded lo hi =>
      exact List.mem_cons_of_mem _ (List.mem_cons_of_mem _
        (List.mem_append.mpr (.inr (List.mem_flatMap.mpr
          ⟨lo, FinEnum.complete lo, List.mem_map.mpr ⟨hi, FinEnum.complete hi, rfl⟩⟩))))

/-! Per-language abbreviations. -/

abbrev validC : Req CLevel → Bool := Req.valid CLevel.leB
abbrev validCxx : Req CxxLevel → Bool := Req.valid CxxLevel.leB
abbrev leC : Req CLevel → Req CLevel → Bool := Req.leB CLevel.leB
abbrev leCxx : Req CxxLevel → Req CxxLevel → Bool := Req.leB CxxLevel.leB
abbrev joinC : Req CLevel → Req CLevel → Req CLevel :=
  Req.join CLevel.leB CLevel.maxL CLevel.minL
abbrev joinCxx : Req CxxLevel → Req CxxLevel → Req CxxLevel :=
  Req.join CxxLevel.leB CxxLevel.maxL CxxLevel.minL
abbrev satC (lvl : CLevel) (r : Req CLevel) : Bool := Req.satisfies CLevel.leB lvl r
abbrev satCxx (lvl : CxxLevel) (r : Req CxxLevel) : Bool := Req.satisfies CxxLevel.leB lvl r

/-! ## L1: `(Req_L, leB)` is a finite preorder, bounded, and NOT total -/

theorem L1_refl_c : ∀ r : Req CLevel, leC r r = true := by decide
theorem L1_trans_c : ∀ r s t : Req CLevel,
    leC r s = true → leC s t = true → leC r t = true := by decide
theorem L1_bot_c : ∀ r : Req CLevel, leC .unconstrained r = true := by decide
theorem L1_top_c : ∀ r : Req CLevel, leC r .forbidden = true := by decide
/-- Antisymmetry holds only up to denotation equality (the `≈` of D3):
mutually related requirements accept exactly the same levels. -/
theorem L1_antisymm_upto_sat_c : ∀ r s : Req CLevel,
    leC r s = true → leC s r = true → ∀ lvl : CLevel, satC lvl r = satC lvl s := by decide
/-- ... and genuinely not on shapes: two distinct shapes denote all of
`Level_C`. -/
theorem L1_antisymm_fails_on_shapes_c :
    leC .unconstrained (.atLeast .c89) = true ∧
      leC (.atLeast .c89) .unconstrained = true ∧
      (Req.atLeast CLevel.c89) ≠ Req.unconstrained := by decide
/-- D3: the strictness order is NOT total - disjoint bounded ranges are
incomparable. -/
theorem L1_not_total_c :
    leC (.bounded .c99 .c11) (.bounded .c17 .c23) = false ∧
      leC (.bounded .c17 .c23) (.bounded .c99 .c11) = false := by decide

theorem L1_refl_cxx : ∀ r : Req CxxLevel, leCxx r r = true := by decide
theorem L1_trans_cxx : ∀ r s t : Req CxxLevel,
    leCxx r s = true → leCxx s t = true → leCxx r t = true := by
  intro r s t hrs hst
  have hs := (Req.leB_spec CxxLevel.leB r s).mp hrs
  have ht := (Req.leB_spec CxxLevel.leB s t).mp hst
  exact (Req.leB_spec CxxLevel.leB r t).mpr fun lvl h => hs lvl (ht lvl h)
theorem L1_bot_cxx : ∀ r : Req CxxLevel, leCxx .unconstrained r = true := by decide
theorem L1_top_cxx : ∀ r : Req CxxLevel, leCxx r .forbidden = true := by decide
theorem L1_not_total_cxx :
    leCxx (.bounded .cxx11 .cxx14) (.bounded .cxx20 .cxx23) = false ∧
      leCxx (.bounded .cxx20 .cxx23) (.bounded .cxx11 .cxx14) = false := by decide
theorem L1_antisymm_upto_sat_cxx : ∀ r s : Req CxxLevel,
    leCxx r s = true → leCxx s r = true → ∀ lvl : CxxLevel, satCxx lvl r = satCxx lvl s := by
  intro r s hrs hsr lvl
  have h1 := (Req.leB_spec CxxLevel.leB r s).mp hrs
  have h2 := (Req.leB_spec CxxLevel.leB s r).mp hsr
  show Req.satisfies CxxLevel.leB lvl r = Req.satisfies CxxLevel.leB lvl s
  cases hr : Req.satisfies CxxLevel.leB lvl r with
  | true => exact (h2 lvl hr).symm
  | false =>
    cases hs : Req.satisfies CxxLevel.leB lvl s with
    | true =>
      have h := h1 lvl hs
      rw [hr] at h
      exact Bool.noConfusion h
    | false => rfl
theorem L1_antisymm_fails_on_shapes_cxx :
    leCxx .unconstrained (.atLeast .cxx98) = true ∧
      leCxx (.atLeast .cxx98) .unconstrained = true ∧
      (Req.atLeast CxxLevel.cxx98) ≠ Req.unconstrained := by decide

/-- The canonical representative of a valid requirement's `≈` class:
drop a bottom-of-chain minimum, fold a top-reaching cap into the
minimum-only shape.  L1's class list says these rewrites - and only
these - identify shapes denotationally. -/
def canonC : Req CLevel → Req CLevel
  | .atLeast m => if m = .c89 then .unconstrained else .atLeast m
  | .bounded lo hi =>
    if hi = .c23 then
      if lo = .c89 then .unconstrained else .atLeast lo
    else .bounded lo hi
  | r => r

def canonCxx : Req CxxLevel → Req CxxLevel
  | .atLeast m => if m = .cxx98 then .unconstrained else .atLeast m
  | .bounded lo hi =>
    if hi = .cxx26 then
      if lo = .cxx98 then .unconstrained else .atLeast lo
    else .bounded lo hi
  | r => r

/-- L1's quotient characterization: two valid shapes are
denotationally equal iff they canonicalize identically - the `≈`
classes are exactly the ones the spec lists. -/
theorem L1_quotient_c : ∀ r s : Req CLevel, validC r = true → validC s = true →
    ((∀ lvl : CLevel, satC lvl r = satC lvl s) ↔ canonC r = canonC s) := by decide
theorem L1_quotient_cxx : ∀ r s : Req CxxLevel, validCxx r = true → validCxx s = true →
    ((∀ lvl : CxxLevel, satCxx lvl r = satCxx lvl s) ↔ canonCxx r = canonCxx s) := by decide

/-- The class L1 displays: `unconstrained`, `[cxx98, ↑]`, and
`[cxx98, cxx26]` all denote every current C++ level. -/
example : canonCxx (.atLeast .cxx98) = .unconstrained ∧
    canonCxx (.bounded .cxx98 .cxx26) = .unconstrained := by decide

/-! ## L3: the strictness order is denotational (definitional here) -/

theorem L3_denotational_c (r s : Req CLevel) :
    leC r s = true ↔ ∀ lvl : CLevel, satC lvl s = true → satC lvl r = true :=
  Req.leB_spec CLevel.leB r s
theorem L3_denotational_cxx (r s : Req CxxLevel) :
    leCxx r s = true ↔ ∀ lvl : CxxLevel, satCxx lvl s = true → satCxx lvl r = true :=
  Req.leB_spec CxxLevel.leB r s

/-! ## L5: antitonicity of `satisfies` -/

theorem L5_c (r s : Req CLevel) (lvl : CLevel)
    (h : leC r s = true) (hs : satC lvl s = true) : satC lvl r = true :=
  (L3_denotational_c r s).mp h lvl hs
theorem L5_cxx (r s : Req CxxLevel) (lvl : CxxLevel)
    (h : leCxx r s = true) (hs : satCxx lvl s = true) : satCxx lvl r = true :=
  (L3_denotational_cxx r s).mp h lvl hs

/-! ## L4: join is intersection of satisfaction sets (total - the
collapse renders an invalid intermediate exactly as its empty denotation) -/

theorem L4_inter_c : ∀ r s : Req CLevel, ∀ lvl : CLevel,
    satC lvl (joinC r s) = (satC lvl r && satC lvl s) := by decide
theorem L4_inter_cxx : ∀ r s : Req CxxLevel, ∀ lvl : CxxLevel,
    satCxx lvl (joinCxx r s) = (satCxx lvl r && satCxx lvl s) := by decide

/-- D4: the empty-intersection collapse is exact - the join is `forbidden`
iff no level satisfies both operands. -/
theorem L4_empty_collapse_cxx : ∀ r s : Req CxxLevel,
    validCxx r = true → validCxx s = true →
    (joinCxx r s = .forbidden ↔
      ∀ lvl : CxxLevel, (satCxx lvl r && satCxx lvl s) = false) := by decide

/-! ## L2: bounded join-semilattice on valid shapes -/

theorem L2_closed_c : ∀ a b : Req CLevel,
    validC a = true → validC b = true → validC (joinC a b) = true := by decide
theorem L2_assoc_c : ∀ a b c : Req CLevel,
    validC a = true → validC b = true → validC c = true →
    joinC (joinC a b) c = joinC a (joinC b c) := by decide
theorem L2_comm_c : ∀ a b : Req CLevel, joinC a b = joinC b a := by decide
theorem L2_idem_c : ∀ a : Req CLevel, validC a = true → joinC a a = a := by decide
theorem L2_id_c : ∀ a : Req CLevel, joinC .unconstrained a = a := by decide
theorem L2_absorb_c : ∀ a : Req CLevel, joinC .forbidden a = .forbidden := by decide
/-- Join is the least upper bound in the denotational order. -/
theorem L2_lub_c : ∀ a b c : Req CLevel,
    leC (joinC a b) c = (leC a c && leC b c) := by decide

theorem L2_closed_cxx : ∀ a b : Req CxxLevel,
    validCxx a = true → validCxx b = true → validCxx (joinCxx a b) = true := by decide
set_option maxHeartbeats 8000000 in
theorem L2_assoc_cxx : ∀ a b c : Req CxxLevel,
    validCxx a = true → validCxx b = true → validCxx c = true →
    joinCxx (joinCxx a b) c = joinCxx a (joinCxx b c) := by decide
theorem L2_comm_cxx : ∀ a b : Req CxxLevel, joinCxx a b = joinCxx b a := by decide
theorem L2_idem_cxx : ∀ a : Req CxxLevel, validCxx a = true → joinCxx a a = a := by decide
theorem L2_id_cxx : ∀ a : Req CxxLevel, joinCxx .unconstrained a = a := by decide
theorem L2_absorb_cxx : ∀ a : Req CxxLevel, joinCxx .forbidden a = .forbidden := by decide
/-- Boolean extensionality over the two-point lattice, for the derived
lub proofs. -/
theorem bool_ext {a b : Bool} (h : a = true ↔ b = true) : a = b := by
  cases a <;> cases b <;> simp_all

theorem L2_lub_cxx (a b c : Req CxxLevel) :
    leCxx (joinCxx a b) c = (leCxx a c && leCxx b c) := by
  apply bool_ext
  rw [Bool.and_eq_true]
  constructor
  · intro h
    constructor <;> refine (L3_denotational_cxx _ _).mpr fun lvl hlvl => ?_
    · have hj := (L3_denotational_cxx _ _).mp h lvl hlvl
      rw [L4_inter_cxx, Bool.and_eq_true] at hj
      exact hj.1
    · have hj := (L3_denotational_cxx _ _).mp h lvl hlvl
      rw [L4_inter_cxx, Bool.and_eq_true] at hj
      exact hj.2
  · intro h
    refine (L3_denotational_cxx _ _).mpr fun lvl hlvl => ?_
    rw [L4_inter_cxx, Bool.and_eq_true]
    exact ⟨(L3_denotational_cxx _ _).mp h.1 lvl hlvl,
      (L3_denotational_cxx _ _).mp h.2 lvl hlvl⟩

/-! The subtype of valid requirements - the implementation's
constructed-validated `Requirement` - carrying the semilattice instances
that feed the generic graph layer. -/

abbrev VReqC : Type := { r : Req CLevel // validC r = true }
abbrev VReqCxx : Type := { r : Req CxxLevel // validCxx r = true }

def SC : JSL VReqC where
  join a b := ⟨joinC a.val b.val, L2_closed_c _ _ a.property b.property⟩
  bot := ⟨.unconstrained, rfl⟩
  join_assoc a b c :=
    Subtype.ext (L2_assoc_c a.val b.val c.val a.property b.property c.property)
  join_comm a b := Subtype.ext (L2_comm_c a.val b.val)
  join_idem a := Subtype.ext (L2_idem_c a.val a.property)
  bot_join a := Subtype.ext (L2_id_c a.val)

def SCxx : JSL VReqCxx where
  join a b := ⟨joinCxx a.val b.val, L2_closed_cxx _ _ a.property b.property⟩
  bot := ⟨.unconstrained, rfl⟩
  join_assoc a b c :=
    Subtype.ext (L2_assoc_cxx a.val b.val c.val a.property b.property c.property)
  join_comm a b := Subtype.ext (L2_comm_cxx a.val b.val)
  join_idem a := Subtype.ext (L2_idem_cxx a.val a.property)
  bot_join a := Subtype.ext (L2_id_cxx a.val)

/-- Bridge: the semilattice-derived order implies the denotational
strictness order (the converse fails on shapes - `atLeast bot` and
`unconstrained` are denotationally equivalent but join to `atLeast bot`,
not to `unconstrained`). -/
theorem join_eq_le_c : ∀ a b : Req CLevel, joinC a b = b → leC a b = true := by decide
theorem join_eq_le_cxx : ∀ a b : Req CxxLevel, joinCxx a b = b → leCxx a b = true := by decide

theorem SC_le_sat {r s : VReqC} (h : SC.le r s) : leC r.val s.val = true :=
  join_eq_le_c r.val s.val (congrArg Subtype.val h)

theorem SCxx_le_sat {r s : VReqCxx} (h : SCxx.le r s) : leCxx r.val s.val = true :=
  join_eq_le_cxx r.val s.val (congrArg Subtype.val h)

/-! The denotational order is itself join-compatible on the valid
subtype (join stays a least upper bound for `⊑`, L2's lub property
read through L3/L4), so the generic growth theorems instantiate to
the spec's `⊑` - see `T2_growth_denotational_*` below. -/

theorem le_join_left_c (a b : Req CLevel) : leC a (joinC a b) = true :=
  (L3_denotational_c _ _).mpr fun lvl h => by
    rw [L4_inter_c, Bool.and_eq_true] at h
    exact h.1
theorem le_join_right_c (a b : Req CLevel) : leC b (joinC a b) = true :=
  (L3_denotational_c _ _).mpr fun lvl h => by
    rw [L4_inter_c, Bool.and_eq_true] at h
    exact h.2
theorem le_join_left_cxx (a b : Req CxxLevel) : leCxx a (joinCxx a b) = true :=
  (L3_denotational_cxx _ _).mpr fun lvl h => by
    rw [L4_inter_cxx, Bool.and_eq_true] at h
    exact h.1
theorem le_join_right_cxx (a b : Req CxxLevel) : leCxx b (joinCxx a b) = true :=
  (L3_denotational_cxx _ _).mpr fun lvl h => by
    rw [L4_inter_cxx, Bool.and_eq_true] at h
    exact h.2

theorem denotJoinLub_c : JoinLub SC (fun a b : VReqC => leC a.val b.val = true) where
  trans {a b c} hab hbc := L1_trans_c a.val b.val c.val hab hbc
  bot_le a := L1_bot_c a.val
  le_join_left a b := le_join_left_c a.val b.val
  le_join_right a b := le_join_right_c a.val b.val
  join_le {a b c} hac hbc := by
    show leC (joinC a.val b.val) c.val = true
    rw [L2_lub_c, Bool.and_eq_true]
    exact ⟨hac, hbc⟩

theorem denotJoinLub_cxx : JoinLub SCxx (fun a b : VReqCxx => leCxx a.val b.val = true) where
  trans {a b c} hab hbc := L1_trans_cxx a.val b.val c.val hab hbc
  bot_le a := L1_bot_cxx a.val
  le_join_left a b := le_join_left_cxx a.val b.val
  le_join_right a b := le_join_right_cxx a.val b.val
  join_le {a b c} hac hbc := by
    show leCxx (joinCxx a.val b.val) c.val = true
    rw [L2_lub_cxx, Bool.and_eq_true]
    exact ⟨hac, hbc⟩

/-- Spec L7 (subset claim) for the concrete domains, in the
denotational order. -/
theorem L7_subset_denotational_c {l l' : List VReqC} (h : ∀ a ∈ l, a ∈ l') :
    leC (SC.joinList l).val (SC.joinList l').val = true :=
  denotJoinLub_c.L7_subset h
theorem L7_subset_denotational_cxx {l l' : List VReqCxx} (h : ∀ a ∈ l, a ∈ l') :
    leCxx (SCxx.joinList l).val (SCxx.joinList l').val = true :=
  denotJoinLub_cxx.L7_subset h

/-- Spec L7 (pointwise claim) for the concrete domains, in the
denotational order. -/
theorem L7_pointwise_denotational_c (ps : List (VReqC × VReqC))
    (h : ∀ p ∈ ps, leC p.1.val p.2.val = true) :
    leC (SC.joinList (ps.map Prod.fst)).val (SC.joinList (ps.map Prod.snd)).val = true :=
  denotJoinLub_c.L7_pointwise ps h
theorem L7_pointwise_denotational_cxx (ps : List (VReqCxx × VReqCxx))
    (h : ∀ p ∈ ps, leCxx p.1.val p.2.val = true) :
    leCxx (SCxx.joinList (ps.map Prod.fst)).val (SCxx.joinList (ps.map Prod.snd)).val = true :=
  denotJoinLub_cxx.L7_pointwise ps h

/-! ## L6: satisfaction sets are convex, NOT upward closed -/

theorem L6_convex_c : ∀ r : Req CLevel, ∀ lo x hi : CLevel,
    CLevel.leB lo x = true → CLevel.leB x hi = true →
    satC lo r = true → satC hi r = true → satC x r = true := by decide
theorem L6_convex_cxx : ∀ r : Req CxxLevel, ∀ lo x hi : CxxLevel,
    CxxLevel.leB lo x = true → CxxLevel.leB x hi = true →
    satCxx lo r = true → satCxx hi r = true → satCxx x r = true := by decide

/-- The old upward-closure lemma is genuinely gone: raising a consumer
past a bounded requirement's cap breaks satisfaction.  (Minimum-only
shapes, `forbidden` - vacuously - and ranges reaching the top of today's
chain remain upward closed; the spec's L6 remark records the remedy
consequence.) -/
theorem L6_not_upward_closed_cxx :
    satCxx .cxx14 (.bounded .cxx11 .cxx14) = true ∧
      satCxx .cxx17 (.bounded .cxx11 .cxx14) = false := by decide
theorem L6_upward_closed_min_only_cxx : ∀ m lvl lvl' : CxxLevel,
    CxxLevel.leB lvl lvl' = true → satCxx lvl (.atLeast m) = true →
    satCxx lvl' (.atLeast m) = true := by decide

/-- Upward closure of `Sat(r)` in today's chain, as an exhaustive
Boolean check. -/
def upwardClosedC (r : Req CLevel) : Bool :=
  (FinEnum.elems (alpha := CLevel)).all fun lvl =>
    (FinEnum.elems (alpha := CLevel)).all fun lvl' =>
      !(satC lvl r && CLevel.leB lvl lvl') || satC lvl' r

def upwardClosedCxx (r : Req CxxLevel) : Bool :=
  (FinEnum.elems (alpha := CxxLevel)).all fun lvl =>
    (FinEnum.elems (alpha := CxxLevel)).all fun lvl' =>
      !(satCxx lvl r && CxxLevel.leB lvl lvl') || satCxx lvl' r

/-- The shape L6 blames: a bounded range capped below the top of
today's chain. -/
def cappedBelowTopC : Req CLevel → Bool
  | .bounded _ hi => if hi = .c23 then false else true
  | _ => false

def cappedBelowTopCxx : Req CxxLevel → Bool
  | .bounded _ hi => if hi = .cxx26 then false else true
  | _ => false

/-- L6's exact characterization: upward closure fails precisely for
the bounded shapes capped below the top - every other valid shape
(unconstrained, minimum-only, top-reaching ranges, and `forbidden`
vacuously) stays upward closed on today's chain. -/
theorem L6_upward_closed_iff_c : ∀ r : Req CLevel,
    validC r = true → upwardClosedC r = !cappedBelowTopC r := by decide
theorem L6_upward_closed_iff_cxx : ∀ r : Req CxxLevel,
    validCxx r = true → upwardClosedCxx r = !cappedBelowTopCxx r := by decide

/-- Instances the spec's L6 discussion names: the failure exists in C
too, a top-reaching C range stays upward closed today, and
`forbidden` is upward closed vacuously. -/
example : satC .c11 (.bounded .c99 .c11) = true ∧
    satC .c17 (.bounded .c99 .c11) = false ∧
    upwardClosedC (.bounded .c17 .c23) = true ∧
    upwardClosedC .forbidden = true := by decide

/-! ## D6, D9: target attributes and `ReqOf` -/

inductive Kind : Type
  | compiled | headerOnly
  deriving DecidableEq, Repr

inductive IfaceDecl (alpha : Type) : Type
  | declaredMin (m : alpha)
  | declaredRange (lo hi : alpha)
  | declaredNone
  | absent
  deriving DecidableEq, Repr

/-- D6: the resolved per-target attributes.  `implC` / `implCxx` obey D6's
population contract: they are `some` exactly when the target itself
implements the language (source-backed for compiled targets, target-declared
for header-only ones) - a package-level implementation default alone never
populates them.  The manifest layer guarantees this - and that declared
ranges are non-empty; the model takes the attributes as given and
defensively normalizes an empty declared range to `forbidden` so `ReqOf`
is total and always valid. -/
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
  | .declaredRange lo hi, _, _ =>
    if CLevel.leB lo hi then .bounded lo hi else .forbidden
  | .absent, some m, .headerOnly => .atLeast m
  | .absent, some _, .compiled => .unconstrained
  | .absent, none, _ => .forbidden

/-- D9 for `L = C++`: rows 1-4, then row 5 (the permissive C-to-C++
default). -/
def reqOfCxx (a : Attrs) : Req CxxLevel :=
  match a.declCxx, a.implCxx, a.kind with
  | .declaredNone, _, _ => .forbidden
  | .declaredMin m, _, _ => .atLeast m
  | .declaredRange lo hi, _, _ =>
    if CxxLevel.leB lo hi then .bounded lo hi else .forbidden
  | .absent, some m, .headerOnly => .atLeast m
  | .absent, some _, .compiled => .unconstrained
  | .absent, none, _ => .unconstrained

/-- `ReqOf` lands in the valid subdomain: the only bounded output is
guarded by its own range check. -/
theorem reqOfC_valid (a : Attrs) : validC (reqOfC a) = true := by
  unfold reqOfC
  split <;> first
    | rfl
    | (split <;> simp_all [Req.valid])

theorem reqOfCxx_valid (a : Attrs) : validCxx (reqOfCxx a) = true := by
  unfold reqOfCxx
  split <;> first
    | rfl
    | (split <;> simp_all [Req.valid])

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
language, as the DAG fold of the generic layer over the valid subtype. -/
def effectiveReqC {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) (t : T) : Req CLevel :=
  (Rfun SC deps wf fun u => ⟨reqOfC (attrs u), reqOfC_valid (attrs u)⟩ : T → VReqC) t |>.val

def effectiveReqCxx {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) (t : T) : Req CxxLevel :=
  (Rfun SCxx deps wf fun u => ⟨reqOfCxx (attrs u), reqOfCxx_valid (attrs u)⟩ : T → VReqCxx) t
    |>.val

/-! ## T2 in the denotational order (the spec's `⊑`)

The generic `T2_growth` takes its pointwise hypothesis in the
structural order, which is stronger than the spec's: `atLeast bot`
and `unconstrained` are `⊑`-equivalent but structurally unrelated.
These are the faithful readings of the spec's T2 for the concrete
domains, via `T2_growth_of` and the denotational `JoinLub`
instances. -/

theorem T2_growth_denotational_c {T : Type} (deps deps' : T → List T)
    (wf : WellFounded (DepRel deps)) (wf' : WellFounded (DepRel deps'))
    (attrs attrs' : T → Attrs)
    (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t)
    (hreq : ∀ u, leC (reqOfC (attrs u)) (reqOfC (attrs' u)) = true) (t : T) :
    leC (effectiveReqC deps wf attrs t) (effectiveReqC deps' wf' attrs' t) = true :=
  T2_growth_of SC denotJoinLub_c deps deps' wf wf'
    (fun u => ⟨reqOfC (attrs u), reqOfC_valid (attrs u)⟩)
    (fun u => ⟨reqOfC (attrs' u), reqOfC_valid (attrs' u)⟩)
    hdeps hreq t

theorem T2_growth_denotational_cxx {T : Type} (deps deps' : T → List T)
    (wf : WellFounded (DepRel deps)) (wf' : WellFounded (DepRel deps'))
    (attrs attrs' : T → Attrs)
    (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t)
    (hreq : ∀ u, leCxx (reqOfCxx (attrs u)) (reqOfCxx (attrs' u)) = true) (t : T) :
    leCxx (effectiveReqCxx deps wf attrs t) (effectiveReqCxx deps' wf' attrs' t) = true :=
  T2_growth_of SCxx denotJoinLub_cxx deps deps' wf wf'
    (fun u => ⟨reqOfCxx (attrs u), reqOfCxx_valid (attrs u)⟩)
    (fun u => ⟨reqOfCxx (attrs' u), reqOfCxx_valid (attrs' u)⟩)
    hdeps hreq t

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
  T4_soundness SC deps wf _ (fun r => satC lvl r.val = true)
    (fun {a b} hle hb => L5_c a.val b.val lvl (SC_le_sat hle) hb) Compiles d A hcompat

theorem T4_soundness_cxx {T : Type} (deps : T → List T) (wf : WellFounded (DepRel deps))
    (attrs : T → Attrs) (lvl : CxxLevel) (Compiles : T → Prop) (d : T)
    (A : ∀ u ∈ reachList deps wf d, satCxx lvl (reqOfCxx (attrs u)) = true → Compiles u)
    (hcompat : satCxx lvl (effectiveReqCxx deps wf attrs d) = true) :
    ∀ u ∈ reachList deps wf d, Compiles u :=
  T4_soundness SCxx deps wf _ (fun r => satCxx lvl r.val = true)
    (fun {a b} hle hb => L5_cxx a.val b.val lvl (SCxx_le_sat hle) hb) Compiles d A hcompat

end StdCompat
