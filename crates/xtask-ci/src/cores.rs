//! How many CPUs this process may actually use, and how the gate's
//! concurrent phases divide them.
//!
//! Concurrent cargo invocations each default `--jobs` to the CPU
//! count, so four unbounded phases could peak at 4N compiler jobs and
//! swap or OOM a smaller host.  The split is static instead: the test
//! phase gets half (it is the longest, and `--jobs` caps only its
//! compilation - test-running parallelism is libtest's own), the rest
//! a quarter each.  The ~1.25N aggregate peak is deliberate mild
//! oversubscription.
//!
//! "Available" means available to *this process*, not merely online:
//! agent and CI containers often run under an affinity mask or a
//! cgroup CPU quota, and a cap derived from the online count would
//! overshoot exactly on the constrained hosts it exists for.

use std::path::{Path, PathBuf};

/// The shares each concurrent phase passes to `--jobs`.
#[derive(Debug, PartialEq, Eq)]
pub struct Jobs {
    /// False below four cores, where the rounded-up shares would sum
    /// back to the oversubscription the cap exists to prevent.  The
    /// gate then runs serially and every phase gets the whole machine,
    /// which was already the right shape there.
    pub parallel: bool,
    pub test: u32,
    pub clippy: u32,
    pub check: u32,
    pub doc: u32,
}

/// The split, for a given core count.
///
/// With four or more the shares are exact: the test phase takes half
/// and the remainder is distributed - never independently rounded up,
/// which compounds (2+2+3+2 = 9 jobs on a 5-core host).
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

/// The CPUs this process may use.
#[must_use]
pub fn effective() -> u32 {
    quota(Path::new("/proc"), online())
}

/// The core count after applying the tightest cgroup CPU quota found
/// under `proc`.
///
/// `proc` is a parameter so the walk is testable: this box is macOS
/// and has no `/proc` at all, which is exactly how an earlier version
/// of this shipped with the whole cgroup path unreachable and its
/// tests passing anyway.
#[must_use]
pub fn quota(proc: &Path, mut cores: u32) -> u32 {
    let self_cgroup = proc.join("self/cgroup");
    let Ok(own) = std::fs::read_to_string(&self_cgroup) else {
        return cores.max(1);
    };
    let mountinfo = std::fs::read_to_string(proc.join("self/mountinfo")).unwrap_or_default();

    // cgroup v2: `cpu.max` holds "<quota> <period>" on one line.
    if let Some((root, mount)) = mount(&mountinfo, |kind, _| kind == "cgroup2") {
        let path = own
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .unwrap_or("");
        for node in ancestors(&root, &mount, path) {
            if let Some((q, p)) = std::fs::read_to_string(node.join("cpu.max"))
                .ok()
                .and_then(|text| pair(&text))
            {
                cores = clamp(cores, q, p);
            }
        }
    }

    // cgroup v1: the pair is split across two single-value files.
    if let Some((root, mount)) = mount(&mountinfo, |kind, options| {
        kind == "cgroup" && options.split(',').any(|option| option == "cpu")
    }) {
        // `<id>:<controllers>:<path>` - a controller name never holds a
        // colon, but a cgroup path legally can.
        let path = own
            .lines()
            .find_map(|line| {
                let (_, rest) = line.split_once(':')?;
                let (controllers, path) = rest.split_once(':')?;
                controllers
                    .split(',')
                    .any(|option| option == "cpu")
                    .then_some(path)
            })
            .unwrap_or("");
        for node in ancestors(&root, &mount, path) {
            let read = |name: &str| {
                std::fs::read_to_string(node.join(name))
                    .ok()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            };
            if let (Some(q), Some(p)) = (read("cpu.cfs_quota_us"), read("cpu.cfs_period_us")) {
                cores = clamp(cores, q, p);
            }
        }
    }
    cores.max(1)
}

/// `nproc`'s answer.  `std::thread::available_parallelism` honors the
/// affinity mask the same way, and additionally the cgroup quota -
/// harmless here, because the walk takes a minimum either way.
///
/// `nproc` also honors `OMP_NUM_THREADS`/`OMP_THREAD_LIMIT`, which the
/// standard library does not, so this reads those itself rather than
/// quietly using more of a host than the operator asked for.
fn online() -> u32 {
    let detected = std::thread::available_parallelism()
        .map_or(4, |count| u32::try_from(count.get()).unwrap_or(4));
    ["OMP_THREAD_LIMIT", "OMP_NUM_THREADS"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter_map(|value| value.trim().parse::<u32>().ok())
        .filter(|limit| *limit > 0)
        .fold(detected, u32::min)
}

/// `ceil(quota / period)`, floored at 1, applied only when it is
/// strictly tighter than what we already have.
#[must_use]
pub fn clamp(cores: u32, quota: u64, period: u64) -> u32 {
    if period == 0 {
        return cores;
    }
    let allowed = u32::try_from(quota.div_ceil(period))
        .unwrap_or(u32::MAX)
        .max(1);
    cores.min(allowed)
}

/// A `<quota> <period>` pair.  `max` (v2) and `-1` (v1) both mean no
/// limit and fail the parse rather than clamping to something.
#[must_use]
pub fn pair(text: &str) -> Option<(u64, u64)> {
    let mut fields = text.split_whitespace();
    let quota = fields.next()?.parse().ok()?;
    let period = fields.next()?.parse().ok()?;
    Some((quota, period))
}

/// The mount root and mountpoint of the first matching cgroup mount.
///
/// A mountinfo line is `... root mountpoint ... - fstype source
/// super_options`, so the filesystem type is the FIRST field after the
/// ` - ` separator and the options are the third.  Reading those
/// positions wrong is how the v2 branch of this silently matched
/// nothing.
#[must_use]
pub fn mount(mountinfo: &str, matches: impl Fn(&str, &str) -> bool) -> Option<(String, String)> {
    mountinfo.lines().find_map(|line| {
        let (before, after) = line.split_once(" - ")?;
        let mut post = after.split_whitespace();
        let kind = post.next()?;
        let _source = post.next()?;
        let options = post.next().unwrap_or("");
        if !matches(kind, options) {
            return None;
        }
        let mut fields = before.split_whitespace().skip(3);
        let root = fields.next()?.to_owned();
        let mountpoint = fields.next()?.to_owned();
        Some((root, mountpoint))
    })
}

/// Every directory from the process's own cgroup up to the mountpoint.
///
/// The prefix check is component-aware on purpose: a mount root of
/// `/tenant` must not strip from `/tenant2/job`, which is a textual
/// prefix but a sibling cgroup.  A path outside the mount's root
/// degrades to the mountpoint itself.
#[must_use]
pub fn ancestors(root: &str, mount: &str, path: &str) -> Vec<PathBuf> {
    let relative = if root == "/" {
        path
    } else if path == root {
        ""
    } else {
        path.strip_prefix(root)
            .filter(|rest| rest.starts_with('/'))
            .unwrap_or_default()
    };
    let mut nodes = Vec::new();
    let mut node = PathBuf::from(format!("{mount}{relative}"));
    let mount = Path::new(mount);
    loop {
        nodes.push(node.clone());
        if node == mount {
            break;
        }
        match node.parent() {
            Some(parent) if parent.starts_with(mount) => node = parent.to_path_buf(),
            _ => break,
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table the shell computed, cell for cell.  The shares sum to
    /// the detected capacity, except the four-core floor where keeping
    /// every phase alive costs one extra.
    #[test]
    fn the_split_matches_the_shell_arithmetic() {
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

    #[test]
    fn a_quota_clamps_only_when_it_is_tighter() {
        assert_eq!(clamp(10, 250_000, 100_000), 3, "ceil(2.5)");
        assert_eq!(clamp(10, 150_000, 100_000), 2);
        assert_eq!(clamp(10, 50_000, 100_000), 1);
        assert_eq!(clamp(10, 0, 100_000), 1, "floored at one");
        assert_eq!(clamp(10, 1_200_000, 100_000), 10, "never widens");
        assert_eq!(clamp(10, 800_000, 100_000), 8);
        assert_eq!(clamp(10, 100_000, 0), 10, "a zero period is no limit");
    }

    /// `max` (v2) and `-1` (v1) both mean no limit and must fail the
    /// parse rather than clamping to something.
    #[test]
    fn an_unlimited_quota_does_not_parse() {
        assert_eq!(pair("200000 100000"), Some((200_000, 100_000)));
        assert_eq!(pair("200000 100000\n"), Some((200_000, 100_000)));
        assert_eq!(pair("max 100000"), None);
        assert_eq!(pair("-1 100000"), None);
        assert_eq!(pair(""), None);
        assert_eq!(pair("200000"), None);
    }

    /// The component-aware prefix check: a mount root of `/tenant`
    /// must not strip from the sibling `/tenant2/job`.
    #[test]
    fn the_ancestor_walk_is_component_aware() {
        assert_eq!(
            ancestors("/", "/sys/fs/cgroup", "/user.slice/job"),
            [
                PathBuf::from("/sys/fs/cgroup/user.slice/job"),
                PathBuf::from("/sys/fs/cgroup/user.slice"),
                PathBuf::from("/sys/fs/cgroup"),
            ]
        );
        assert_eq!(
            ancestors("/tenant", "/sys/fs/cgroup", "/tenant/job"),
            [
                PathBuf::from("/sys/fs/cgroup/job"),
                PathBuf::from("/sys/fs/cgroup"),
            ]
        );
        // A sibling that merely shares a textual prefix degrades to the
        // mountpoint rather than stripping.
        assert_eq!(
            ancestors("/tenant", "/sys/fs/cgroup", "/tenant2/job"),
            [PathBuf::from("/sys/fs/cgroup")]
        );
        assert_eq!(
            ancestors("/tenant", "/sys/fs/cgroup", "/tenant"),
            [PathBuf::from("/sys/fs/cgroup")]
        );
    }

    /// No `/proc` at all - every macOS run - answers the online count
    /// rather than failing.
    #[test]
    fn a_host_without_proc_answers_the_online_count() {
        assert!(effective() >= 1);
        let missing = std::path::Path::new("/nonexistent-proc-for-this-test");
        assert_eq!(quota(missing, 10), 10);
    }

    /// A mountinfo line is `... root mountpoint ... - fstype source
    /// super_options`.  Reading those positions wrong is how the v2
    /// branch of this once matched nothing at all while every test
    /// still passed, because this box has no `/proc`.
    #[test]
    fn the_cgroup_mount_is_found_at_the_right_fields() {
        let line = "31 23 0:27 / /sys/fs/cgroup rw,nosuid,relatime shared:9 \
- cgroup2 cgroup2 rw,nsdelegate";
        let line = line.replace(" \\\n", " ");
        assert_eq!(
            mount(&line, |kind, _| kind == "cgroup2"),
            Some(("/".to_owned(), "/sys/fs/cgroup".to_owned()))
        );
        let v1 = "30 23 0:26 /tenant /sys/fs/cgroup/cpu rw,relatime \
- cgroup cgroup rw,cpu,cpuacct";
        let v1 = v1.replace(" \\\n", " ");
        assert_eq!(
            mount(&v1, |kind, options| kind == "cgroup"
                && options.split(',').any(|o| o == "cpu")),
            Some(("/tenant".to_owned(), "/sys/fs/cgroup/cpu".to_owned()))
        );
        // The controller list must actually contain `cpu`.
        assert_eq!(
            mount(&v1, |kind, options| kind == "cgroup"
                && options.split(',').any(|o| o == "memory")),
            None
        );
    }

    /// The whole v2 walk, over a fixture `/proc` WITH the mountpoint
    /// inside the fixture too: pointing it at the host's real
    /// `/sys/fs/cgroup` made the assertion depend on whatever quota
    /// the host happened to run under, and a walk that had gone dead
    /// again would still have passed.
    #[test]
    fn a_v2_quota_clamps_through_the_full_walk() {
        let proc = assert_fs::TempDir::new().unwrap();
        let root = proc.path();
        let cgroup = root.join("cgroup");
        std::fs::create_dir_all(root.join("self")).unwrap();
        std::fs::create_dir_all(cgroup.join("user.slice/job")).unwrap();
        std::fs::write(
            root.join("self/mountinfo"),
            format!(
                "31 23 0:27 / {} rw,relatime - cgroup2 cgroup2 rw,nsdelegate\n",
                cgroup.display()
            ),
        )
        .unwrap();
        std::fs::write(root.join("self/cgroup"), "0::/user.slice/job\n").unwrap();
        // Without a readable cpu.max anywhere, nothing clamps.
        assert_eq!(quota(root, 10), 10);
        // A quota on an ancestor clamps: the walk has to climb.
        std::fs::write(cgroup.join("user.slice/cpu.max"), "200000 100000\n").unwrap();
        assert_eq!(quota(root, 10), 2);
        // The tightest level wins.
        std::fs::write(cgroup.join("user.slice/job/cpu.max"), "50000 100000\n").unwrap();
        assert_eq!(quota(root, 10), 1);
    }

    /// The whole v1 walk: the pair comes from two single-value files
    /// read separately - feeding one of them to the pair parse yields
    /// `None` forever, which is the second way this shipped dead.
    #[test]
    fn a_v1_quota_clamps_through_the_full_walk() {
        let proc = assert_fs::TempDir::new().unwrap();
        let root = proc.path();
        let cpu = root.join("cgroup-cpu");
        std::fs::create_dir_all(root.join("self")).unwrap();
        std::fs::create_dir_all(cpu.join("job")).unwrap();
        std::fs::write(
            root.join("self/mountinfo"),
            format!(
                "30 23 0:26 / {} rw,relatime - cgroup cgroup rw,cpu,cpuacct\n",
                cpu.display()
            ),
        )
        .unwrap();
        std::fs::write(root.join("self/cgroup"), "2:cpu,cpuacct:/job\n").unwrap();
        assert_eq!(quota(root, 10), 10, "no files, no clamp");
        std::fs::write(cpu.join("job/cpu.cfs_quota_us"), "150000\n").unwrap();
        std::fs::write(cpu.join("job/cpu.cfs_period_us"), "100000\n").unwrap();
        assert_eq!(quota(root, 10), 2, "ceil(1.5)");
        // `-1` means unlimited: it must fail the parse, not clamp.
        std::fs::write(cpu.join("cpu.cfs_quota_us"), "-1\n").unwrap();
        std::fs::write(cpu.join("cpu.cfs_period_us"), "100000\n").unwrap();
        assert_eq!(quota(root, 10), 2, "an unlimited ancestor adds nothing");
    }
}
