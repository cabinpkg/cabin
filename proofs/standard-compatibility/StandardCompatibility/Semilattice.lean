/-
Generic bounded join-semilattice layer.

This is the algebra the graph development (Graph.lean) runs on: the spec's
L2 says (Req_L, sqsubseteq, sqcup) is a bounded join-semilattice, and T1/T2
are derived from exactly the associativity, commutativity, idempotence, and
identity properties bundled here.  The
concrete instances for Req over CLevel / CxxLevel are built in Spec.lean by
`decide`.

Also here: `FinEnum`, a minimal finite-enumeration class that makes `decide`
work for statements universally quantified over the finite spec domains.
-/

namespace StdCompat

/-- Minimal finite enumeration: a complete list of the type's elements.
Enough to decide universally quantified propositions over the type. -/
class FinEnum (alpha : Type) where
  elems : List alpha
  complete : ∀ x : alpha, x ∈ elems

instance {alpha : Type} [FinEnum alpha] (p : alpha → Prop) [DecidablePred p] :
    Decidable (∀ x : alpha, p x) :=
  if h : ∀ x ∈ FinEnum.elems (alpha := alpha), p x then
    .isTrue fun x => h x (FinEnum.complete x)
  else
    .isFalse fun hall => h fun x _ => hall x

/-- A bounded join-semilattice, presented equationally (spec L2): an
associative, commutative, idempotent join with an identity element `bot`.
The order is derived: `le a b := join a b = b`. -/
structure JSL (R : Type) where
  join : R → R → R
  bot : R
  join_assoc : ∀ a b c, join (join a b) c = join a (join b c)
  join_comm : ∀ a b, join a b = join b a
  join_idem : ∀ a, join a a = a
  bot_join : ∀ a, join bot a = a

namespace JSL

variable {R : Type} (S : JSL R)

/-- The derived partial order: `a` is below `b` iff joining changes nothing. -/
def le (a b : R) : Prop := S.join a b = b

theorem le_refl (a : R) : S.le a a := S.join_idem a

theorem le_trans {a b c : R} (h1 : S.le a b) (h2 : S.le b c) : S.le a c := by
  show S.join a c = c
  rw [← h2, ← S.join_assoc, h1]

theorem le_antisymm {a b : R} (h1 : S.le a b) (h2 : S.le b a) : a = b := by
  calc a = S.join b a := h2.symm
    _ = S.join a b := S.join_comm b a
    _ = b := h1

theorem join_bot (a : R) : S.join a S.bot = a := by
  rw [S.join_comm, S.bot_join]

theorem bot_le (a : R) : S.le S.bot a := S.bot_join a

theorem le_join_left (a b : R) : S.le a (S.join a b) := by
  show S.join a (S.join a b) = S.join a b
  rw [← S.join_assoc, S.join_idem]

theorem le_join_right (a b : R) : S.le b (S.join a b) := by
  rw [S.join_comm]
  exact S.le_join_left b a

/-- `join` is the least upper bound. -/
theorem join_le {a b c : R} (h1 : S.le a c) (h2 : S.le b c) : S.le (S.join a b) c := by
  show S.join (S.join a b) c = c
  rw [S.join_assoc, h2, h1]

theorem join_le_join {a a' b b' : R} (ha : S.le a a') (hb : S.le b b') :
    S.le (S.join a b) (S.join a' b') :=
  S.join_le (S.le_trans ha (S.le_join_left a' b')) (S.le_trans hb (S.le_join_right a' b'))

/-- Join of a list of elements (the spec's finite big-join, D4); the empty
join is `bot` (= unconstrained). -/
def joinList : List R → R
  | [] => S.bot
  | a :: l => S.join a (joinList l)

@[simp] theorem joinList_nil : S.joinList [] = S.bot := rfl

@[simp] theorem joinList_cons (a : R) (l : List R) :
    S.joinList (a :: l) = S.join a (S.joinList l) := rfl

theorem le_joinList : ∀ {l : List R} {a : R}, a ∈ l → S.le a (S.joinList l) := by
  intro l
  induction l with
  | nil => intro a h; cases h
  | cons b l ih =>
    intro a h
    cases h with
    | head => exact S.le_join_left _ _
    | tail _ h => exact S.le_trans (ih h) (S.le_join_right _ _)

theorem joinList_le {b : R} : ∀ {l : List R}, (∀ a ∈ l, S.le a b) → S.le (S.joinList l) b := by
  intro l
  induction l with
  | nil => intro _; exact S.bot_le b
  | cons a l ih =>
    intro h
    exact S.join_le (h a (List.mem_cons_self ..)) (ih fun x hx => h x (List.mem_cons_of_mem a hx))

/-- The flattening law used by T1: joining an appended list is the join of
the two joins. -/
theorem joinList_append : ∀ (l l' : List R),
    S.joinList (l ++ l') = S.join (S.joinList l) (S.joinList l') := by
  intro l l'
  induction l with
  | nil => simp [S.bot_join]
  | cons a l ih => simp [ih, S.join_assoc]

/-- Spec L7, first claim (with multisets read as lists): a list join is
monotone under membership inclusion. -/
theorem L7_joinList_mono_subset {l l' : List R} (h : ∀ a ∈ l, a ∈ l') :
    S.le (S.joinList l) (S.joinList l') :=
  S.joinList_le fun a ha => S.le_joinList (h a ha)

/-- Spec L7, second claim: a list join is monotone under pointwise `le`
(the two lists presented as a single list of pairs). -/
theorem L7_joinList_mono_pointwise : ∀ (ps : List (R × R)),
    (∀ p ∈ ps, S.le p.1 p.2) →
    S.le (S.joinList (ps.map Prod.fst)) (S.joinList (ps.map Prod.snd)) := by
  intro ps
  induction ps with
  | nil => intro _; exact S.le_refl _
  | cons p ps ih =>
    intro h
    exact S.join_le_join (h p (List.mem_cons_self ..))
      (ih fun q hq => h q (List.mem_cons_of_mem p hq))

/-- Spec T1 order-independence / L2 multiset independence: list joins depend
only on the set of members - any enumeration order and any multiplicity give
the same result.  This is the confluence of computing R_L in an arbitrary
topological order. -/
theorem joinList_congr_mem {l l' : List R} (h : ∀ a, a ∈ l ↔ a ∈ l') :
    S.joinList l = S.joinList l' :=
  S.le_antisymm
    (S.L7_joinList_mono_subset fun a ha => (h a).mp ha)
    (S.L7_joinList_mono_subset fun a ha => (h a).mpr ha)

end JSL

end StdCompat
