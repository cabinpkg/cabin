//! How the gate's concurrent phases split the CPUs, so concurrent cargo
//! invocations do not each default `--jobs` to the full count.

/// The shares each concurrent phase passes to `--jobs`.
#[derive(Debug, PartialEq, Eq)]
pub struct Jobs {
    /// False below four cores, where four phases of at least one job
    /// each would exceed the core count.  The gate then runs serially
    /// and every phase gets the whole machine.
    pub parallel: bool,
    pub test: u32,
    pub clippy: u32,
    pub check: u32,
    pub doc: u32,
}

/// The split, for a given core count.
///
/// With four or more the test phase takes half and the remainder is
/// distributed - never independently rounded up, which compounds
/// (2+2+3+2 = 9 jobs on a 5-core host).  The shares sum to the core
/// count except at four cores, where doc's floor of one job makes five.
#[must_use]
pub fn split(cores: u32) -> Jobs {
    if cores < 4 {
        return Jobs {
            parallel: false,
            test: cores,
            clippy: cores,
            check: cores,
            doc: cores,
        };
    }
    let test = cores / 2;
    let left = cores - test;
    let each = left / 3;
    let remainder = left % 3;
    Jobs {
        parallel: true,
        test,
        clippy: each + u32::from(remainder >= 1),
        check: each + u32::from(remainder >= 2),
        doc: each.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shares sum to the detected capacity, except the four-core
    /// floor where keeping every phase alive costs one extra.
    #[test]
    fn the_split_divides_the_cores_between_the_phases() {
        for cores in 1..4 {
            let jobs = split(cores);
            assert!(!jobs.parallel, "{cores} cores should stay serial");
            assert_eq!(
                (jobs.test, jobs.clippy, jobs.check, jobs.doc),
                (cores, cores, cores, cores)
            );
        }
        let expected = [
            // cores, test, clippy, check, doc
            (4, 2, 1, 1, 1),
            (5, 2, 1, 1, 1),
            (6, 3, 1, 1, 1),
            (7, 3, 2, 1, 1),
            (8, 4, 2, 1, 1),
            (9, 4, 2, 2, 1),
            (10, 5, 2, 2, 1),
            (11, 5, 2, 2, 2),
            (12, 6, 2, 2, 2),
            (16, 8, 3, 3, 2),
        ];
        for (cores, test, clippy, check, doc) in expected {
            let jobs = split(cores);
            assert!(jobs.parallel, "{cores} cores should be parallel");
            assert_eq!(
                (jobs.test, jobs.clippy, jobs.check, jobs.doc),
                (test, clippy, check, doc),
                "the split for {cores} cores"
            );
            let sum = jobs.test + jobs.clippy + jobs.check + jobs.doc;
            let allowance = if cores == 4 { cores + 1 } else { cores };
            assert_eq!(sum, allowance, "the shares for {cores} cores sum wrong");
        }
    }
}
