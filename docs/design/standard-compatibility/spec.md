# Standard compatibility: formal specification

## 1. Status and scope

This document is the **normative** specification of resolver-level language-standard
compatibility filtering.  Where implementation code and this document disagree, this document
wins.  Existing type names in the code (for example those in
`crates/cabin-core/src/language_standard.rs`) are non-authoritative implementation detail; the
implementation must be brought into agreement with this document, not the other way around.

The document is self-contained: an implementer must be able to build the compatibility module
from this document alone, and a reviewer must be able to check every proof here without external
context.  It contains no implementation code.

In scope:

- The per-language requirement domain, its order, and its join.
- How a dependency target's declarations map to a requirement on consumers, including
  header-only inference and the cross-language defaults.
- How requirements propagate along public dependency edges.
- Edge compatibility and package-version viability, as used by the resolver to filter candidate
  versions.
- Proofs of the algebraic and computational properties the implementation and its tests rely on.

Out of scope (specified elsewhere, consumed here as resolved inputs):

- The manifest surface: field names, parsing, target-over-package precedence, workspace
  inheritance, diagnostics, and the interface/implementation contradiction lint.  See
  `docs/language-standards.md`.  This document consumes only the resolved, typed per-target
  values (D6, D7).
- Compiler flag lowering and toolchain support validation.
- How the resolver enumerates candidate versions.  This document defines only the viability
  predicate the resolver applies to them (D14).
- The post-resolution, build-time interface enforcement documented in
  `docs/language-standards.md`.  That check runs after a resolution is fixed and keeps its own
  documented contract; this specification governs which candidate versions the resolver may
  pick in the first place.  The two layers deliberately differ in two defaults.  At the
  resolver, a compiled target with no interface declaration imposes no constraint (D9 row 4) -
  filtering versions by the implementation-standard fallback would reject resolutions the
  build-time check is already positioned to diagnose precisely, so the fallback stays a
  build-time concern.  And an explicit `"none"` is unsatisfiable here (D9 row 1) - the
  resolver ranks such a version last and selects it only when nothing better is in range, where
  the post-resolution enforcement then refuses it (`preference-mode.md`).  The build-time check
  deliberately leaves `"none"` to that post-resolution layer, whose per-edge
  `ignore-interface-standard` override must be able to unblock exactly that class; the range
  bounds themselves (minimum and maximum) are enforced at both layers.  Where the two documents
  appear to disagree, each governs its own layer; for resolver behavior, this document wins.

## 2. The model at a glance (informative)

Every dependency target induces, per consumer language, a **requirement**: the set of consumer
levels it accepts - everything (unconstrained), everything from a minimum up, an inclusive
bounded range, or nothing (forbidden).  Requirements accumulate along public dependency edges
by **intersecting** their accepted sets (the join); an empty intersection accepts nothing.  A
dependency edge is compatible when the consumer's compile level, in every language the consumer
compiles, lies inside the dependency's accumulated set.  A candidate package version is viable
when every edge resolving to it is compatible.  Two consequences shape everything downstream:
requirements are only **partially** ordered by strictness (two ranges can be incomparable), and
a composed requirement's two bounds may come from **different** sources - no algorithm or
diagnostic may assume one declaration explains a composed value.  Everything below makes this
precise and proves it well-behaved.

## 3. Definitions

Notation: $\bot$ denotes "absent" for partial attribute values; $\emptyset$ is the empty set;
$\subseteq$ is set inclusion; $\le$ is the level order of D2; $\sqsubseteq$ is the requirement
order of D3.  Definitions are numbered D1, D2, ...; lemmas L1, ...; theorems T1, ...;
corollaries C1, ....  Proof ends are marked $\blacksquare$.

**D1 (languages).**  The set of languages is $\mathrm{Lang} = \{\mathsf{C}, \mathsf{C{+}{+}}\}$.

**D2 (levels).**  Each language has a finite, totally ordered set of ISO standard levels:

$$
\begin{aligned}
\mathrm{CLevel}   &= \{89, 99, 11, 17, 23\},
  &\text{ordered } 89 < 99 < 11 < 17 < 23; \\
\mathrm{CxxLevel} &= \{98, 11, 14, 17, 20, 23, 26\},
  &\text{ordered } 98 < 11 < 14 < 17 < 20 < 23 < 26.
\end{aligned}
$$

The order is **chronological enumeration order**, not numeric order ($98 < 11$ in
$\mathrm{CxxLevel}$; $89 < 11$ in $\mathrm{CLevel}$).  There is no equivalence special case
anywhere in either chain; in particular $\texttt{c11} < \texttt{c17}$ strictly.  We write levels
as `c89`, ..., `c23` and `c++98`, ..., `c++26`, and write $\mathrm{Level}_L$ for the level set of
language $L$ ($\mathrm{Level}_{\mathsf{C}} = \mathrm{CLevel}$,
$\mathrm{Level}_{\mathsf{C{+}{+}}} = \mathrm{CxxLevel}$).  We write $\bot_L$ for the least
element of $\mathrm{Level}_L$ (`c89`, `c++98`).

*Remark (aliases are outside the model).*  `c90` is a parser-level alias of `c89`, and `c++03`
of `c++98`.  Aliases are normalized by the manifest parser before any value reaches this model;
no alias is an element of $\mathrm{CLevel}$ or $\mathrm{CxxLevel}$, and nothing in this document
mentions them again.

**D3 (requirement domain).**  For each language $L$, the per-language requirement domain is
the **interval domain**

$$
\mathrm{Req}_L = \{\textsf{unconstrained}\}
  \cup \{\, [m, {\uparrow}] : m \in \mathrm{Level}_L \,\}
  \cup \{\, [a, b] : a, b \in \mathrm{Level}_L,\ a \le b \,\}
  \cup \{\textsf{forbidden}\}
$$

with the **denotation** $\llbracket \cdot \rrbracket : \mathrm{Req}_L \to
\mathcal{P}(\mathrm{Level}_L)$ - the set of consumer levels a requirement accepts:

$$
\begin{aligned}
\llbracket \textsf{unconstrained} \rrbracket &= \mathrm{Level}_L \\
\llbracket [m, {\uparrow}] \rrbracket &= \{\, \ell : \ell \ge m \,\} \\
\llbracket [a, b] \rrbracket &= \{\, \ell : a \le \ell \le b \,\} \\
\llbracket \textsf{forbidden} \rrbracket &= \emptyset
\end{aligned}
$$

$[m, {\uparrow}]$ is the **minimum-only** shape (declared `min` with no `max`); $[a, b]$ the
**bounded** shape (declared `min` and `max`, inclusive on both ends; $a \le b$ is a manifest
validation invariant - an empty declared range is rejected at parse).  The **strictness**
preorder is reverse inclusion of denotations:

$$
r_1 \sqsubseteq r_2 \iff \llbracket r_2 \rrbracket \subseteq \llbracket r_1 \rrbracket
$$

Write $r_1 \approx r_2$ when both directions hold, i.e.
$\llbracket r_1 \rrbracket = \llbracket r_2 \rrbracket$.  $\sqsubseteq$ is reflexive and
transitive; it is antisymmetric only on the quotient by $\approx$ (L1 lists the $\approx$
classes) and it is **not total**: two ranges can be incomparable - e.g.
$\llbracket [\texttt{c++11}, \texttt{c++14}] \rrbracket$ and
$\llbracket [\texttt{c++20}, \texttt{c++23}] \rrbracket$ contain neither one the other.
No definition, algorithm, or diagnostic may assume two requirements are comparable.

**D4 (join).**  For $r_1, r_2 \in \mathrm{Req}_L$, the join $r_1 \sqcup r_2$ is the
requirement denoting the **intersection** of the accepted sets:
$\llbracket r_1 \sqcup r_2 \rrbracket
  = \llbracket r_1 \rrbracket \cap \llbracket r_2 \rrbracket$.
The denoted sets are intervals of a finite chain and intervals are closed under intersection,
so such a requirement always exists; it is unique up to $\approx$, and the normative
**structural rule** picks one shape deterministically:

- if either operand is $\textsf{forbidden}$, the join is $\textsf{forbidden}$;
- otherwise take the lower bound as the maximum of the operands' lower bounds (absent when
  neither has one) and the upper bound as the minimum of the operands' upper bounds (absent
  when neither has one);
- no bounds $\to$ $\textsf{unconstrained}$; lower bound $m$ only $\to$ $[m, {\uparrow}]$;
  both bounds with $a \le b$ $\to$ $[a, b]$; both bounds with $a > b$ - the **empty
  intersection** - $\to$ $\textsf{forbidden}$.

For a finite set or multiset $S \subseteq \mathrm{Req}_L$, $\bigsqcup S$ is the iterated
join, with $\bigsqcup \emptyset = \textsf{unconstrained}$ (L2 shows the result is
independent of iteration order and multiplicity).  An empty intersection arising anywhere in a
composition collapses the whole join to $\textsf{forbidden}$: no consumer level satisfies the
combined requirements, and diagnostics must be able to name **both** contributing bounds.

*Remark (why $\approx$-equal shapes stay distinct).*  $[m, {\uparrow}]$ and
$[m, \max \mathrm{Level}_L]$ denote the same set **today**, and
$[\bot_L, {\uparrow}]$ denotes the same set as $\textsf{unconstrained}$.  The shapes are kept
distinct anyway, for two normative reasons.  First, provenance: diagnostics report exactly the
declared bounds ("`c++17` or newer" versus "`c++17..c++26`"), and "nothing declared" versus "a
declared minimum at the lowest level" are different facts about the manifest.  Second, chain
extension: when a future revision appends a new level to $\mathrm{Level}_L$, a minimum-only
requirement accepts it and a bounded one does not - the two shapes diverge, so serialized
metadata must preserve which one was declared.  The structural join of D4 preserves shapes
accordingly: it emits a bounded result only when some operand contributed an upper bound.

**D5 (targets, dependency graph, public reachability).**  Fix a finite set $T$ of targets and a
set of directed dependency edges $E \subseteq T \times T$, where $(c, d) \in E$ means target $c$
depends on target $d$.  Each edge is classified **public** or **private**;
$E_{\mathrm{pub}} \subseteq E$ is the set of public edges.  The graph $(T, E)$ is acyclic;
acyclicity is guaranteed by resolution before this model applies (a dependency cycle is an error
upstream of compatibility filtering).

The intended semantics of the classification, which T4 makes precise as a premise: across any
edge $(c, d) \in E$, the consumer $c$'s translation units may include $d$'s public headers; the
edge is public exactly when $d$'s public headers are themselves part of $c$'s public interface
(re-exported), so that headers reachable through $d$'s public edges are in turn reachable from
$c$'s consumers.  A private edge exposes $d$'s public headers to $c$'s translation units but not
to $c$'s own public headers.  How an edge's classification is declared in manifests is outside
this document's scope.

Define the **public reachability set** of $t \in T$:

$$
\mathrm{PubReach}(t) = \{t\} \cup \{\, u \in T :
  \text{there is a nonempty path from } t \text{ to } u
  \text{ using only edges in } E_{\mathrm{pub}} \,\}
$$

$\mathrm{PubReach}(t)$ is finite (it is a subset of $T$).

**D6 (dependency target attributes).**  Each target $t \in T$ carries the following resolved
attributes, produced by the manifest layer (precedence and inheritance already applied; see
`docs/language-standards.md`):

- $\mathrm{kind}(t) \in \{\textsf{compiled}, \textsf{header-only}\}$ - whether the target has
  translation units of its own.
- For each $L \in \mathrm{Lang}$, an effective implementation standard
  $\mathrm{impl}_L(t) \in \mathrm{Level}_L \cup \{\bot\}$, with $\bot$ meaning the target does
  not implement $L$.  **Population contract:** $\mathrm{impl}_L(t)$ is non-$\bot$ exactly when
  the target itself implements $L$ - a compiled target implements $L$ when it has sources of
  $L$ (the level then resolves through the usual target-over-package precedence), and a
  header-only target implements $L$ only through a target-level implementation declaration.  A
  package-level implementation default alone never populates $\mathrm{impl}_L(t)$, mirroring
  the relevance rule of `docs/language-standards.md`: an inherited implementation default says
  how sibling targets compile, not that this target's headers involve $L$.
- For each $L \in \mathrm{Lang}$, an explicit interface declaration
  $\mathrm{decl}_L(t)$, one of: a declared range $(m, M)$ with
  $m \in \mathrm{Level}_L$ and $M \in \mathrm{Level}_L \cup \{{\uparrow}\}$, $m \le M$
  (`interface-c-standard` / `interface-cxx-standard`; the string form `"c++17"` is the
  minimum-only $(m, {\uparrow})$, the table form `{ min, max }` a bounded $(m, M)$);
  $\textsf{none}$, the declared value `"none"` (headers not consumable from $L$); or
  $\bot$, no explicit interface declaration for $L$.

*Remark (why D6's population contract matters).*  D9 routes on whether $\mathrm{impl}_L(d)$ is
present.  If a package-level implementation default could populate it, a pure-C++ compiled
target inheriting a package `c-standard` would take D9 row 4 ($\textsf{unconstrained}$)
instead of row 6 ($\textsf{forbidden}$) for C consumers, silently defeating the strict
C++-to-C default; and a header-only target inheriting a package `cxx-standard` while declaring
only `interface-c-standard` would manufacture a C++ interface minimum (row 3) for a language
its package never exposed.  The population contract rules both out.

**D7 (consumer effective standards).**  A consumer target $c$ compiles a (possibly empty) set
of languages $\mathrm{langs}(c) \subseteq \mathrm{Lang}$, and for each
$L \in \mathrm{langs}(c)$ has an effective compile level
$\mathrm{lvl}(c, L) \in \mathrm{Level}_L$.  (A target that compiles a language without an
effective standard is a manifest error upstream of this model; $\mathrm{lvl}$ is total on
$\mathrm{langs}(c)$.)  A header-only target has no translation units, so as a consumer it has
$\mathrm{langs}(c) = \emptyset$; D13 spells out what its edges mean.

**D8 / Invariant I1 (`gnu-extensions` is excluded).**  Each target has a boolean
`gnu-extensions` attribute, default `false`, which selects the GNU spelling of the same ISO
level at compiler-flag lowering time.  **Invariant I1:** no definition, function, or predicate
in this specification takes `gnu-extensions` as an input; it never participates in
compatibility.  In particular $\mathrm{ReqOf}$, $R_L$, $\mathrm{satisfies}$, edge compatibility,
and viability are independent of every target's `gnu-extensions` value.  This is a design
invariant: future revisions of this specification must preserve it.  (GNU dialect strings such
as `gnu++20` do not exist in manifests and are not levels; see D2.)

**D9 (declaration-to-requirement function $\mathrm{ReqOf}$).**  For a dependency target
$d \in T$ and a consumer language $L \in \mathrm{Lang}$, define
$\mathrm{ReqOf}(d, L) \in \mathrm{Req}_L$ by the first matching row:

| # | Condition | $\mathrm{ReqOf}(d, L)$ |
|---|-----------|------------------------|
| 1 | $\mathrm{decl}_L(d) = \textsf{none}$ | $\textsf{forbidden}$ |
| 2 | $\mathrm{decl}_L(d) = (m, M)$ | $[m, {\uparrow}]$ when $M = {\uparrow}$, else $[m, M]$ |
| 3 | $\mathrm{decl}_L(d) = \bot$, $\mathrm{impl}_L(d) = m \in \mathrm{Level}_L$, $\mathrm{kind}(d) = \textsf{header-only}$ | $[m, {\uparrow}]$ |
| 4 | $\mathrm{decl}_L(d) = \bot$, $\mathrm{impl}_L(d) = m \in \mathrm{Level}_L$, $\mathrm{kind}(d) = \textsf{compiled}$ | $\textsf{unconstrained}$ |
| 5 | $\mathrm{decl}_L(d) = \bot$, $\mathrm{impl}_L(d) = \bot$, $L = \mathsf{C{+}{+}}$ | $\textsf{unconstrained}$ |
| 6 | $\mathrm{decl}_L(d) = \bot$, $\mathrm{impl}_L(d) = \bot$, $L = \mathsf{C}$ | $\textsf{forbidden}$ |

The rows are mutually exclusive and exhaustive, so $\mathrm{ReqOf}$ is a total function.  Row by
row:

- Rows 1-2: an **explicit declaration always wins**, over inference and over both
  cross-language defaults.  Declaring `interface-c-standard` on a C++ target is exactly how its
  headers become consumable from C (overriding row 6), and `"none"` is how a C target opts out
  of C++ consumption (overriding row 5).
- Row 3: **header-only inference** - a header-only target without an explicit interface
  declaration for $L$ infers its interface minimum from its implementation standard for $L$.
- Row 4: a **compiled** target without an interface declaration imposes **no constraint**.
- Row 5: the **permissive C-to-C++ default** - a target that implements no C++ (in practice, a
  C target) is consumable from C++ at any C++ level by default.
- Row 6: the **strict C++-to-C default** - a target that implements no C (in practice, a C++
  target) is not consumable from C unless `interface-c-standard` is explicitly declared.

*Remark (why the defaults are asymmetric).*  C headers are conventionally consumable from C++
(possibly via `extern "C"` guards, which are the author's obligation under Assumption A in T4);
C++ headers are in general not valid C.  The defaults encode that convention; both are
overridable per rows 1-2.

**D10 (effective requirement $R_L$).**  For each language $L \in \mathrm{Lang}$, the effective
requirement $R_L : T \to \mathrm{Req}_L$ is defined by the recursion

$$
R_L(t) = \mathrm{ReqOf}(t, L) \sqcup
  \bigsqcup \{\, R_L(d) : (t, d) \in E_{\mathrm{pub}} \,\}
$$

where the inner join is over the public dependencies of $t$ (and is $\textsf{unconstrained}$
when $t$ has none, by the empty-join convention of D4).  Requirements propagate along **public**
edges only; private edges of $t$ do not contribute to $R_L(t)$.  T1 proves this recursion has
exactly one solution on the finite DAG $(T, E_{\mathrm{pub}})$ and gives its closed form.

*Remark (per-bound provenance).*  Because the join intersects ranges, the lower and upper
bound of $R_L(t)$ may be attained by **different** elements of $\mathrm{PubReach}(t)$, and a
$\textsf{forbidden}$ may arise either from a single $\textsf{forbidden}$ contribution (rows
1 and 6 of D9) or from an empty intersection of two bounds.  An implementation that explains
$R_L(t)$ to users must therefore track provenance **per bound** - one origin chain for the
lower bound, one for the upper - and, for an empty intersection, report both clashing chains;
a single "origin of the requirement" does not exist in general.

**D11 ($\mathrm{satisfies}$).**  For a consumer $c$, a language $L \in \mathrm{langs}(c)$, and a
requirement $r \in \mathrm{Req}_L$:

$$
\mathrm{satisfies}(c, L, r) = \bigl(\mathrm{lvl}(c, L) \in \llbracket r \rrbracket\bigr)
$$

unfolded per shape: true for $\textsf{unconstrained}$; $\mathrm{lvl}(c, L) \ge m$ for
$[m, {\uparrow}]$; $a \le \mathrm{lvl}(c, L) \le b$ for $[a, b]$; false for
$\textsf{forbidden}$.

**D12 (satisfaction sets).**  For $r \in \mathrm{Req}_L$, the satisfaction set is the
denotation: $\mathrm{Sat}_L(r) = \llbracket r \rrbracket$.  By construction,
$\mathrm{satisfies}(c, L, r)$ iff $\mathrm{lvl}(c, L) \in \mathrm{Sat}_L(r)$.  We drop the
subscript and write $\mathrm{Sat}(r)$ when $L$ is clear.  (D11/D12 keep both names so the
edge-compatibility prose reads the same as before; they are one function.)

**D13 (edge compatibility).**  A dependency edge $(c, d) \in E$ is **compatible** iff

$$
\forall\, L \in \mathrm{langs}(c) :\ \mathrm{satisfies}(c, L, R_L(d))
$$

The conjunction ranges over every language the **consumer** compiles: a mixed-language consumer
must satisfy the dependency's effective requirement for each of its languages.  Languages the
consumer does not compile impose nothing (in particular, $R_L(d) = \textsf{forbidden}$ for a
language $L \notin \mathrm{langs}(c)$ does not affect the edge).  Compatibility is defined per
edge; the edge's own public/private classification does not appear in the condition (both kinds
expose $d$'s public headers to $c$'s translation units, per D5).

A **header-only consumer** compiles no language ($\mathrm{langs}(c) = \emptyset$, D7), so every
edge out of it is compatible vacuously - the empty conjunction is true.  This is deliberate,
not a hole: the target has no translation units for a requirement to constrain, and its
dependencies reach the targets that do compile through propagation instead - when the
header-only target's edge to the dependency is public, D10 folds $R_L(d)$ into the header-only
target's own effective requirement, and every downstream compiling consumer picks it up across
its edge onto the header-only target (Example 3's chain shows the same mechanism).

**D14 (package-version viability).**  In a candidate resolution, a package version $v$ is
**viable** iff every dependency edge $(c, d) \in E$ whose dependency target $d$ belongs to $v$
is compatible.  Equivalently: $v$ is excluded as soon as at least one edge resolving to it is
incompatible.  Viability is the predicate that governs candidate preference: the resolver applies
it as a version-selection *ordering* (`preference-mode.md`), never as a hard in-solver filter, and
the post-resolution build-time enforcement of `docs/language-standards.md` is what actually refuses
an unviable resolution.  How candidates are enumerated is outside this document's scope; what
happens when no candidate is viable is answered by preference mode with select-latest-and-report.

## 4. Lemmas

**L1 (structure of the domain).**  $(\mathrm{Req}_L, \sqsubseteq)$ is a finite preorder with
least element $\textsf{unconstrained}$ and greatest element $\textsf{forbidden}$.  Its
quotient by $\approx$ is a finite partial order in bijection with the set of interval-shaped
subsets of $\mathrm{Level}_L$ (including $\mathrm{Level}_L$ itself and $\emptyset$), ordered
by reverse inclusion.  The $\approx$ classes are exactly:

- $\{\textsf{unconstrained},\ [\bot_L, {\uparrow}],\ [\bot_L, \max \mathrm{Level}_L]\}$
  (all denoting $\mathrm{Level}_L$);
- $\{[m, {\uparrow}],\ [m, \max \mathrm{Level}_L]\}$ for each $m > \bot_L$;
- the singleton $\{[a, b]\}$ for each $a \le b < \max \mathrm{Level}_L$;
- the singleton $\{\textsf{forbidden}\}$.

$\sqsubseteq$ is **not total**: for disjoint or partially overlapping ranges - e.g.
$[\texttt{c++11}, \texttt{c++14}]$ and $[\texttt{c++20}, \texttt{c++23}]$ - neither
denotation contains the other, so neither $r_1 \sqsubseteq r_2$ nor $r_2 \sqsubseteq r_1$.

*Proof.*  Reflexivity and transitivity of $\sqsubseteq$ are those of $\subseteq$; the bounds
follow from $\llbracket \textsf{unconstrained} \rrbracket = \mathrm{Level}_L \supseteq
\llbracket r \rrbracket \supseteq \emptyset = \llbracket \textsf{forbidden} \rrbracket$.
The map $r \mapsto \llbracket r \rrbracket$ is surjective onto the nonempty intervals,
$\mathrm{Level}_L$, and $\emptyset$ by construction of D3, and it identifies exactly the
listed classes (two shapes denote the same set iff they have the same lower endpoint and both
reach the top, or are both empty).  Non-totality is the displayed counterexample.
$\blacksquare$

**L2 (bounded join-semilattice).**  The structural join of D4 is associative, commutative,
and idempotent **on shapes** (not merely up to $\approx$), with $\textsf{unconstrained}$ as
identity and $\textsf{forbidden}$ as absorbing element.  Consequently the set join
$\bigsqcup S$ of D4 is well-defined for every finite multiset $S$ - independent of iteration
order and multiplicity - with $\bigsqcup \emptyset = \textsf{unconstrained}$ and
$\bigsqcup (S \cup S') = \bigsqcup S \sqcup \bigsqcup S'$.

*Proof.*  Represent every non-$\textsf{forbidden}$ shape by its bound pair
$(m^{-}, m^{+}) \in (\mathrm{Level}_L \cup \{\bot\}) \times
(\mathrm{Level}_L \cup \{\bot\})$, reading $\bot$ as "no bound": $\textsf{unconstrained}
= (\bot, \bot)$, $[m, {\uparrow}] = (m, \bot)$, $[a, b] = (a, b)$.  The structural rule
combines pairs componentwise - $\max$ on lower bounds and $\min$ on upper bounds, each with
$\bot$ as identity - and both components are commutative idempotent monoids, so the pair
combination is associative, commutative, and idempotent, with $(\bot, \bot)$ as identity.
The final rendering (collapsing an inverted pair to $\textsf{forbidden}$) does not disturb
this: an inverted pair arises in some grouping iff the overall intersection is empty (the
componentwise bounds are grouping-independent), and $\textsf{forbidden}$ absorbs every
further join, so all groupings agree on the shape.  Well-definedness of $\bigsqcup$ and the
flattening law follow as usual from associativity, commutativity, idempotence, and the
identity.  $\blacksquare$

**L3 (strictness is denotational).**  $r_1 \sqsubseteq r_2$ iff
$\mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$ - definitional after D3/D12, recorded as a
lemma because downstream proofs cite it.  The induced equivalence is the $\approx$ of D3,
whose classes L1 lists; $\approx$-equal shapes are behaviorally identical for
$\mathrm{satisfies}$ and differ only in provenance and under future chain extension (the
remark after D4).

**L4 (join is intersection of satisfaction sets).**  For all $r_1, r_2 \in \mathrm{Req}_L$:
$\mathrm{Sat}(r_1 \sqcup r_2) = \mathrm{Sat}(r_1) \cap \mathrm{Sat}(r_2)$, and for finite
$S$: $\mathrm{Sat}(\bigsqcup S) = \bigcap_{r \in S} \mathrm{Sat}(r)$, with the empty
intersection denoting $\mathrm{Level}_L$.

*Proof.*  The binary claim is D4's defining property; what needs proof is that the
**structural rule** realizes it.  Membership in
$\llbracket r_1 \rrbracket \cap \llbracket r_2 \rrbracket$ means satisfying every lower
bound and every upper bound present among the operands, i.e. $\ell \ge$ the maximum of the
lower bounds and $\ell \le$ the minimum of the upper bounds (each vacuous when absent) -
exactly the set the structural result denotes, including the empty case rendered
$\textsf{forbidden}$ and the no-bound cases rendered $\textsf{unconstrained}$ /
$[m, {\uparrow}]$.  The finite generalization follows by induction on $|S|$ using L2.
$\blacksquare$

**L5 (antitonicity of $\mathrm{satisfies}$).**  If $r_1 \sqsubseteq r_2$ and
$\mathrm{satisfies}(c, L, r_2)$, then $\mathrm{satisfies}(c, L, r_1)$.

*Proof.*  $\mathrm{lvl}(c, L) \in \mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$ by L3.
$\blacksquare$

**L6 (satisfaction sets are convex, not upward closed in general).**  Every
$\mathrm{Sat}(r)$ is an order-convex subset of $\mathrm{Level}_L$: if
$\ell_1 \le x \le \ell_2$ with $\ell_1, \ell_2 \in \mathrm{Sat}(r)$, then
$x \in \mathrm{Sat}(r)$.  Upward closure - "raising a consumer's level never breaks
satisfaction" - **fails** exactly for the bounded shapes $[a, b]$ with
$b < \max \mathrm{Level}_L$: raising past $b$ breaks satisfaction.  Every other shape's
denotation is upward closed: $\mathrm{Level}_L$ itself, the up-sets of the minimum-only
shapes, the empty set (vacuously), and a bounded range whose $b = \max \mathrm{Level}_L$ -
though the last stays upward closed only for **today's** chain, since a future appended level
falls outside it (the remark after D4).

*Proof.*  Each denotation of D3 is $\mathrm{Level}_L$, an up-set, an interval, or empty; all
are convex.  For the failure claim: $b \in \mathrm{Sat}([a, b])$ and any $\ell > b$ is not,
and such $\ell$ exists exactly when $b < \max \mathrm{Level}_L$; the remaining shapes'
denotations are upward closed by inspection.  $\blacksquare$

*Remark (normative consequence for remedies).*  Diagnostics and documentation must not
unconditionally advise raising a consumer's standard.  Below a minimum, raising (up to any
cap) helps; **above a maximum, only lowering the consumer, or changing the dependency, can** -
and against $\textsf{forbidden}$ nothing at the standard level helps.

**L7 (set joins are monotone).**  For finite multisets $S \subseteq S'$ over
$\mathrm{Req}_L$: $\bigsqcup S \sqsubseteq \bigsqcup S'$.  Moreover, if
$S = \{r_1, \ldots, r_k\}$ and $S' = \{r'_1, \ldots, r'_k\}$ with
$r_i \sqsubseteq r'_i$ pointwise, then $\bigsqcup S \sqsubseteq \bigsqcup S'$.

*Proof.*  By L4, $\mathrm{Sat}(\bigsqcup S') = \bigcap_{r \in S'} \mathrm{Sat}(r)
\subseteq \bigcap_{r \in S} \mathrm{Sat}(r) = \mathrm{Sat}(\bigsqcup S)$ for the subset
claim (intersecting more sets can only shrink the result), and
$\bigcap_i \mathrm{Sat}(r'_i) \subseteq \bigcap_i \mathrm{Sat}(r_i)$ for the pointwise
claim ($\mathrm{Sat}(r'_i) \subseteq \mathrm{Sat}(r_i)$ componentwise by L3).  Both are
$\sqsubseteq$ by L3.  $\blacksquare$

## 5. Theorems

**T1 ($R_L$ is well-defined on finite DAGs).**  On the finite DAG $(T, E_{\mathrm{pub}})$, the
recursion of D10 has exactly one solution, namely the closed form

$$
R_L(t) = \bigsqcup \{\, \mathrm{ReqOf}(u, L) : u \in \mathrm{PubReach}(t) \,\}
$$

and it is computable by processing targets in any topological order of
$(T, E_{\mathrm{pub}})$ (dependencies before dependents), with the same result for every such
order.  Computation terminates.

*Proof.*

*Well-foundedness.*  Since $(T, E_{\mathrm{pub}})$ is finite and acyclic (D5), define $h(t)$ as
the length of the longest path from $t$ using edges in $E_{\mathrm{pub}}$ (finite: paths in a
finite DAG cannot repeat vertices, so their length is bounded by $\lvert T \rvert - 1$).  If
$(t, d) \in E_{\mathrm{pub}}$ then $h(d) < h(t)$ (any path from $d$ extends to a longer one
from $t$).

*Existence and uniqueness.*  We show by strong induction on $h(t)$ that any function
$f : T \to \mathrm{Req}_L$ satisfying the recursion of D10 must agree with the closed form at
$t$; since the closed form itself is a well-defined function (each $\mathrm{PubReach}(t)$ is a
finite set and $\bigsqcup$ of a finite set is well-defined by L2), and substituting it into the
recursion succeeds (verified below), existence and uniqueness both follow.

Fix $t$ and assume the claim for all $u$ with $h(u) < h(t)$; in particular for every public
dependency $d$ of $t$.  Then for any solution $f$:

$$
\begin{aligned}
f(t) &= \mathrm{ReqOf}(t, L) \sqcup
  \bigsqcup \{\, f(d) : (t, d) \in E_{\mathrm{pub}} \,\}
  &&\text{(recursion, D10)} \\
&= \mathrm{ReqOf}(t, L) \sqcup \bigsqcup \Bigl\{\,
  \bigsqcup \{\, \mathrm{ReqOf}(u, L) : u \in \mathrm{PubReach}(d) \,\}
  : (t, d) \in E_{\mathrm{pub}} \,\Bigr\}
  &&\text{(IH)} \\
&= \bigsqcup \Bigl( \{\mathrm{ReqOf}(t, L)\} \cup
  \bigcup \{\, \{\, \mathrm{ReqOf}(u, L) : u \in \mathrm{PubReach}(d) \,\}
  : (t, d) \in E_{\mathrm{pub}} \,\} \Bigr)
  && \\
&= \bigsqcup \{\, \mathrm{ReqOf}(u, L) : u \in \mathrm{PubReach}(t) \,\}
  &&
\end{aligned}
$$

The third equality is the flattening law
$\bigsqcup (S \cup S') = \bigsqcup S \sqcup \bigsqcup S'$ iterated over the finitely many
public dependencies, valid by associativity, commutativity, and the identity element (L2).
The fourth holds because

$$
\mathrm{PubReach}(t) = \{t\} \cup
  \bigcup \{\, \mathrm{PubReach}(d) : (t, d) \in E_{\mathrm{pub}} \,\}
$$

by definition of $\mathrm{PubReach}$ (D5): a nonempty public path from $t$ starts with some
public edge $(t, d)$ and continues as a possibly-empty public path from $d$.  Note that a
target $u$ reachable through **several** public dependencies of $t$ (a diamond) contributes
$\mathrm{ReqOf}(u, L)$ once on the right but possibly several times in the flattened multiset
on the left; idempotence (L2) makes the multiset join equal to the set join, so the equality
holds regardless of path multiplicity.  Reading the chain of equalities backwards also shows
the closed form *is* a solution of the recursion, completing existence.

*Order-independence and termination.*  A topological order of the finite DAG exists and every
prefix of the computation only reads values of targets already processed (each $d$ with
$(t, d) \in E_{\mathrm{pub}}$ precedes $t$).  Whatever topological order is chosen, the
computed value at each $t$ satisfies the recursion, and by uniqueness it equals the closed
form - so all orders agree (confluence).  Termination is immediate: $\lvert T \rvert$ steps,
each a finite join.  $\blacksquare$

**T2 (growth).**  Let two attribute assignments over the same target set $T$ be given, with
public edge sets $E_{\mathrm{pub}} \subseteq E'_{\mathrm{pub}}$ and requirement functions
satisfying $\mathrm{ReqOf}(u, L) \sqsubseteq \mathrm{ReqOf}'(u, L)$ for every $u \in T$.  Write
$R_L$ and $R'_L$ for the respective effective requirements.  Then for every $t \in T$:

$$
R_L(t) \sqsubseteq R'_L(t)
$$

*Proof.*  $E_{\mathrm{pub}} \subseteq E'_{\mathrm{pub}}$ implies
$\mathrm{PubReach}(t) \subseteq \mathrm{PubReach}'(t)$ for every $t$ (every public path in the
smaller graph is one in the larger).  Using the closed form (T1) twice:

$$
\begin{aligned}
R_L(t) &= \bigsqcup \{\, \mathrm{ReqOf}(u, L) : u \in \mathrm{PubReach}(t) \,\} && \\
&\sqsubseteq \bigsqcup \{\, \mathrm{ReqOf}'(u, L) : u \in \mathrm{PubReach}(t) \,\}
  &&\text{(pointwise, L7 second claim)} \\
&\sqsubseteq \bigsqcup \{\, \mathrm{ReqOf}'(u, L) : u \in \mathrm{PubReach}'(t) \,\}
  &&\text{(superset, L7 first claim)} \\
&= R'_L(t) && \blacksquare
\end{aligned}
$$

**C1 (adding a public dependency never lowers $R_L$).**  Adding an edge to $E_{\mathrm{pub}}$
(leaving every $\mathrm{ReqOf}$ unchanged, and preserving acyclicity) satisfies T2's hypotheses
with equality on $\mathrm{ReqOf}$, so $R_L(t) \sqsubseteq R'_L(t)$ for every $t$.

**C2 (adding a declaration where nothing was imposed never lowers $R_L$).**  Suppose target $u$
has $\mathrm{ReqOf}(u, L) = \textsf{unconstrained}$ - by D9 that is a compiled target with no
interface declaration for a language it implements (row 4), or a target consumed from C++
under the permissive default (row 5).  Changing $u$'s declarations so that
$\mathrm{ReqOf}'(u, L) = r$ for **any** $r \in \mathrm{Req}_L$, leaving everything else fixed,
satisfies T2's hypotheses: $\textsf{unconstrained}$ is the least element (L1), so
$\textsf{unconstrained} \sqsubseteq r$, and every other target's $\mathrm{ReqOf}$ is unchanged.
Hence $R_L(t) \sqsubseteq R'_L(t)$ for every $t$.

**C3 (viable versions can only shrink).**  Under the hypotheses of T2 (in particular after any
change covered by C1 or C2), every dependency edge compatible under the primed assignment is
compatible under the unprimed one; consequently every package version viable under the primed
assignment is viable under the unprimed one - the set of viable versions can only shrink as
public edges are added or requirements grow.

*Proof.*  Let edge $(c, d)$ be compatible under the primed assignment: for every
$L \in \mathrm{langs}(c)$, $\mathrm{satisfies}(c, L, R'_L(d))$.  By T2,
$R_L(d) \sqsubseteq R'_L(d)$, so by antitonicity (L5) $\mathrm{satisfies}(c, L, R_L(d))$ for
every such $L$: the edge is compatible unprimed.  Viability of a version is a conjunction of
edge compatibilities (D14); each conjunct transfers, so viability transfers.
Contrapositively, growing the requirements can only remove versions from the viable set, never
add any.  $\blacksquare$

*Remark (the two deliberate exceptions).*  C2's hypothesis - that the prior requirement was
$\textsf{unconstrained}$ - is essential, and two rows of D9 sit **above** the bottom by design:

- A **header-only** target with no declaration already imposes its inferred implementation
  minimum (row 3).  Declaring an explicit, older `interface-*-standard` replaces
  $[\mathrm{impl}_L(u), {\uparrow}]$ by a wider range - a relaxation that can move $R_L$
  **down**.  That is the declared purpose of the field: promising less than the implementation
  uses.
- A target consumed from **C** under the strict default already imposes $\textsf{forbidden}$
  (row 6).  Declaring `interface-c-standard` replaces $\textsf{forbidden}$ by a range - again
  a relaxation moving down, and again the point of the declaration.
- Symmetrically, a change that only tightens - raising a minimum, lowering or adding a
  maximum, shifting a range so that it excludes previously accepted levels - satisfies T2's
  hypotheses in the tightening direction and can only shrink the viable set (C3).  A sideways
  shift both relaxes and tightens; neither T2 direction applies to it as a whole, and the
  viable set can change arbitrarily.

These moves are relaxations by the author of the **dependency**, widening its consumer set; T2
and C3 are about changes that tighten requirements.  Both directions are monotone: T2 applied
with the roles of the two assignments swapped shows a pointwise relaxation can only move every
$R_L$ down and can only grow the viable set.

**T3 (decidability and complexity).**  All predicates of this specification are decidable,
with:

1. $\mathrm{satisfies}(c, L, r)$ in $O(1)$;
2. $R_L(t)$ for **all** $t \in T$ in $O(\lvert T \rvert + \lvert E_{\mathrm{pub}} \rvert)$ per
   language, hence $O(\lvert T \rvert + \lvert E \rvert)$ total (with
   $\lvert \mathrm{Lang} \rvert = 2$ a constant);
3. viability of all package versions in a candidate resolution in
   $O(\lvert T \rvert + \lvert E \rvert)$ overall.

*Proof.*

(1)  A requirement is one of four shapes, and the range cases are at most two comparisons of
elements of a fixed finite chain (D2): constant work.

(2)  Fix $L$.  Compute a topological order of $(T, E_{\mathrm{pub}})$ in
$O(\lvert T \rvert + \lvert E_{\mathrm{pub}} \rvert)$ (standard for finite DAGs).  Process
targets in reverse dependency order; at target $t$, fold $\sqcup$ over $\mathrm{ReqOf}(t, L)$
(constant work: D9 is a six-row decision table over already-resolved attributes) and the
stored values $R_L(d)$ of its public dependencies (one $O(1)$ join per outgoing public edge:
D4's structural rule is a constant number of comparisons).  Each edge is touched once, each target
once: $O(\lvert T \rvert + \lvert E_{\mathrm{pub}} \rvert)$.  Correctness: this is exactly the
topological computation of T1, which proved it yields the unique solution regardless of the
order chosen.  Summing over the two languages gives $O(\lvert T \rvert + \lvert E \rvert)$.

(3)  With all $R_L$ values stored, checking one edge $(c, d)$ is a conjunction over
$\mathrm{langs}(c) \subseteq \mathrm{Lang}$, so at most two $O(1)$ $\mathrm{satisfies}$
checks.  Viability of every version is the conjunction, over each version's incoming edges, of
those edge checks (D14); every edge belongs to exactly one dependency target hence to one
version's conjunction, so all versions together cost $O(\lvert E \rvert)$.  Adding (2)'s
precomputation gives $O(\lvert T \rvert + \lvert E \rvert)$.  Decidability is immediate: every
domain in sight is finite and every function is total (D9's table is exhaustive; D11 is a
three-case match).  $\blacksquare$

**Assumption A (author obligation).**  For every target $u$ and language $L$: every consumer
level $\ell \in \mathrm{Sat}_L(\mathrm{ReqOf}(u, L))$ can compile $u$'s public headers as
language $L$ at level $\ell$.  Unfolding D12, that means: if $u$ declares (or, per D9, infers)
an interface range, its public headers compile under every consumer level inside that range -
including, for a bounded range, **no newer** than its maximum, which is exactly how an author
records headers that use features a later standard removed; if $\mathrm{ReqOf}(u, L) = \textsf{unconstrained}$, they compile under
**every** level of $L$ (for the C-to-C++ default, row 5 of D9, this is the C author's
obligation that the headers are consumable from any C++ level, for example via `extern "C"`
guards); if $\mathrm{ReqOf}(u, L) = \textsf{forbidden}$, $\mathrm{Sat} = \emptyset$ and the
obligation is vacuous.  Assumption A is the **package author's obligation, not something Cabin
verifies** (see Non-goals).

**T4 (conditional semantic soundness).**  Let $(c, d) \in E$ be a compatible edge (D13), and
suppose Assumption A holds for every target $u \in \mathrm{PubReach}(d)$ and every language
$L \in \mathrm{langs}(c)$.  Then for every such $u$ and $L$:

$$
\mathrm{lvl}(c, L) \in \mathrm{Sat}_L(\mathrm{ReqOf}(u, L))
$$

and therefore, under A, every public header of every target in $\mathrm{PubReach}(d)$ compiles
as language $L$ at $c$'s level $\mathrm{lvl}(c, L)$.  Since by D5 the public headers reachable
from $c$'s translation units through the edge $(c, d)$ are exactly the public headers of
targets in $\mathrm{PubReach}(d)$, edge compatibility implies that $c$'s translation units can
compile every public include they can reach through $d$.

*Proof.*  Fix $L \in \mathrm{langs}(c)$ and $u \in \mathrm{PubReach}(d)$.  By the closed form
(T1), $R_L(d) = \bigsqcup \{\, \mathrm{ReqOf}(w, L) : w \in \mathrm{PubReach}(d) \,\}$, and a
join is an upper bound of each of its elements (L2), so

$$
\mathrm{ReqOf}(u, L) \sqsubseteq R_L(d)
$$

Compatibility of the edge gives $\mathrm{satisfies}(c, L, R_L(d))$, i.e.
$\mathrm{lvl}(c, L) \in \mathrm{Sat}(R_L(d))$ (D12).  By L3,
$\mathrm{Sat}(R_L(d)) \subseteq \mathrm{Sat}(\mathrm{ReqOf}(u, L))$, hence
$\mathrm{lvl}(c, L) \in \mathrm{Sat}(\mathrm{ReqOf}(u, L))$.  Assumption A for $u$ and $L$
states that every level in $\mathrm{Sat}(\mathrm{ReqOf}(u, L))$ compiles $u$'s public headers
as $L$; applying it at $\ell = \mathrm{lvl}(c, L)$ yields the conclusion for $u$.  As $u$ and
$L$ were arbitrary, the claim holds for all of $\mathrm{PubReach}(d)$ and all of
$\mathrm{langs}(c)$.  $\blacksquare$

*Remark (scope of the guarantee).*  T4 speaks only about headers reachable along **public**
edges below $d$.  Headers of $d$'s private dependencies are, by the edge semantics of D5, not
included from $d$'s public headers, so $c$'s translation units never see them through this
edge and no constraint is needed; the private dependency's own edge from $d$ is checked
separately (D13 applies to every edge).  T4 is exactly as strong as Assumption A: Cabin checks
the arithmetic, the author promises the headers (see Non-goals).

## 6. Non-goals

This specification makes **no** claim about any of the following, and no lemma or theorem
above should be read as implying one:

- **ODR consistency across `#if __cplusplus` (or `__STDC_VERSION__`) branches.**  Two
  translation units at different levels may see different definitions of the same entity
  through the same header; T4 guarantees each unit *compiles*, not that their definitions are
  link-compatible or ODR-consistent.
- **ABI and mangling.**  No guarantee that objects compiled at different levels link correctly
  or mean the same thing at the boundary - for example, C++17 made `noexcept` part of the
  function type, changing template results and mangling relative to C++14 for the same
  header.
- **C++20 module BMI compatibility.**  Built module interfaces are compiler-, version-, flag-,
  and level-sensitive; nothing here models them.
- **Verification of Assumption A itself.**  Cabin does not compile-check a dependency's
  headers at each level of $\mathrm{Sat}(\mathrm{ReqOf}(\cdot))$; A is the package author's
  obligation, and a violated A voids T4's conclusion for the offending header without
  affecting any other result in this document (T1-T3 and L1-L7 are purely order-theoretic and
  hold regardless).

## Appendix: worked examples

All domains in this specification are finite:
$\lvert \mathrm{CLevel} \rvert = 5$, $\lvert \mathrm{CxxLevel} \rvert = 7$,
$\lvert \mathrm{Req}_{\mathsf{C}} \rvert = 2 + 5 + 15 = 22$,
$\lvert \mathrm{Req}_{\mathsf{C{+}{+}}} \rvert = 2 + 7 + 28 = 37$ (two sentinels, the
minimum-only shapes, and one bounded shape per pair $a \le b$).  Every per-pair claim below -
and every lemma about $\sqsubseteq$, $\sqcup$, $\mathrm{Sat}$, and $\mathrm{satisfies}$ -
is therefore verifiable by **exhaustive enumeration** over the full domain, and the
implementation's test suite is expected to do exactly that: enumerate all pairs (and triples,
for associativity, at least on the C chain) and assert the property, citing the lemma it
checks (L2 associativity, commutativity, idempotence, identity and absorption; L4
intersection including the empty-intersection collapse; L5 antitonicity; L6 convexity and the
failure of upward closure on bounded shapes; L1 non-totality by counterexample).  The
examples pick representative points of that space and work them end to end.

Reference table - $\mathrm{satisfies}$ over all of $\mathrm{CxxLevel}$ for the requirements
used below (rows are requirements $r$, columns consumer levels $\ell$; $\checkmark$ means
$\ell \in \mathrm{Sat}(r)$, i.e. $\mathrm{satisfies}$ is true per D11/D12, and an empty cell
means false):

| Requirement / level | `c++98` | `c++11` | `c++14` | `c++17` | `c++20` | `c++23` | `c++26` |
|---|---|---|---|---|---|---|---|
| $\textsf{unconstrained}$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $[\texttt{c++17}, {\uparrow}]$ | | | | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $[\texttt{c++20}, {\uparrow}]$ | | | | | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $[\texttt{c++11}, \texttt{c++14}]$ | | $\checkmark$ | $\checkmark$ | | | | |
| $\textsf{forbidden}$ | | | | | | | |

Each row is an order-convex block (L6); the bounded row is the one that is not upward closed -
it ends at its cap, while the $\textsf{unconstrained}$ and minimum-only rows are up-sets and the
$\textsf{forbidden}$ row is upward closed vacuously.  $[\texttt{c++20}, {\uparrow}] \sqcup
[\texttt{c++11}, \texttt{c++14}] = \textsf{forbidden}$: the two rows share no column (D4's
empty-intersection collapse).

### Example 1: C++23 implementation, c++17 interface, consumed from c++17

Library $Z$: $\mathrm{kind}(Z) = \textsf{compiled}$,
$\mathrm{impl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++23}$,
$\mathrm{decl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++17}$ (`interface-cxx-standard = "c++17"`:
the public headers only need C++17 even though the implementation compiles as C++23).  $Z$ has
no public dependencies.  Consumer $X$: $\mathrm{langs}(X) = \{\mathsf{C{+}{+}}\}$,
$\mathrm{lvl}(X, \mathsf{C{+}{+}}) = \texttt{c++17}$.

- $\mathrm{ReqOf}(Z, \mathsf{C{+}{+}}) = [\texttt{c++17}, {\uparrow}]$ by D9 row 2 - the
  explicit declaration wins; the implementation standard never enters (contrast Example 5,
  where it would infer $[\texttt{c++23}, {\uparrow}]$ only for a header-only target; for this
  **compiled** target an *absent* declaration would give $\textsf{unconstrained}$ by row 4).
- $R_{\mathsf{C{+}{+}}}(Z) = \mathrm{ReqOf}(Z, \mathsf{C{+}{+}}) \sqcup \bigsqcup \emptyset
  = [\texttt{c++17}, {\uparrow}] \sqcup \textsf{unconstrained}
  = [\texttt{c++17}, {\uparrow}]$ (D10, D4, L2 identity).
- Edge $(X, Z)$: $\mathrm{satisfies}(X, \mathsf{C{+}{+}}, [\texttt{c++17}, {\uparrow}])$ iff
  $\texttt{c++17} \ge \texttt{c++17}$: **true** - see the $[\texttt{c++17}, {\uparrow}]$
  row of the reference table.  The edge is compatible (D13); if it is the only edge resolving
  to $Z$'s version, that version is viable (D14).

### Example 2: diamond - consumers at c++17 and c++23 sharing one dependency

Targets $X$ ($\mathrm{lvl}(X, \mathsf{C{+}{+}}) = \texttt{c++17}$) and $Y$
($\mathrm{lvl}(Y, \mathsf{C{+}{+}}) = \texttt{c++23}$) both depend on library $Z$
($\mathrm{kind}(Z) = \textsf{compiled}$,
$\mathrm{decl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++20}$, no public dependencies), and some root
depends on both $X$ and $Y$ - a diamond with $Z$ shared at the bottom, both edges resolving to
the same candidate version $v$ of $Z$.

- $R_{\mathsf{C{+}{+}}}(Z) = [\texttt{c++20}, {\uparrow}]$ as in Example 1.
- Edge $(Y, Z)$: $\texttt{c++23} \ge \texttt{c++20}$ - compatible.
- Edge $(X, Z)$: $\texttt{c++17} \ge \texttt{c++20}$ is false
  ($\texttt{c++17} < \texttt{c++20}$ in D2's chain) - **incompatible**.
- Viability (D14) is a conjunction over **every** edge resolving to $v$: the $(Y, Z)$ edge
  cannot rescue $v$; because $(X, Z)$ is incompatible, $v$ is not viable, and the resolver
  must find a version of $Z$ whose requirement $X$ satisfies (or fail).  One incompatible
  consumer poisons the version for the whole graph - exactly the per-edge conjunction of
  D13/D14.  ($\mathrm{Sat}$ view: $Y$ sits inside
  $\mathrm{Sat}([\texttt{c++20}, {\uparrow}])$, $X$ below it.)

### Example 3: `"none"` on a transitive public dependency poisons the root

Chain $\mathrm{Root} \to A \to B$, both edges **public**; every target compiles only C++.
$\mathrm{Root}$ has $\mathrm{lvl}(\mathrm{Root}, \mathsf{C{+}{+}}) = \texttt{c++26}$ - the
newest level there is.  $A$ is a compiled library with no interface declaration; $B$ declares
`interface-cxx-standard = "none"`.

- $\mathrm{ReqOf}(B, \mathsf{C{+}{+}}) = \textsf{forbidden}$ (D9 row 1).
  $R_{\mathsf{C{+}{+}}}(B) = \textsf{forbidden}$.
- $\mathrm{ReqOf}(A, \mathsf{C{+}{+}}) = \textsf{unconstrained}$ (D9 row 4).  By D10:
  $R_{\mathsf{C{+}{+}}}(A) = \textsf{unconstrained} \sqcup R_{\mathsf{C{+}{+}}}(B)
  = \textsf{unconstrained} \sqcup \textsf{forbidden} = \textsf{forbidden}$ - the absorbing
  element of L2 in action: once $\textsf{forbidden}$ enters a join, nothing recovers.
- Edge $(\mathrm{Root}, A)$:
  $\mathrm{satisfies}(\mathrm{Root}, \mathsf{C{+}{+}}, \textsf{forbidden})$ is false (D11) -
  incompatible at **every** consumer level, even $\texttt{c++26}$ (the $\textsf{forbidden}$
  row of the reference table is empty; $\mathrm{Sat}(\textsf{forbidden}) = \emptyset$).  Any
  version of $A$ that publicly depends on this $B$ is unviable for any C++ consumer: $B$'s
  opt-out propagates up the public chain and poisons the root.  Had the edge $A \to B$ been
  **private**, D10 would not have folded $R_{\mathsf{C{+}{+}}}(B)$ into
  $R_{\mathsf{C{+}{+}}}(A)$ at all, $R_{\mathsf{C{+}{+}}}(A) = \textsf{unconstrained}$, and
  the root would be unaffected - propagation is along public edges only.

### Example 4: mixed-language consumer

Consumer $M$ compiles both languages: $\mathrm{langs}(M) = \{\mathsf{C}, \mathsf{C{+}{+}}\}$,
$\mathrm{lvl}(M, \mathsf{C}) = \texttt{c11}$,
$\mathrm{lvl}(M, \mathsf{C{+}{+}}) = \texttt{c++20}$.  Dependency $W$ is a compiled C library:
$\mathrm{impl}_{\mathsf{C}}(W) = \texttt{c17}$,
$\mathrm{impl}_{\mathsf{C{+}{+}}}(W) = \bot$,
$\mathrm{decl}_{\mathsf{C}}(W) = \texttt{c17}$ (`interface-c-standard = "c17"`),
$\mathrm{decl}_{\mathsf{C{+}{+}}}(W) = \bot$, no public dependencies.

- $R_{\mathsf{C}}(W) = \mathrm{ReqOf}(W, \mathsf{C}) = [\texttt{c17}, {\uparrow}]$ (D9
  row 2).
- $R_{\mathsf{C{+}{+}}}(W) = \mathrm{ReqOf}(W, \mathsf{C{+}{+}}) = \textsf{unconstrained}$
  (D9 row 5: no C++ implementation, no declaration - the permissive C-to-C++ default).
- Edge $(M, W)$ is a conjunction over $\mathrm{langs}(M)$ (D13):
  - $L = \mathsf{C{+}{+}}$:
    $\mathrm{satisfies}(M, \mathsf{C{+}{+}}, \textsf{unconstrained})$ is true.
  - $L = \mathsf{C}$: $\mathrm{satisfies}(M, \mathsf{C}, [\texttt{c17}, {\uparrow}])$ iff
    $\texttt{c11} \ge \texttt{c17}$: **false** ($\texttt{c11} < \texttt{c17}$, D2 - no
    equivalence special case).
- One failed conjunct suffices: the edge is **incompatible**, even though the C++ side is
  satisfied.  $M$ must raise its C level to `c17` or `c23` (a minimum-only requirement is
  upward closed, L6), or $W$ must relax its interface.  Conversely, a C++-only consumer
  ($\mathrm{langs} = \{\mathsf{C{+}{+}}\}$) would take only the first conjunct and pass:
  languages the consumer does not compile impose nothing.

For the strict opposite direction: if $M$ instead depended on a compiled C++ library $V$ with
$\mathrm{decl}_{\mathsf{C}}(V) = \bot$ and $\mathrm{impl}_{\mathsf{C}}(V) = \bot$, then
$R_{\mathsf{C}}(V) = \textsf{forbidden}$ (D9 row 6) and the $L = \mathsf{C}$ conjunct would
fail at every C level - a C++ library is consumable from C only via an explicit
`interface-c-standard` (D9 row 2 overriding row 6).

### Example 5: header-only inference

Header-only library $H$: $\mathrm{kind}(H) = \textsf{header-only}$,
$\mathrm{impl}_{\mathsf{C{+}{+}}}(H) = \texttt{c++20}$ (declared on the target itself - per
D6's population contract, a package-level implementation default alone would leave
$\mathrm{impl}_{\mathsf{C{+}{+}}}(H) = \bot$), $\mathrm{decl}_{\mathsf{C{+}{+}}}(H) = \bot$,
no public dependencies.  Consumer $X$ at $\mathrm{lvl}(X, \mathsf{C{+}{+}}) = \texttt{c++17}$.

- $\mathrm{ReqOf}(H, \mathsf{C{+}{+}}) = [\texttt{c++20}, {\uparrow}]$ by D9 row 3: with
  no translation units of its own, $H$'s headers *are* the implementation, so the
  implementation standard is inferred as the interface minimum.
  $R_{\mathsf{C{+}{+}}}(H) = [\texttt{c++20}, {\uparrow}]$.
- Edge $(X, H)$: $\texttt{c++17} \ge \texttt{c++20}$ is false - incompatible (the
  $[\texttt{c++20}, {\uparrow}]$ row of the reference table).
- Now the author audits the headers, finds they only use C++17, and declares
  `interface-cxx-standard = "c++17"`:
  $\mathrm{decl}_{\mathsf{C{+}{+}}}(H) = (\texttt{c++17}, {\uparrow})$, and D9 row 2
  preempts row 3 - the explicit declaration wins over inference.
  $R_{\mathsf{C{+}{+}}}(H) = [\texttt{c++17}, {\uparrow}]$, and the edge is compatible.
  Note this move widened the accepted set
  ($[\texttt{c++17}, {\uparrow}] \sqsubseteq [\texttt{c++20}, {\uparrow}]$): it is the
  first deliberate exception in the remark after C3 - a relaxation by the dependency's author,
  widening the consumer set (T2 with the assignments swapped: the viable set can only grow).

### Example 6: a bounded interface and the empty intersection

Library $G$ ships headers that use a construct a later standard **removed** (say, dynamic
exception specifications or `register`, both removed in C++17): its author declares
`interface-cxx-standard = { min = "c++11", max = "c++14" }`, so
$\mathrm{decl}_{\mathsf{C{+}{+}}}(G) = (\texttt{c++11}, \texttt{c++14})$ and
$\mathrm{ReqOf}(G, \mathsf{C{+}{+}}) = [\texttt{c++11}, \texttt{c++14}]$ (D9 row 2).

- Consumer $X$ at $\texttt{c++17}$:
  $\mathrm{satisfies}(X, \mathsf{C{+}{+}}, [\texttt{c++11}, \texttt{c++14}])$ is false -
  $\texttt{c++17} > \texttt{c++14}$, the bounded row of the reference table.  Raising $X$
  cannot help (L6's remark); only lowering to the range, or a newer $G$, can.
- Aggregator $A$ publicly depends on both $G$ and a modern library $N$ with
  $\mathrm{ReqOf}(N, \mathsf{C{+}{+}}) = [\texttt{c++20}, {\uparrow}]$.  By D10,
  $R_{\mathsf{C{+}{+}}}(A) = [\texttt{c++20}, {\uparrow}] \sqcup
  [\texttt{c++11}, \texttt{c++14}] = \textsf{forbidden}$ - the empty intersection: **no**
  C++ level satisfies both.  Every edge onto $A$ is incompatible at every consumer level, and
  a useful diagnostic must name both chains - the $[\texttt{c++20}, {\uparrow}]$ bound via
  $N$ and the $[\texttt{c++11}, \texttt{c++14}]$ cap via $G$ - because neither source alone
  explains the composed $\textsf{forbidden}$ (the remark after D10).

### Exhaustiveness note

Every check above is a lookup in a table like the reference table, and both tables and graphs
here are small by construction of the model: $\mathrm{Req}_L$ has at most 37 elements,
$\mathrm{satisfies}$ at most $37 \times 7 = 259$ cells per language, $\sqcup$ at most
$37 \times 37 = 1369$ cells, and D9 is a six-row decision table over finitely many attribute
combinations.  The implementation's test suite is expected to verify L1-L7 by full enumeration
of those tables (citing the lemmas), T1/T2 on small DAGs including the diamond of Example 2
and the chain of Example 3, the empty intersection of Example 6 with both provenance chains,
and each row of D9 by a dedicated fixture - covering C alongside C++ throughout.
