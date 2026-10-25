/-
The dependency-graph layer: R_L as a fold over a finite DAG (spec D10),
its well-definedness (T1), growth (T2, C1, C2), and the generic form of
conditional semantic soundness (T4).

The spec's "finite DAG" hypothesis enters as well-foundedness of the
public-dependency relation `DepRel deps` (on a finite acyclic graph the
relation is well-founded; well-foundedness is exactly what the spec's
longest-path height function provides).  `deps t` lists the public
dependencies of `t`, so finite branching is structural.
-/

import StandardCompatibility.Semilattice

namespace StdCompat

/-- `DepRel deps d t` holds when `d` is a (public) dependency of `t`. -/
def DepRel {T : Type} (deps : T → List T) (d t : T) : Prop := d ∈ deps t

section Helpers

variable {R : Type} {alpha beta : Type}

/-- Join over a list with membership evidence, so it can be used under
`WellFounded.fix` (the recursive call needs the `DepRel` witness). -/
def joinListMem (S : JSL R) : (l : List alpha) → ((a : alpha) → a ∈ l → R) → R
  | [], _ => S.bot
  | a :: l, f =>
    S.join (f a (List.mem_cons_self ..))
      (joinListMem S l fun d h => f d (List.mem_cons_of_mem a h))

theorem joinListMem_eq (S : JSL R) (l : List alpha) (g : alpha → R) :
    joinListMem S l (fun a _ => g a) = S.joinList (l.map g) := by
  induction l with
  | nil => rfl
  | cons a l ih => simp [joinListMem, ih]

/-- `flatMap` with membership evidence, for the same reason. -/
def flatMapMem : (l : List alpha) → ((a : alpha) → a ∈ l → List beta) → List beta
  | [], _ => []
  | a :: l, f =>
    f a (List.mem_cons_self ..) ++ flatMapMem l fun d h => f d (List.mem_cons_of_mem a h)

theorem flatMapMem_eq (l : List alpha) (g : alpha → List beta) :
    flatMapMem l (fun a _ => g a) = l.flatMap g := by
  induction l with
  | nil => rfl
  | cons a l ih => simp [flatMapMem, ih]

theorem mapCongrMem {l : List alpha} {f g : alpha → beta} (h : ∀ a ∈ l, f a = g a) :
    l.map f = l.map g := by
  induction l with
  | nil => rfl
  | cons a l ih =>
    rw [List.map_cons, List.map_cons, h a (List.mem_cons_self ..),
      ih fun b hb => h b (List.mem_cons_of_mem a hb)]

theorem joinList_map_flatMap (S : JSL R) (l : List alpha) (g : alpha → List beta)
    (f : beta → R) :
    S.joinList ((l.flatMap g).map f)
      = S.joinList (l.map fun a => S.joinList ((g a).map f)) := by
  induction l with
  | nil => rfl
  | cons a l ih =>
    rw [List.flatMap_cons, List.map_append, S.joinList_append, ih, List.map_cons,
      S.joinList_cons]

end Helpers

section Graph

variable {T : Type} {R : Type}
variable (S : JSL R) (deps : T → List T) (wf : WellFounded (DepRel deps)) (reqOf : T → R)

/-- Spec D10: the effective requirement, defined as a fold over the finite
DAG of public dependencies.  Termination is the well-foundedness hypothesis
(spec T1's height argument). -/
def Rfun : T → R :=
  wf.fix fun t rec => S.join (reqOf t) (joinListMem S (deps t) fun d hd => rec d hd)

/-- Spec T1 (existence): `Rfun` satisfies the defining recursion of D10. -/
theorem T1_exists (t : T) :
    Rfun S deps wf reqOf t
      = S.join (reqOf t) (S.joinList ((deps t).map (Rfun S deps wf reqOf))) := by
  unfold Rfun
  rw [WellFounded.fix_eq]
  exact congrArg _ (joinListMem_eq S (deps t) _)

include wf in
/-- Spec T1 (uniqueness): any two solutions of the D10 recursion agree. -/
theorem T1_unique (f g : T → R)
    (hf : ∀ t, f t = S.join (reqOf t) (S.joinList ((deps t).map f)))
    (hg : ∀ t, g t = S.join (reqOf t) (S.joinList ((deps t).map g))) : f = g := by
  funext t
  refine wf.induction (C := fun x => f x = g x) t fun x ih => ?_
  rw [hf x, hg x, mapCongrMem fun d hd => ih d hd]

/-- Spec D5: the public reachability list (`PubReach` as a list; membership
is reachability, duplicates are irrelevant by `joinList_congr_mem`). -/
def reachList : T → List T :=
  wf.fix fun t rec => t :: flatMapMem (deps t) fun d hd => rec d hd

theorem reachList_eq (t : T) :
    reachList deps wf t = t :: (deps t).flatMap (reachList deps wf) := by
  unfold reachList
  rw [WellFounded.fix_eq]
  exact congrArg _ (flatMapMem_eq (deps t) _)

theorem mem_reachList_self (t : T) : t ∈ reachList deps wf t := by
  rw [reachList_eq]
  exact List.mem_cons_self ..

/-- Spec T1 (closed form): `R_L(t)` is the join of `ReqOf` over the public
reachability set.  Duplicate visits along diamond paths are harmless by
idempotence (`joinList_congr_mem`); this equation holds for the raw
duplicate-carrying enumeration already. -/
theorem T1_closed_form (t : T) :
    Rfun S deps wf reqOf t = S.joinList ((reachList deps wf t).map reqOf) := by
  refine wf.induction
    (C := fun x => Rfun S deps wf reqOf x = S.joinList ((reachList deps wf x).map reqOf))
    t fun x ih => ?_
  rw [T1_exists, reachList_eq, List.map_cons, S.joinList_cons]
  congr 1
  rw [joinList_map_flatMap]
  exact congrArg S.joinList (mapCongrMem fun d hd => ih d hd)

/-- Spec T1 (order-independence, list form): the closed-form join is
invariant under any re-enumeration of the reachability set - hence any
topological computation order yields the same `R_L`. -/
theorem T1_order_independence (t : T) {l : List T}
    (h : ∀ u, u ∈ l ↔ u ∈ reachList deps wf t) :
    Rfun S deps wf reqOf t = S.joinList (l.map reqOf) := by
  rw [T1_closed_form]
  refine S.joinList_congr_mem fun a => ?_
  constructor
  · intro ha
    obtain ⟨u, hu, rfl⟩ := List.mem_map.mp ha
    exact List.mem_map.mpr ⟨u, (h u).mpr hu, rfl⟩
  · intro ha
    obtain ⟨u, hu, rfl⟩ := List.mem_map.mp ha
    exact List.mem_map.mpr ⟨u, (h u).mp hu, rfl⟩

/-- T4's key step: everything publicly reachable is below the effective
requirement (join upper bound, via the closed form). -/
theorem le_reqOf_of_mem_reach {u d : T} (hu : u ∈ reachList deps wf d) :
    S.le (reqOf u) (Rfun S deps wf reqOf d) := by
  rw [T1_closed_form]
  exact S.le_joinList (List.mem_map.mpr ⟨u, hu, rfl⟩)

end Graph

section Growth

variable {T : Type} {R : Type} (S : JSL R)

/-- A sub-graph of a well-founded graph is well-founded (used to transport
the DAG hypothesis in C1/C2). -/
theorem wf_of_sub {deps deps' : T → List T} (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t)
    (wf' : WellFounded (DepRel deps')) : WellFounded (DepRel deps) :=
  Subrelation.wf (fun {d t} h => hdeps t d h) wf'

/-- Spec T2 (growth): more public edges and pointwise-larger `ReqOf` can
only raise the effective requirement. -/
theorem T2_growth (deps deps' : T → List T)
    (wf : WellFounded (DepRel deps)) (wf' : WellFounded (DepRel deps'))
    (reqOf reqOf' : T → R)
    (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t)
    (hreq : ∀ t, S.le (reqOf t) (reqOf' t)) (t : T) :
    S.le (Rfun S deps wf reqOf t) (Rfun S deps' wf' reqOf' t) := by
  refine wf'.induction
    (C := fun x => S.le (Rfun S deps wf reqOf x) (Rfun S deps' wf' reqOf' x))
    t fun x ih => ?_
  rw [T1_exists S deps wf reqOf x, T1_exists S deps' wf' reqOf' x]
  refine S.join_le (S.le_trans (hreq x) (S.le_join_left _ _)) ?_
  refine S.le_trans ?_ (S.le_join_right _ _)
  refine S.joinList_le fun a ha => ?_
  obtain ⟨d, hd, rfl⟩ := List.mem_map.mp ha
  exact S.le_trans (ih d (hdeps x d hd))
    (S.le_joinList (List.mem_map.mpr ⟨d, hdeps x d hd, rfl⟩))

/-- Spec T2 (growth) against any join-compatible preorder: the form
that instantiates to the spec's **denotational** strictness order
(`T2_growth_denotational_*` in Spec.lean), whose pointwise hypothesis
is weaker than the structural one of `T2_growth`. -/
theorem T2_growth_of {le' : R → R → Prop} (H : JoinLub S le')
    (deps deps' : T → List T)
    (wf : WellFounded (DepRel deps)) (wf' : WellFounded (DepRel deps'))
    (reqOf reqOf' : T → R)
    (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t)
    (hreq : ∀ t, le' (reqOf t) (reqOf' t)) (t : T) :
    le' (Rfun S deps wf reqOf t) (Rfun S deps' wf' reqOf' t) := by
  refine wf'.induction
    (C := fun x => le' (Rfun S deps wf reqOf x) (Rfun S deps' wf' reqOf' x))
    t fun x ih => ?_
  rw [T1_exists S deps wf reqOf x, T1_exists S deps' wf' reqOf' x]
  refine H.join_le (H.trans (hreq x) (H.le_join_left _ _)) ?_
  refine H.trans ?_ (H.le_join_right _ _)
  refine H.joinList_le fun a ha => ?_
  obtain ⟨d, hd, rfl⟩ := List.mem_map.mp ha
  exact H.trans (ih d (hdeps x d hd))
    (H.le_joinList (List.mem_map.mpr ⟨d, hdeps x d hd, rfl⟩))

/-- Spec C1: adding a public dependency edge never lowers `R_L`. -/
theorem C1_add_edge (deps deps' : T → List T)
    (wf : WellFounded (DepRel deps)) (wf' : WellFounded (DepRel deps'))
    (reqOf : T → R) (hdeps : ∀ t, ∀ d ∈ deps t, d ∈ deps' t) (t : T) :
    S.le (Rfun S deps wf reqOf t) (Rfun S deps' wf' reqOf t) :=
  T2_growth S deps deps' wf wf' reqOf reqOf hdeps (fun u => S.le_refl (reqOf u)) t

/-- Spec C2: adding a declaration where nothing was imposed (`ReqOf` was the
bottom element, unconstrained) never lowers `R_L`. -/
theorem C2_declare (deps : T → List T) (wf : WellFounded (DepRel deps))
    (reqOf reqOf' : T → R) (u : T)
    (hu : reqOf u = S.bot) (hother : ∀ t, t ≠ u → reqOf' t = reqOf t) (t : T) :
    S.le (Rfun S deps wf reqOf t) (Rfun S deps wf reqOf' t) := by
  refine T2_growth S deps deps wf wf reqOf reqOf' (fun _ d hd => hd) (fun x => ?_) t
  by_cases hx : x = u
  · subst hx
    rw [hu]
    exact S.bot_le _
  · rw [hother x hx]
    exact S.le_refl _

/-- Spec T4, generic form: for any antitone satisfaction predicate `P`
(instantiated with `satisfies` at the consumer's level via L5) and any
"compiles" obligation discharged by Assumption A on every publicly
reachable target, edge compatibility (`P` holds of `R_L(d)`) implies the
obligation for the whole public reach of `d`. -/
theorem T4_soundness (deps : T → List T) (wf : WellFounded (DepRel deps)) (reqOf : T → R)
    (P : R → Prop) (Panti : ∀ {a b : R}, S.le a b → P b → P a)
    (Compiles : T → Prop) (d : T)
    (A : ∀ u ∈ reachList deps wf d, P (reqOf u) → Compiles u)
    (hcompat : P (Rfun S deps wf reqOf d)) :
    ∀ u ∈ reachList deps wf d, Compiles u :=
  fun u hu => A u hu (Panti (le_reqOf_of_mem_reach S deps wf reqOf hu) hcompat)

end Growth

end StdCompat
