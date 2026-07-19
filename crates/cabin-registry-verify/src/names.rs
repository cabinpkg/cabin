//! Name advisories: pure checks over a candidate `<scope>/<name>`
//! and the registry's package corpus
//! (`GET /api/v1/admin/packages`), run by the workflow **before**
//! the artifact download - none of them need the archive bytes.
//!
//! Advisory doctrine (`registry/docs/architecture.md`, "Name
//! fidelity"): a finding never rejects.  The workflow abstains -
//! renders no verdict at all - so the version stays `pending`, the
//! stuck-pending alert summons the operator, and a false positive
//! costs a delay, never a rejection.  Only a version that would
//! introduce a new name is evaluated: once any version of the
//! package is verified its name was accepted - by the advisories
//! proceeding or by an operator's manual verdict - and every later
//! version skips the advisories.  A rejection never vets a name:
//! rejecting an abstained squat must not exempt that same name's
//! next version.

use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

/// The corpus response body.  Tolerant of extra fields like
/// [`crate::PendingVersion`], so the verifier keeps working when the
/// listing grows.
#[derive(Debug, Deserialize)]
pub struct Corpus {
    pub packages: Vec<CorpusPackage>,
}

/// One package of the corpus: its name, plus whether any of its
/// versions is verified (the name was accepted once).
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusPackage {
    pub scope: String,
    pub name: String,
    pub vetted: bool,
}

/// One advisory finding: the rule identifier, optionally followed by
/// the existing package or scope the candidate collides with.
/// [`Display`](fmt::Display) renders the workflow's log detail -
/// `confusable_package (fmtlib/fmt)`.  The detail is always an
/// already-published name (public by construction); the profanity
/// rule carries none, so the matched term never reaches a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    rule: &'static str,
    detail: Option<String>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} ({detail})", self.rule),
            None => f.write_str(self.rule),
        }
    }
}

/// The confusability skeleton: `-` and `_` fold away, `{1, i} -> l`
/// and `0 -> o`.  A structural mirror of the registry worker's
/// `names::skeleton` (`registry/src/names.rs`) - the worker is a
/// standalone wasm32 workspace, so the map cannot be shared as code;
/// the two copies are pinned by each crate's tests.
fn skeleton(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            '-' | '_' => None,
            '1' | 'i' => Some('l'),
            '0' => Some('o'),
            c => Some(c),
        })
        .collect()
}

/// Hand-maintained, unambiguous slurs only; the charter is "a name
/// containing this going live unreviewed would be an incident",
/// nothing milder.  Substring matching over the skeleton fold on
/// purpose - separator- and homoglyph-laundered spellings
/// (`n-igger`, `n1gger`) still match, and the Scunthorpe false
/// positive costs a delay, never a rejection.  Deliberately not a
/// general profanity vocabulary.
const PROFANITY: &[&str] = &["nigger", "nigga", "faggot", "kike"];

/// Run every name advisory for the candidate against the corpus.
/// An empty result means proceed; any finding means abstain.  The
/// findings are deterministic: package collisions in corpus order,
/// then scope collisions in lexicographic order, then profanity.
pub fn advise(scope: &str, name: &str, corpus: &Corpus) -> Vec<Finding> {
    // The vetted-once skip: a verified version means the name was
    // accepted exactly once already (advisories proceeded, or an
    // operator approved it past an abstain).
    if corpus
        .packages
        .iter()
        .any(|package| package.scope == scope && package.name == name && package.vetted)
    {
        return Vec::new();
    }

    let mut findings = Vec::new();
    let folded_scope = skeleton(scope);
    let folded_full = format!("{folded_scope}/{}", skeleton(name));
    for package in &corpus.packages {
        // The candidate's own package row exists since its publish;
        // a name is not confusable with itself.
        if package.scope == scope && package.name == name {
            continue;
        }
        let other_full = format!("{}/{}", skeleton(&package.scope), skeleton(&package.name));
        if folded_full == other_full {
            findings.push(Finding {
                rule: "confusable_package",
                detail: Some(format!("{}/{}", package.scope, package.name)),
            });
        } else if package.scope != scope && within_one_edit(&folded_full, &other_full) {
            // Edit distance stays cross-scope: within one scope only
            // members publish, so `fmt` next to `fmts` is a sibling,
            // not a squat.
            findings.push(Finding {
                rule: "near_name",
                detail: Some(format!("{}/{}", package.scope, package.name)),
            });
        }
    }

    let scopes: BTreeSet<&str> = corpus
        .packages
        .iter()
        .map(|package| package.scope.as_str())
        .collect();
    for existing in scopes {
        if existing != scope && skeleton(existing) == folded_scope {
            findings.push(Finding {
                rule: "confusable_scope",
                detail: Some(existing.to_owned()),
            });
        }
    }

    if PROFANITY
        .iter()
        .any(|term| folded_full.contains(&skeleton(term)))
    {
        findings.push(Finding {
            rule: "profanity",
            detail: None,
        });
    }
    findings
}

/// Whether `a` and `b` are within Levenshtein distance 1.  Equal
/// strings answer true too, but the caller only reaches here after
/// the equality rule failed.
fn within_one_edit(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    match long.len() - short.len() {
        0 => {
            short
                .iter()
                .zip(long.iter())
                .filter(|(x, y)| x != y)
                .count()
                <= 1
        }
        1 => {
            // One insertion: the strings must agree once exactly one
            // character of the longer is skipped.
            let mut i = 0;
            while i < short.len() && short[i] == long[i] {
                i += 1;
            }
            short[i..] == long[i + 1..]
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(entries: &[(&str, &str, bool)]) -> Corpus {
        Corpus {
            packages: entries
                .iter()
                .map(|(scope, name, vetted)| CorpusPackage {
                    scope: (*scope).to_owned(),
                    name: (*name).to_owned(),
                    vetted: *vetted,
                })
                .collect(),
        }
    }

    fn rendered(scope: &str, name: &str, corpus: &Corpus) -> Vec<String> {
        advise(scope, name, corpus)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn skeleton_mirrors_the_worker_fold() {
        assert_eq!(skeleton("fmtlib"), "fmtllb");
        assert_eq!(skeleton("fmtl1b"), "fmtllb");
        assert_eq!(skeleton("f-mt_lib"), "fmtllb");
        assert_eq!(skeleton("z0log"), "zolog");
        for name in ["fmtlib", "a-b_c", "0i1l"] {
            let once = skeleton(name);
            assert_eq!(skeleton(&once), once, "name: {name:?}");
        }
    }

    #[test]
    fn a_vetted_name_skips_every_advisory() {
        // The name itself carries a slur-free but confusable spelling;
        // its own verified sibling version turns everything off.
        let vetted = corpus(&[("fmtlib", "fmt", true), ("fmtl1b", "fmt", false)]);
        assert!(advise("fmtlib", "fmt", &vetted).is_empty());
        // An unvetted flag (pending- or rejected-only) does not: a
        // rejection never vets a name.
        let unvetted = corpus(&[("fmtlib", "fmt", false), ("fmtl1b", "fmt", true)]);
        assert!(!advise("fmtlib", "fmt", &unvetted).is_empty());
    }

    #[test]
    fn a_clean_new_name_proceeds() {
        let existing = corpus(&[("fmtlib", "fmt", true), ("gabime", "spdlog", true)]);
        assert!(advise("acme", "widgets", &existing).is_empty());
        // Its own just-published row does not collide with itself.
        let with_self = corpus(&[("acme", "widgets", false), ("fmtlib", "fmt", true)]);
        assert!(advise("acme", "widgets", &with_self).is_empty());
    }

    #[test]
    fn skeleton_equality_flags_packages_and_scopes() {
        let existing = corpus(&[("fmtlib", "fmt", true)]);
        // The homoglyph scope squat: the full name and the scope part
        // both collide.
        assert_eq!(
            rendered("fmtl1b", "fmt", &existing),
            [
                "confusable_package (fmtlib/fmt)",
                "confusable_scope (fmtlib)"
            ]
        );
        // A separator-drop inside one scope is still a package
        // collision (the deterministic publish reject only covers the
        // `-`/`_` interchange, not removal).
        assert_eq!(
            rendered("fmtlib", "f-mt", &existing),
            ["confusable_package (fmtlib/fmt)"]
        );
    }

    #[test]
    fn near_names_flag_across_scopes_only() {
        let existing = corpus(&[("fmtlib", "fmt", true), ("acme", "json", true)]);
        // One edit away on the folded full name, in a different scope:
        // the near-scope squat.
        assert_eq!(
            rendered("fmtlib2", "fmt", &existing),
            ["near_name (fmtlib/fmt)"]
        );
        // A sibling next to the same scope's own package does not
        // abstain: members publish there, not squatters.
        assert!(advise("acme", "jsonc", &existing).is_empty());
        // The distance is over the full name, so the same name part
        // under an unrelated scope is far away; so is a two-edit
        // scope.
        assert!(advise("bob", "json", &existing).is_empty());
        assert!(advise("fmtlib22", "fmt", &existing).is_empty());
    }

    #[test]
    fn unvetted_corpus_rows_still_collide() {
        // Two confusable names racing through publish: neither has a
        // verdict, each sees the other's package row and abstains -
        // the operator resolves the pair.
        let racing = corpus(&[("alpha", "foo-bar", false), ("alpha", "foobar", false)]);
        assert_eq!(
            rendered("alpha", "foobar", &racing),
            ["confusable_package (alpha/foo-bar)"]
        );
    }

    #[test]
    fn profanity_matches_folded_substrings_without_echoing_the_term() {
        let empty = corpus(&[]);
        assert_eq!(rendered("acme", "nigger2", &empty), ["profanity"]);
        // Separator- and homoglyph-laundered spellings fold back.
        assert_eq!(rendered("n-igger", "lib", &empty), ["profanity"]);
        assert_eq!(rendered("acme", "n1gga", &empty), ["profanity"]);
        // The accepted Scunthorpe cost, pinned: a containing word
        // abstains and costs the operator one review.
        assert_eq!(rendered("acme", "snigger", &empty), ["profanity"]);
        assert!(advise("acme", "scunthorpe", &empty).is_empty());
    }

    #[test]
    fn within_one_edit_covers_substitution_insertion_deletion() {
        assert!(within_one_edit("abc", "abc"));
        assert!(within_one_edit("abc", "abz"));
        assert!(within_one_edit("abc", "abcd"));
        assert!(within_one_edit("abcd", "acd"));
        assert!(!within_one_edit("abc", "add"));
        assert!(!within_one_edit("abc", "abcde"));
        assert!(!within_one_edit("", "ab"));
        assert!(within_one_edit("", "a"));
    }
}
