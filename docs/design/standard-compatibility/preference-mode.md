# Standard compatibility: preference mode

> **Status: implemented.**  The resolver applies the ordering below, gated by the
> `[resolver] incompatible-standards` config knob.  Spec identifiers (D1-D14, C1-C3, T1-T4,
> Example n) refer to `spec.md`.

## 0. The `incompatible-standards` knob

Standard-aware preference is controlled by `[resolver] incompatible-standards` in
`.cabin/config.toml` (or the `CABIN_RESOLVER_INCOMPATIBLE_STANDARDS` environment variable, which
wins over the file).  The value vocabulary is **deliberately identical to Cargo's
`resolver.incompatible-rust-versions`** (`allow` / `fallback`) and the semantics mirror it:

- **`fallback`** (the default) applies the tiered ordering of section 1.
- **`allow`** gives standards *no* influence on selection: the chosen version is a pure function
  of the semver constraints, so a lockfile never moves when a workspace standard changes, and
  incompatibilities surface only through the post-resolution enforcement.  This is the
  strict / deterministic mode - documented as such, not as a legacy or compatibility fallback.

Under either value, solvability is identical: preference never introduces a resolution failure
`allow` would not also produce (section 4).

## 1. Selection policy

When choosing among candidate versions admitted by a dependency's semver requirement:

1. **Prefer** candidates whose declared requirements the consumer satisfies.
2. When **no** candidate in range satisfies, select what the resolver selects today - the newest
   admissible version - and **report** the unsatisfied requirement.
3. Candidates with **no declared interface standard** are deprioritized relative to
   declared-and-compatible candidates but **never filtered out**.
4. **Lockfile stability wins.**  A locked version that still qualifies is kept regardless of
   standard preference; metadata alone never churns a lockfile.

So the ordering within a range is: declared-and-satisfied (newest first), then undeclared
(newest first), then declared-but-unsatisfied (newest first) - and selecting from either lower
class is always accompanied by a report.  Undeclared candidates sit in the middle deliberately:
absence of metadata is $\textsf{unconstrained}$ (`registry-index.md`, section 3), which satisfies
every consumer (spec D11), but a declared-and-satisfied candidate has *promised* compatibility
while an undeclared one merely has not denied it - and pre-`standards` index entries must keep
resolving exactly as their authors expect, so they are never dropped.

**What "the consumer satisfies" means in v1.**  Satisfaction is evaluated against the
candidate's per-target `standards` rows, scoped to the targets an edge actually uses - the
reason `registry-index.md` publishes the full table instead of the summary its section 7
rejects:

- For edges from the workspace, the consumer's target tables name the dependency targets they
  link (`deps = ["fmt:fmt"]`, `docs/targets.md`), so the advertised requirement per language is
  the join (spec D4) over exactly the referenced target rows, checked against each consuming
  workspace target's effective level for the languages it compiles (spec D11).  A version whose
  stricter `extras` target the workspace never references is not held back by that target -
  matching the per-edge shape of D13/D14 as closely as index data allows.
- For packages reached only transitively, the index today carries neither the intermediate
  consumer's compile levels nor its target-level `deps`, so v1 falls back to the version-wide
  join checked against the workspace's minimum effective levels.  **The v1 consumer standard is,
  per language, the minimum effective implementation standard declared across the workspace's
  member targets** (a language no member compiles imposes nothing) - the Cargo-style
  workspace-level approximation, used uniformly for every edge.  With bounded requirements the
  minimum-level proxy has a further blind spot: a candidate capped at `max` is checked against
  the workspace *minimum* level, so a *higher*-level member sitting above the cap is not seen
  here - the post-resolution enforcement catches it, exactly as for the other optimistic
  cases below.  Exactness is not required
  because the post-resolution enforcement remains the correctness authority.  This fallback errs
  in **both**
  directions, and only one of them is conservative.  The version-wide join can only
  over-constrain - a spurious hold-back, lossy exactly as `registry-index.md` section 7
  describes.  The consumer proxy, however, can be optimistic: an intermediate registry package
  may compile at a *lower* level than every workspace target, so a candidate can be ranked
  satisfied that the true intermediate consumer cannot consume, and the post-resolution checks
  will still fail the resolution.  The scope of that optimism is bounded by the status quo: a
  standard-unaware resolver selects that same candidate today, and the same checks catch it
  (the post-resolution standard-compatibility check and the build-time enforcement of
  `docs/language-standards.md`).
  Preference mode never manufactures a failure that today's newest-first selection would have
  avoided; in this corner it merely fails to help.  Narrowing both sides - target references on
  index dependency entries for the edge side, serialized per-target implementation levels for
  the consumer side - is additive on the `standards` schema and can land when wanted.

**v1 applies the transitive fallback to every edge, direct edges included.**  The resolver
selects package-level versions and does not yet carry the workspace targets' `deps` references,
so a direct dependency is ranked by its version-wide join against the workspace-minimum consumer
levels exactly as a transitive one is - the per-edge target scoping of the first bullet is the
deferred refinement, not the v1 behavior.  Until it lands, a multi-target dependency whose
stricter `extras` target the workspace never links can be ranked incompatible, and reported as
held back, even though the linked edge alone is compatible.  This is a preference-only
over-constraint (a spurious hold-back or an older selection), never a resolution failure, and the
post-resolution enforcement stays authoritative.

The published cells are each target's **own declared** requirement, uncomposed
(`registry-index.md`, "Composition is the consumer's job"); preference mode completes the D10
join itself, walking the candidate versions' per-target rows along the public edges of the
resolved target graph.  How far v1 traverses candidate tables is an implementation decision
inside the same seam; skipped traversal errs toward under-advertising (a re-exporting wrapper may
rank better than its true $R_L$ warrants), which the advertisement framing already owns -
selection may under-promise, and the post-resolution checks recompute the truth.

Either way the check is an **ordering heuristic, never a correctness gate**: index metadata is
advertisement (`registry-index.md`, section 3), and the post-resolution build-time interface
enforcement of `docs/language-standards.md`, which recomputes from the fetched manifests, stays
authoritative.

## 2. Held-back reporting

When preference mode selects a version older than the newest available because of a standard
requirement, `cabin update` output (and any resolve-level report) must name all three of:

1. the **selected** version,
2. the **newest available** version,
3. the **requirement that held it back** - language, level, and the target row carrying it.
   Whether that minimum is an explicit declaration or a header-only inference (spec D9 row 3,
   Example 5) is not recorded in the index (`registry-index.md`); a report that wants to
   present inferred minima as inferred recomputes the provenance from the fetched manifest.

```text
fmt: selected 10.2.1 (newest 11.0.0 held back: requires c++20 via target `fmt`;
     workspace compiles C++ at c++17)
```

Naming all three keeps the remedy visible: the user can move the workspace level into the
candidate's accepted range (raise it toward a minimum; only lowering helps against a maximum -
spec L6's remedy remark), pin the older version deliberately, or ask the dependency author to
relax the interface.  A report that omits
the newest version hides that anything was held back at all; one that omits the requirement makes
the hold-back look like a resolver bug.  When rule 2 of section 1 fires instead (nothing in range
satisfies), the report names the selected version and the requirement it fails to satisfy.

## 3. Implementation seam

The seam is the `DependencyProvider` implementation in `crates/cabin-resolver/src/provider.rs`,
specifically its two decision hooks:

- `choose_version` - the candidate ordering of section 1 lives here, extending the current
  newest-non-yanked, lockfile-preferring selection (`choose_compatible_candidate`).
- `prioritize` - may additionally fold the preference signal into which package is decided next.

PubGrub remains an implementation detail behind the crate's public surface: the provider is
`pub(crate)`, no PubGrub type appears in `cabin-resolver`'s public API, and preference mode adds
**nothing** to what the solver sees - no new incompatibilities, no altered version sets.  The
whole policy is a reordering of choices the provider was already free to make.

## 4. Permanently out of scope: hard constraints in the solver

Standard requirements are **never** encoded as constraints inside the solver - no PubGrub
incompatibility, no altered version set, ever.  And preference mode itself does no provider-side
candidate elimination either: a `choose_version` that returns no candidate for standard reasons
is the same hard constraint through another door, since it triggers backtracking all the same.
Any future strict-filtering mode faces this same argument and is not designed here.

The rationale is the diamond problem.  Viability is a conjunction over every consumer edge (spec
D14), so in a diamond the weakest consumer poisons the shared version for everyone - spec
Example 2 works this end to end: one `c++17` consumer makes the shared `[c++20]` dependency
version unviable for the `c++23` consumer too.  Encoded as hard constraints, that turns a locally
fixable mismatch (raise one target's level, or relax one interface) into a global resolution
failure - or worse, a silent deep downgrade of the shared dependency to some ancient version
whose requirements everyone satisfies, which "solves" the constraints while degrading every
consumer - and the failure surfaces as a derivation-tree error that cannot name the actionable
fix.  Soft preference keeps resolution total and deterministic, and the held-back report names
the remedy.  Hard-filtering on advertised metadata would also turn a stale or wrong advertisement
into an unrecoverable resolution failure, inverting the layering of `registry-index.md`
(advertisement selects, the manifest enforces).  The spec deliberately leaves "what happens when
no candidate is viable" out of scope (D14); preference mode answers it with
select-latest-and-report.

**Reconciliation with the spec's filtering language.**  Spec D14 defines viability as "the
predicate the resolver uses to filter candidate versions", and its section 1 layering note says
the resolver "must not select" a version whose `"none"` declaration forbids the consumer's
language - framing that anticipates strict filtering.  The spec equally scopes out how
candidates are enumerated and what happens when none is viable; this document is the recorded
answer to both: viability-informed *ordering* plus select-latest-and-report, with the
post-resolution standard-compatibility check and the build-time interface enforcement of
`docs/language-standards.md` as the layers that actually refuse.  Those two framing sentences in
`spec.md` (the D14 "filter" wording and section 1's "must not select") have been amended to point
here, per the repository's update-both-documents rule: viability-informed *ordering* plus
select-latest-and-report, not strict in-solver filtering.

## 5. Not offered: automatic standard raising

Cabin does not raise a consumer's effective standard to satisfy a dependency - no CMake-style
elevation where requesting a feature quietly bumps the dialect a target compiles under.
Standards are explicit manifest declarations with no built-in default
(`docs/language-standards.md`); silently raising one changes how the consumer's **own**
translation units compile, which is exactly the ABI/ODR territory the spec declines to model
(Non-goals), and it buries the very signal preference mode exists to surface.  The remedy for an
unsatisfied requirement is an explicit manifest edit, prompted by the held-back report - never a
side effect of resolution.
