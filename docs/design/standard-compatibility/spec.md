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
  the post-resolution enforcement then refuses it (`preference-mode.md`), even while the
  build-time check's `"none"` handling remains deferred.  Where the two documents appear to
  disagree, each governs its own layer; for resolver behavior, this document wins.

## 2. The model at a glance (informative)

Every dependency target induces, per consumer language, a **requirement**: unconstrained, a
minimum standard level, or forbidden.  Requirements accumulate along public dependency edges by
taking the strictest (the join).  A dependency edge is compatible when the consumer's compile
level, in every language the consumer compiles, satisfies the dependency's accumulated
requirement.  A candidate package version is viable when every edge resolving to it is
compatible.  Everything below makes this precise and proves it well-behaved.

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

$$
\mathrm{Req}_L = \{\textsf{unconstrained}\}
  \cup \{\, [m] : m \in \mathrm{Level}_L \,\}
  \cup \{\textsf{forbidden}\}
$$

ordered by **strictness** $\sqsubseteq$, defined case by case:

- $\textsf{unconstrained} \sqsubseteq r$ for every $r \in \mathrm{Req}_L$;
- $r \sqsubseteq \textsf{forbidden}$ for every $r \in \mathrm{Req}_L$;
- $[a] \sqsubseteq [b]$ iff $a \le b$ in $\mathrm{Level}_L$;
- no other pairs are related, and $\sqsubseteq$ is reflexive.

Informally: $\textsf{unconstrained}$ imposes nothing, $[m]$ requires a consumer level of at
least $m$, $\textsf{forbidden}$ is unsatisfiable.  In v1, $\mathrm{Req}_L$ is a **finite chain**
(L1):

$$
\textsf{unconstrained} \sqsubseteq [\bot_L] \sqsubseteq \cdots
  \sqsubseteq [\max \mathrm{Level}_L] \sqsubseteq \textsf{forbidden}
$$

**D4 (join).**  For $r_1, r_2 \in \mathrm{Req}_L$, the join $r_1 \sqcup r_2$ is the
$\sqsubseteq$-maximum of $r_1$ and $r_2$ (well-defined because $\sqsubseteq$ is total, L1).  For
a finite set or multiset $S \subseteq \mathrm{Req}_L$, $\bigsqcup S$ is the
$\sqsubseteq$-maximum of $S$, with $\bigsqcup \emptyset = \textsf{unconstrained}$.

*Remark (reserved `max` and the interval generalization).*  Each interface requirement is
serialized as a pair `{min, max}` whose `max` slot is reserved and **always absent in v1**.
$\mathrm{Req}_L$ is designed so that populating `max` later is a domain swap, not a signature
change.  Define the interval domain

$$
\mathrm{Int}_L = \{\, [a, b] : a, b \in \mathrm{Level}_L,\ a \le b \,\}
  \cup \{\textsf{full}\} \cup \{\textsf{empty}\}
$$

where $[a, b]$ denotes $\{\, \ell \in \mathrm{Level}_L : a \le \ell \le b \,\}$,
$\textsf{full}$ denotes $\mathrm{Level}_L$, and $\textsf{empty}$ denotes $\emptyset$.
$\mathrm{Req}_L$ embeds into $\mathrm{Int}_L$ by $\textsf{unconstrained} \mapsto \textsf{full}$,
$[m] \mapsto [m, \max \mathrm{Level}_L]$, $\textsf{forbidden} \mapsto \textsf{empty}$.  On
$\mathrm{Int}_L$, the order is reverse set inclusion of the denoted sets, the join is **set
intersection** of the denoted sets, and an empty intersection is $\textsf{forbidden}$
($\textsf{empty}$).  Under the embedding, intersection of two up-sets
$[m_1, \max \mathrm{Level}_L]$ and $[m_2, \max \mathrm{Level}_L]$ is
$[m_1 \sqcup m_2, \max \mathrm{Level}_L]$, which agrees with the v1 join, and $\textsf{full}$
and $\textsf{empty}$ remain identity and absorbing element.  Every downstream definition in this
document (D10 through D14) depends on $\mathrm{Req}_L$ only through the operations $\sqcup$,
$\bigsqcup$, and the predicate $\mathrm{satisfies}$ / the set $\mathrm{Sat}$ (D11, D12), all of
which are defined on $\mathrm{Int}_L$ verbatim ($\mathrm{satisfies}$ becomes membership of the
consumer level in the denoted set).  The extension therefore changes no downstream signatures.
The rest of this document works in the v1 chain.

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
  $\mathrm{decl}_L(t) \in \mathrm{Level}_L \cup \{\textsf{none}\} \cup \{\bot\}$, where a level
  means a declared minimum (`interface-c-standard` / `interface-cxx-standard`),
  $\textsf{none}$ means the declared value `"none"` (headers not consumable from $L$), and
  $\bot$ means no explicit interface declaration for $L$.

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
| 2 | $\mathrm{decl}_L(d) = m \in \mathrm{Level}_L$ | $[m]$ |
| 3 | $\mathrm{decl}_L(d) = \bot$, $\mathrm{impl}_L(d) = m \in \mathrm{Level}_L$, $\mathrm{kind}(d) = \textsf{header-only}$ | $[m]$ |
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

**D11 ($\mathrm{satisfies}$).**  For a consumer $c$, a language $L \in \mathrm{langs}(c)$, and a
requirement $r \in \mathrm{Req}_L$:

$$
\begin{aligned}
\mathrm{satisfies}(c, L, \textsf{unconstrained}) &= \text{true} \\
\mathrm{satisfies}(c, L, [m]) &= \bigl(\mathrm{lvl}(c, L) \ge m\bigr) \\
\mathrm{satisfies}(c, L, \textsf{forbidden}) &= \text{false}
\end{aligned}
$$

**D12 (satisfaction sets).**  For $r \in \mathrm{Req}_L$, define
$\mathrm{Sat}_L(r) \subseteq \mathrm{Level}_L$:

$$
\begin{aligned}
\mathrm{Sat}_L(\textsf{unconstrained}) &= \mathrm{Level}_L \\
\mathrm{Sat}_L([m]) &= \{\, \ell \in \mathrm{Level}_L : \ell \ge m \,\} \\
\mathrm{Sat}_L(\textsf{forbidden}) &= \emptyset
\end{aligned}
$$

By construction, $\mathrm{satisfies}(c, L, r)$ iff $\mathrm{lvl}(c, L) \in \mathrm{Sat}_L(r)$.
We drop the subscript and write $\mathrm{Sat}(r)$ when $L$ is clear.

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

**L1 ($\mathrm{Req}_L$ is a finite chain).**  $(\mathrm{Req}_L, \sqsubseteq)$ is a finite
totally ordered set with least element $\textsf{unconstrained}$ and greatest element
$\textsf{forbidden}$.

*Proof.*  Let $\mathrm{Level}_L = \{m_1 < m_2 < \cdots < m_n\}$ (finite and totally ordered by
D2; $n = 5$ for C, $n = 7$ for C++).  Define
$\varphi : \mathrm{Req}_L \to \{0, 1, \ldots, n+1\}$ by

$$
\varphi(\textsf{unconstrained}) = 0, \qquad
\varphi([m_i]) = i, \qquad
\varphi(\textsf{forbidden}) = n+1
$$

$\varphi$ is a bijection: the three shapes of D3 are disjoint, and $m_i \mapsto i$ is a
bijection on the middle block.  We check $r \sqsubseteq s \iff \varphi(r) \le \varphi(s)$ by
cases on the definition of $\sqsubseteq$ in D3:

- $r = \textsf{unconstrained}$: $r \sqsubseteq s$ holds for all $s$, and
  $\varphi(r) = 0 \le \varphi(s)$ holds for all $s$.
- $s = \textsf{forbidden}$: $r \sqsubseteq s$ holds for all $r$, and
  $\varphi(r) \le n+1 = \varphi(s)$ holds for all $r$.
- $r = [m_i]$, $s = [m_j]$:
  $r \sqsubseteq s \iff m_i \le m_j \iff i \le j \iff \varphi(r) \le \varphi(s)$.
- $r = [m_i]$, $s = \textsf{unconstrained}$: D3 relates this pair only via reflexivity, which
  does not apply ($[m_i] \ne \textsf{unconstrained}$), so $r \not\sqsubseteq s$; and
  $\varphi(r) = i \ge 1 > 0 = \varphi(s)$.
- $r = \textsf{forbidden}$, $s \ne \textsf{forbidden}$: similarly $r \not\sqsubseteq s$ and
  $\varphi(r) = n+1 > \varphi(s)$.

So $\varphi$ is an order isomorphism onto the integer interval $\{0, \ldots, n+1\}$ with its
usual total order.  Total orders, reflexivity, antisymmetry, and transitivity transport along
order isomorphisms, so $(\mathrm{Req}_L, \sqsubseteq)$ is a finite chain;
$\varphi^{-1}(0) = \textsf{unconstrained}$ is its least and
$\varphi^{-1}(n+1) = \textsf{forbidden}$ its greatest element.  $\blacksquare$

**L2 (bounded join-semilattice).**  $(\mathrm{Req}_L, \sqsubseteq, \sqcup)$ is a bounded
join-semilattice: $\sqcup$ is the least upper bound, and it is associative, commutative, and
idempotent, with $\textsf{unconstrained}$ as identity and $\textsf{forbidden}$ as absorbing
element.

*Proof.*  By L1 the order is total, so for any $r_1, r_2$ the $\sqsubseteq$-maximum
$\max(r_1, r_2)$ exists and is one of the two elements.  It is an upper bound of both by
definition of maximum, and any upper bound $u$ satisfies $u \sqsupseteq \max(r_1, r_2)$ because
$u$ is above the larger of the two; so $\sqcup = \max$ is the least upper bound.  Through the
isomorphism $\varphi$ of L1, $\sqcup$ corresponds to $\max$ on integers, which is associative,
commutative, and idempotent; these equational properties transport along the bijection
$\varphi$.  For example
$\varphi(r_1 \sqcup r_2) = \max(\varphi(r_1), \varphi(r_2))$, so

$$
\varphi\bigl((r_1 \sqcup r_2) \sqcup r_3\bigr)
  = \max\bigl(\max(\varphi r_1, \varphi r_2), \varphi r_3\bigr)
  = \max\bigl(\varphi r_1, \max(\varphi r_2, \varphi r_3)\bigr)
  = \varphi\bigl(r_1 \sqcup (r_2 \sqcup r_3)\bigr)
$$

and injectivity of $\varphi$ gives associativity; commutativity and idempotence are the same
argument.  $\textsf{unconstrained}$ is the least element (L1), so
$\textsf{unconstrained} \sqcup r = \max(\textsf{unconstrained}, r) = r$: identity.
$\textsf{forbidden}$ is the greatest element (L1), so
$\textsf{forbidden} \sqcup r = \textsf{forbidden}$: absorbing.  Boundedness is L1's least and
greatest elements.  Consequently the set join $\bigsqcup S$ of D4 is well-defined for every
finite multiset $S$: by associativity and commutativity the result is independent of the order
of combination, by idempotence it is independent of multiplicity, and
$\bigsqcup \emptyset = \textsf{unconstrained}$ is the identity, so
$\bigsqcup (S \cup S') = \bigsqcup S \sqcup \bigsqcup S'$ for all finite $S, S'$.
$\blacksquare$

**L3 (semantic characterization).**  For all $r_1, r_2 \in \mathrm{Req}_L$:

1. (Soundness)  $r_1 \sqsubseteq r_2 \implies \mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$.
2. (Completeness, up to one degenerate pair)
   $\mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1) \implies r_1 \sqsubseteq r_2$, **except** for
   the single pair $r_1 = [\bot_L]$, $r_2 = \textsf{unconstrained}$ (where $\bot_L$ is the
   least level, D2).
3. (Induced equivalence)  $\mathrm{Sat}(r_1) = \mathrm{Sat}(r_2)$ iff $r_1 = r_2$ or
   $\{r_1, r_2\} = \{\textsf{unconstrained}, [\bot_L]\}$.

Consequently $\sqsubseteq$ coincides with reverse $\mathrm{Sat}$-inclusion on the quotient of
$\mathrm{Req}_L$ by the equivalence $\textsf{unconstrained} \approx [\bot_L]$, and
$\mathrm{Sat}$ is an order isomorphism from that quotient (ordered by $\sqsubseteq$) onto its
image ordered by $\supseteq$.  The two identified elements are kept distinct in
$\mathrm{Req}_L$ anyway: they are behaviorally equal for $\mathrm{satisfies}$ but differ in
provenance (nothing declared versus a declared minimum), which diagnostics report.

*Proof.*

(1)  Assume $r_1 \sqsubseteq r_2$; cases on D3.  If $r_1 = \textsf{unconstrained}$ then
$\mathrm{Sat}(r_1) = \mathrm{Level}_L \supseteq \mathrm{Sat}(r_2)$ for any $r_2$.  If
$r_2 = \textsf{forbidden}$ then $\mathrm{Sat}(r_2) = \emptyset \subseteq \mathrm{Sat}(r_1)$.
If $r_1 = [a]$, $r_2 = [b]$ with $a \le b$: $\ell \in \mathrm{Sat}([b])$ means
$\ell \ge b \ge a$, so $\ell \in \mathrm{Sat}([a])$.  These three cases cover every related
pair (D3 relates no others, and the reflexive pairs are trivial).

(2)  We prove the contrapositive: assume $r_1 \not\sqsubseteq r_2$ and
$(r_1, r_2) \ne ([\bot_L], \textsf{unconstrained})$; we show
$\mathrm{Sat}(r_2) \not\subseteq \mathrm{Sat}(r_1)$, i.e. exhibit
$\ell \in \mathrm{Sat}(r_2) \setminus \mathrm{Sat}(r_1)$.  Since $\sqsubseteq$ is total (L1),
$r_1 \not\sqsubseteq r_2$ means $r_2 \sqsubset r_1$ strictly.  Cases on the strict pairs, using
the chain of L1:

- $r_2 = \textsf{unconstrained}$, $r_1 = [a]$ with $a \ne \bot_L$ (the excluded pair is exactly
  $a = \bot_L$): take $\ell = \bot_L$.  Then
  $\ell \in \mathrm{Sat}(\textsf{unconstrained}) = \mathrm{Level}_L$, and
  $\ell \notin \mathrm{Sat}([a])$ because $\bot_L < a$.
- $r_2 = \textsf{unconstrained}$, $r_1 = \textsf{forbidden}$: any $\ell \in \mathrm{Level}_L$
  works ($\mathrm{Level}_L$ is nonempty); $\ell \in \mathrm{Level}_L = \mathrm{Sat}(r_2)$ and
  $\mathrm{Sat}(r_1) = \emptyset$.
- $r_2 = [b]$, $r_1 = [a]$ with $b < a$: take $\ell = b$.  Then $\ell \ge b$ so
  $\ell \in \mathrm{Sat}([b])$, and $\ell = b < a$ so $\ell \notin \mathrm{Sat}([a])$.
- $r_2 = [b]$, $r_1 = \textsf{forbidden}$: take $\ell = b \in \mathrm{Sat}([b])$;
  $\mathrm{Sat}(\textsf{forbidden}) = \emptyset$.

(There is no case $r_2 = \textsf{forbidden}$ with $r_2 \sqsubset r_1$, since
$\textsf{forbidden}$ is greatest.)  In every case
$\mathrm{Sat}(r_2) \not\subseteq \mathrm{Sat}(r_1)$, proving the contrapositive.

For the excluded pair itself:
$\mathrm{Sat}(\textsf{unconstrained}) = \mathrm{Level}_L
  = \{\, \ell : \ell \ge \bot_L \,\} = \mathrm{Sat}([\bot_L])$,
so $\mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$ holds while
$[\bot_L] \sqsubseteq \textsf{unconstrained}$ fails (D3, as in L1's fourth case) - the
exception is genuine and is the only one.

(3)  If $r_1 = r_2$, equality of $\mathrm{Sat}$ is trivial; and
$\mathrm{Sat}(\textsf{unconstrained}) = \mathrm{Sat}([\bot_L])$ was just shown.  Conversely
assume $\mathrm{Sat}(r_1) = \mathrm{Sat}(r_2)$ with $r_1 \ne r_2$; by totality (L1) one is
strictly below the other, say $r_2 \sqsubset r_1$.  Mutual inclusion holds, so by the
contrapositive argument of (2) the pair must be the excluded one:
$(r_1, r_2) = ([\bot_L], \textsf{unconstrained})$.  (Every other strict pair produced a
separating $\ell$.)  $\blacksquare$

**L4 (join is intersection of satisfaction sets).**  For all $r_1, r_2 \in \mathrm{Req}_L$:
$\mathrm{Sat}(r_1 \sqcup r_2) = \mathrm{Sat}(r_1) \cap \mathrm{Sat}(r_2)$.  More generally, for
finite $S \subseteq \mathrm{Req}_L$:
$\mathrm{Sat}(\bigsqcup S) = \bigcap_{r \in S} \mathrm{Sat}(r)$, with the convention that the
empty intersection is $\mathrm{Level}_L$.

*Proof.*  By L1, $\sqsubseteq$ is total, so without loss of generality $r_1 \sqsubseteq r_2$,
hence $r_1 \sqcup r_2 = r_2$ and by L3(1) $\mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$.
Then $\mathrm{Sat}(r_1 \sqcup r_2) = \mathrm{Sat}(r_2)
  = \mathrm{Sat}(r_1) \cap \mathrm{Sat}(r_2)$,
the last step because $\mathrm{Sat}(r_2)$ is the smaller of two nested sets.  The finite
generalization follows by induction on $|S|$: the base case is
$\mathrm{Sat}(\bigsqcup \emptyset) = \mathrm{Sat}(\textsf{unconstrained}) = \mathrm{Level}_L$
(the empty intersection), and the step is the binary case just proved together with
$\bigsqcup (S \cup \{r\}) = \bigsqcup S \sqcup r$ (L2).  $\blacksquare$

*Remark.*  L4 is the v1 shadow of the interval rule of D4's remark: under the interval
generalization, $\mathrm{Sat}$ becomes the denotation itself and L4 becomes the definition of
join (intersection), with $\mathrm{Sat}(r) = \emptyset \iff r = \textsf{forbidden}$.  Nothing
downstream distinguishes the two readings.

**L5 (antitonicity of $\mathrm{satisfies}$).**  If $r_1 \sqsubseteq r_2$ and
$\mathrm{satisfies}(c, L, r_2)$, then $\mathrm{satisfies}(c, L, r_1)$.

*Proof.*  $\mathrm{satisfies}(c, L, r_2)$ iff $\mathrm{lvl}(c, L) \in \mathrm{Sat}(r_2)$ (D12).
By L3(1), $\mathrm{Sat}(r_2) \subseteq \mathrm{Sat}(r_1)$, so
$\mathrm{lvl}(c, L) \in \mathrm{Sat}(r_1)$, i.e. $\mathrm{satisfies}(c, L, r_1)$.
$\blacksquare$

**L6 (satisfaction sets are upward closed).**  For every $r \in \mathrm{Req}_L$, if
$\ell \in \mathrm{Sat}(r)$ and $\ell' \ge \ell$ then $\ell' \in \mathrm{Sat}(r)$.
Consequently, raising a consumer's effective level in any language never breaks satisfaction of
any requirement: if $\mathrm{satisfies}(c, L, r)$ and $c'$ agrees with $c$ except
$\mathrm{lvl}(c', L) \ge \mathrm{lvl}(c, L)$, then $\mathrm{satisfies}(c', L, r)$.

*Proof.*  Cases on $r$ (D12).  $\mathrm{Sat}(\textsf{unconstrained}) = \mathrm{Level}_L$ is
upward closed trivially.  $\mathrm{Sat}([m]) = \{\, \ell : \ell \ge m \,\}$: from
$\ell \ge m$ and $\ell' \ge \ell$, transitivity of $\le$ gives $\ell' \ge m$.
$\mathrm{Sat}(\textsf{forbidden}) = \emptyset$ is upward closed vacuously.  The consequence is
immediate from D11/D12: $\mathrm{satisfies}(c, L, r)$ iff
$\mathrm{lvl}(c, L) \in \mathrm{Sat}(r)$, and $\mathrm{lvl}(c', L) \ge \mathrm{lvl}(c, L)$
stays in the upward closed set.  $\blacksquare$

**L7 (set joins are monotone).**  For finite multisets $S \subseteq S'$ over $\mathrm{Req}_L$:
$\bigsqcup S \sqsubseteq \bigsqcup S'$.  Moreover, if $S = \{r_1, \ldots, r_k\}$ and
$S' = \{r'_1, \ldots, r'_k\}$ with $r_i \sqsubseteq r'_i$ pointwise, then
$\bigsqcup S \sqsubseteq \bigsqcup S'$.

*Proof.*  For the first claim: $\bigsqcup S'$ is an upper bound of every element of $S'$, hence
of every element of $S \subseteq S'$; since $\bigsqcup S$ is the **least** upper bound of $S$
(L2), $\bigsqcup S \sqsubseteq \bigsqcup S'$.  For the second: each
$r_i \sqsubseteq r'_i \sqsubseteq \bigsqcup S'$, so $\bigsqcup S'$ is an upper bound of
$\{r_1, \ldots, r_k\}$, and again leastness of $\bigsqcup S$ gives
$\bigsqcup S \sqsubseteq \bigsqcup S'$.  $\blacksquare$

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
  $[\mathrm{impl}_L(u)]$ by a smaller $[m]$ - a relaxation that can move $R_L$ **down**.  That
  is the declared purpose of the field: promising less than the implementation uses.
- A target consumed from **C** under the strict default already imposes $\textsf{forbidden}$
  (row 6).  Declaring `interface-c-standard` replaces $\textsf{forbidden}$ by $[m]$ - again a
  relaxation moving down, and again the point of the declaration.

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

(1)  A requirement is one of three tags, and the $[m]$ case is a single comparison of two
elements of a fixed finite chain (D2): constant work.

(2)  Fix $L$.  Compute a topological order of $(T, E_{\mathrm{pub}})$ in
$O(\lvert T \rvert + \lvert E_{\mathrm{pub}} \rvert)$ (standard for finite DAGs).  Process
targets in reverse dependency order; at target $t$, fold $\sqcup$ over $\mathrm{ReqOf}(t, L)$
(constant work: D9 is a six-row decision table over already-resolved attributes) and the
stored values $R_L(d)$ of its public dependencies (one $O(1)$ join per outgoing public edge,
by (1)'s comparison bound and L2's $\sqcup = \max$).  Each edge is touched once, each target
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
an interface minimum $m$, its public headers compile under every consumer level $\ge m$ in
that language; if $\mathrm{ReqOf}(u, L) = \textsf{unconstrained}$, they compile under
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
$\mathrm{lvl}(c, L) \in \mathrm{Sat}(R_L(d))$ (D12).  By L3(1),
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
$\lvert \mathrm{Req}_{\mathsf{C}} \rvert = 7$,
$\lvert \mathrm{Req}_{\mathsf{C{+}{+}}} \rvert = 9$.  Every per-pair claim below - and every
lemma about $\sqsubseteq$, $\sqcup$, $\mathrm{Sat}$, and $\mathrm{satisfies}$ - is therefore
verifiable by **exhaustive enumeration** over the full domain, and the implementation's test
suite is expected to do exactly that: enumerate all pairs (or triples, for associativity) and
assert the property, citing the lemma number it checks (L1
totality/antisymmetry/transitivity, L2 associativity, commutativity, idempotence, identity
and absorption, L3 soundness and its single
exception, L4 intersection, L5 antitonicity, L6 upward closure).  The examples pick
representative points of that space and work them end to end.

Reference table - $\mathrm{satisfies}$ over all of $\mathrm{CxxLevel}$ for the requirements
used below (rows are requirements $r$, columns consumer levels $\ell$; $\checkmark$ means
$\ell \in \mathrm{Sat}(r)$, i.e. $\mathrm{satisfies}$ is true per D11/D12, and an empty cell
means false):

| Requirement / level | `c++98` | `c++11` | `c++14` | `c++17` | `c++20` | `c++23` | `c++26` |
|---|---|---|---|---|---|---|---|
| $\textsf{unconstrained}$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $[\texttt{c++17}]$ | | | | $\checkmark$ | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $[\texttt{c++20}]$ | | | | | $\checkmark$ | $\checkmark$ | $\checkmark$ |
| $\textsf{forbidden}$ | | | | | | | |

Each row is upward closed (L6), and rows shrink as the requirement climbs the chain of L1
(L3(1) in table form).

### Example 1: C++23 implementation, c++17 interface, consumed from c++17

Library $Z$: $\mathrm{kind}(Z) = \textsf{compiled}$,
$\mathrm{impl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++23}$,
$\mathrm{decl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++17}$ (`interface-cxx-standard = "c++17"`:
the public headers only need C++17 even though the implementation compiles as C++23).  $Z$ has
no public dependencies.  Consumer $X$: $\mathrm{langs}(X) = \{\mathsf{C{+}{+}}\}$,
$\mathrm{lvl}(X, \mathsf{C{+}{+}}) = \texttt{c++17}$.

- $\mathrm{ReqOf}(Z, \mathsf{C{+}{+}}) = [\texttt{c++17}]$ by D9 row 2 - the explicit
  declaration wins; the implementation standard never enters (contrast Example 5, where it
  would infer $[\texttt{c++23}]$ only for a header-only target; for this **compiled** target
  an *absent* declaration would give $\textsf{unconstrained}$ by row 4).
- $R_{\mathsf{C{+}{+}}}(Z) = \mathrm{ReqOf}(Z, \mathsf{C{+}{+}}) \sqcup \bigsqcup \emptyset
  = [\texttt{c++17}] \sqcup \textsf{unconstrained} = [\texttt{c++17}]$ (D10, D4, L2
  identity).
- Edge $(X, Z)$: $\mathrm{satisfies}(X, \mathsf{C{+}{+}}, [\texttt{c++17}])$ iff
  $\texttt{c++17} \ge \texttt{c++17}$: **true** - see the $[\texttt{c++17}]$ row of the
  reference table.  The edge is compatible (D13); if it is the only edge resolving to $Z$'s
  version, that version is viable (D14).

### Example 2: diamond - consumers at c++17 and c++23 sharing one dependency

Targets $X$ ($\mathrm{lvl}(X, \mathsf{C{+}{+}}) = \texttt{c++17}$) and $Y$
($\mathrm{lvl}(Y, \mathsf{C{+}{+}}) = \texttt{c++23}$) both depend on library $Z$
($\mathrm{kind}(Z) = \textsf{compiled}$,
$\mathrm{decl}_{\mathsf{C{+}{+}}}(Z) = \texttt{c++20}$, no public dependencies), and some root
depends on both $X$ and $Y$ - a diamond with $Z$ shared at the bottom, both edges resolving to
the same candidate version $v$ of $Z$.

- $R_{\mathsf{C{+}{+}}}(Z) = [\texttt{c++20}]$ as in Example 1.
- Edge $(Y, Z)$: $\texttt{c++23} \ge \texttt{c++20}$ - compatible.
- Edge $(X, Z)$: $\texttt{c++17} \ge \texttt{c++20}$ is false
  ($\texttt{c++17} < \texttt{c++20}$ in D2's chain) - **incompatible**.
- Viability (D14) is a conjunction over **every** edge resolving to $v$: the $(Y, Z)$ edge
  cannot rescue $v$; because $(X, Z)$ is incompatible, $v$ is not viable, and the resolver
  must find a version of $Z$ whose requirement $X$ satisfies (or fail).  One incompatible
  consumer poisons the version for the whole graph - exactly the per-edge conjunction of
  D13/D14.  (L6 view: $\mathrm{Sat}([\texttt{c++20}])$ is upward closed, $Y$ sits inside it,
  $X$ sits below it.)

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

- $R_{\mathsf{C}}(W) = \mathrm{ReqOf}(W, \mathsf{C}) = [\texttt{c17}]$ (D9 row 2).
- $R_{\mathsf{C{+}{+}}}(W) = \mathrm{ReqOf}(W, \mathsf{C{+}{+}}) = \textsf{unconstrained}$
  (D9 row 5: no C++ implementation, no declaration - the permissive C-to-C++ default).
- Edge $(M, W)$ is a conjunction over $\mathrm{langs}(M)$ (D13):
  - $L = \mathsf{C{+}{+}}$:
    $\mathrm{satisfies}(M, \mathsf{C{+}{+}}, \textsf{unconstrained})$ is true.
  - $L = \mathsf{C}$: $\mathrm{satisfies}(M, \mathsf{C}, [\texttt{c17}])$ iff
    $\texttt{c11} \ge \texttt{c17}$: **false** ($\texttt{c11} < \texttt{c17}$, D2 - no
    equivalence special case).
- One failed conjunct suffices: the edge is **incompatible**, even though the C++ side is
  satisfied.  $M$ must raise its C level to `c17` or `c23` (L6: once inside
  $\mathrm{Sat}([\texttt{c17}])$, raising further never breaks it), or $W$ must relax its
  interface.  Conversely, a C++-only consumer
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

- $\mathrm{ReqOf}(H, \mathsf{C{+}{+}}) = [\texttt{c++20}]$ by D9 row 3: with no translation
  units of its own, $H$'s headers *are* the implementation, so the implementation standard is
  inferred as the interface minimum.  $R_{\mathsf{C{+}{+}}}(H) = [\texttt{c++20}]$.
- Edge $(X, H)$: $\texttt{c++17} \ge \texttt{c++20}$ is false - incompatible (the
  $[\texttt{c++20}]$ row of the reference table).
- Now the author audits the headers, finds they only use C++17, and declares
  `interface-cxx-standard = "c++17"`: $\mathrm{decl}_{\mathsf{C{+}{+}}}(H) = \texttt{c++17}$,
  and D9 row 2 preempts row 3 - the explicit declaration wins over inference.
  $R_{\mathsf{C{+}{+}}}(H) = [\texttt{c++17}]$, and the edge is compatible.  Note this move
  went **down** the chain ($[\texttt{c++17}] \sqsubseteq [\texttt{c++20}]$): it is the first
  deliberate exception in the remark after C3 - a relaxation by the dependency's author,
  widening the consumer set (T2 with the assignments swapped: the viable set can only grow).

### Exhaustiveness note

Every check above is a lookup in a table like the reference table, and both tables and graphs
here are tiny by construction of the model: $\mathrm{Req}_L$ has at most 9 elements,
$\mathrm{satisfies}$ at most $9 \times 7 = 63$ cells per language, $\sqcup$ at most
$9 \times 9 = 81$ cells, and D9 is a six-row decision table over finitely many attribute
combinations.  The implementation's test suite is expected to verify L1-L6 by full enumeration
of those tables (citing the lemma numbers), T1/T2 on small DAGs including the diamond of
Example 2 and the chain of Example 3, and each row of D9 by a dedicated fixture - covering C
alongside C++ throughout.
