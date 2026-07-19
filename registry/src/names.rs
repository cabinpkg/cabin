//! The name-fidelity vocabulary shared by publish and the claim flow
//! (`docs/architecture.md`, "Name fidelity"): the reserved-name list
//! and the confusability skeleton fold.

/// Reserved DOS device stems, refused as package and scope names.
/// Both become client-side directory names (the vendor and cache
/// layouts), where Windows resolves a reserved stem to a character
/// device instead of a directory - the archive-path predicate never
/// sees them, so the refusal must happen at the name level.
///
/// A lowercase mirror of `cabin-fs`'s `DOS_DEVICE_NAMES`: the worker
/// is a standalone wasm32 workspace that cannot depend on the crate
/// at runtime, so the parity test below pins this list to the shared
/// predicate instead. The superscript-digit `COM`/`LPT` forms the
/// predicate also covers cannot occur here - the name grammars are
/// ASCII.
const DOS_DEVICE_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Project vocabulary reserved as package and scope names, so official
/// spellings can never be squatted or shadowed. Operator-maintained:
/// extend when the project starts speaking a new name, never shrink
/// (a released name would instantly be claimable).
const PROJECT_RESERVED: &[&str] = &["cabin", "cabinpkg", "std", "core"];

/// Whether `name` is refused as a package name (publish `400`) or a
/// scope name (the claim flow's uniform denied redirect).
pub fn is_reserved(name: &str) -> bool {
    DOS_DEVICE_STEMS.contains(&name) || PROJECT_RESERVED.contains(&name)
}

/// The confusability skeleton: `-` and `_` fold away, `{1, i} -> l`
/// and `0 -> o`. Two names with equal skeletons are treated as
/// visually interchangeable - the homoglyph-style squat (`fmtl1b` vs
/// `fmtlib`) - by the claim refusal here and the verifier's name
/// advisories (`crates/cabin-registry-verify`), which mirrors this
/// map structurally.
pub fn skeleton(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            '-' | '_' => None,
            '1' | 'i' => Some('l'),
            '0' => Some('o'),
            c => Some(c),
        })
        .collect()
}

/// Parses a comma-separated list of exact scope names (the
/// `CLAIM_SKELETON_EXEMPT_SCOPES` operator override); entries are
/// trimmed and empty entries - including the whole variable being
/// empty, meaning no exemptions - are skipped.
pub fn parse_scope_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_cover_devices_and_project_vocabulary() {
        for name in ["con", "nul", "com1", "lpt9", "cabin", "std", "core"] {
            assert!(is_reserved(name), "name: {name:?}");
        }
        for name in [
            "fmt", "com", "com10", "lpt0", "console", "stdlib", "cabinet",
        ] {
            assert!(!is_reserved(name), "name: {name:?}");
        }
    }

    /// The stem mirror must stay byte-equal to what the shared
    /// `cabin-fs` predicate flags. Brute force over every
    /// grammar-expressible string of at most 4 bytes - every ASCII DOS
    /// device stem is at most 4 bytes, so within the `[a-z0-9_-]`
    /// grammars this pins set equality in both directions; a future
    /// longer stem in `cabin-fs` would need this bound raised.
    #[test]
    fn device_stems_mirror_the_shared_cabin_fs_predicate() {
        use cabin_fs::path::{PortabilityViolation, component_portability};

        let alphabet: Vec<char> = ('a'..='z').chain('0'..='9').chain(['-', '_']).collect();
        let mut candidates: Vec<String> = alphabet.iter().map(ToString::to_string).collect();
        let mut previous = candidates.clone();
        for _ in 0..3 {
            previous = previous
                .iter()
                .flat_map(|prefix| {
                    alphabet.iter().map(move |c| {
                        let mut next = prefix.clone();
                        next.push(*c);
                        next
                    })
                })
                .collect();
            candidates.extend(previous.iter().cloned());
        }
        for candidate in candidates {
            let shared =
                component_portability(&candidate) == Some(PortabilityViolation::WindowsDeviceName);
            assert_eq!(
                DOS_DEVICE_STEMS.contains(&candidate.as_str()),
                shared,
                "candidate: {candidate:?}"
            );
        }
    }

    #[test]
    fn skeleton_folds_separators_and_confusable_digits() {
        assert_eq!(skeleton("fmtlib"), "fmtllb");
        assert_eq!(skeleton("fmtl1b"), "fmtllb");
        assert_eq!(skeleton("f-mt_lib"), "fmtllb");
        assert_eq!(skeleton("z0log"), "zolog");
        assert_eq!(skeleton("zolog"), "zolog");
        assert_ne!(skeleton("fmt"), skeleton("fmts"));
    }

    #[test]
    fn skeleton_is_idempotent() {
        for name in ["fmtlib", "fmtl1b", "a-b_c", "0i1l"] {
            let once = skeleton(name);
            assert_eq!(skeleton(&once), once, "name: {name:?}");
        }
    }

    #[test]
    fn parse_scope_list_trims_and_skips_empties() {
        assert_eq!(parse_scope_list(""), Vec::<String>::new());
        assert_eq!(parse_scope_list(" , ,"), Vec::<String>::new());
        assert_eq!(
            parse_scope_list("fmtlib, jsmith1 ,,x"),
            vec!["fmtlib", "jsmith1", "x"]
        );
    }
}
