//! A Kani report must show every `kani::cover!` satisfied.
//!
//! Kani reports a harness `SUCCESSFUL` even when some of its covers are
//! never satisfied - unreachable or unsatisfiable, the sign of
//! assumptions that leave nothing to check - so the proof job saves the
//! report and this guard refuses that pass.
//! Ceiling: it reads Kani 0.68's text summary (`Checking harness <name>...`,
//! then ` ** <met> of <total> cover properties satisfied`, with any
//! unreachable or undetermined count after it), so a report without any
//! summary line is a violation rather than a pass.

/// One violation per harness whose covers are not all satisfied, and
/// one if the report carries no cover summary at all.
#[must_use]
pub fn check(report: &str) -> Vec<String> {
    let mut harness = "";
    let mut summaries = 0;
    let mut violations = Vec::new();
    for line in report.lines() {
        if let Some(name) = line.strip_prefix("Checking harness ") {
            harness = name.trim_end_matches("...");
        } else if let Some(summary) = line.strip_prefix(" ** ")
            && let Some((counts, _)) = summary.split_once(" cover properties satisfied")
        {
            // Kani may append `(<n> unreachable)` or `(<n> undetermined)`.
            summaries += 1;
            match counts.split_once(" of ") {
                Some((met, total)) if met == total => {}
                _ => violations.push(format!("{harness}: {summary}")),
            }
        }
    }
    if summaries == 0 {
        violations.push("the Kani report has no cover summary".to_owned());
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "\
Checking harness quota::verification::a...
SUMMARY:
 ** 0 of 180 failed
 ** 15 of 15 cover properties satisfied
VERIFICATION:- SUCCESSFUL
Checking harness quota::verification::b...
SUMMARY:
 ** 0 of 144 failed
 ** 8 of 8 cover properties satisfied
VERIFICATION:- SUCCESSFUL
";

    #[test]
    fn every_cover_satisfied_passes() {
        assert!(check(REPORT).is_empty());
    }

    #[test]
    fn an_unsatisfied_cover_names_its_harness() {
        let report = REPORT.replace(" 8 of 8 ", " 7 of 8 ");
        assert_eq!(
            check(&report),
            ["quota::verification::b: 7 of 8 cover properties satisfied"]
        );
    }

    #[test]
    fn an_unreachable_cover_fails_beside_a_satisfied_harness() {
        let report = REPORT.replace(
            " 8 of 8 cover properties satisfied",
            " 7 of 8 cover properties satisfied (1 unreachable)",
        );
        assert_eq!(
            check(&report),
            ["quota::verification::b: 7 of 8 cover properties satisfied (1 unreachable)"]
        );
    }

    #[test]
    fn a_report_without_a_cover_summary_fails() {
        assert_eq!(check("VERIFICATION:- SUCCESSFUL\n").len(), 1);
    }
}
