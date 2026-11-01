use std::path::Path;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use camino::Utf8PathBuf;
use predicates::prelude::*;

use super::*;

const DAY: u64 = 20_000;

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        tmp.child("ws").create_dir_all().unwrap();
        tmp.child("build").create_dir_all().unwrap();
        Self { tmp }
    }

    fn build_dir(&self) -> PathBuf {
        self.tmp.child("build").to_path_buf()
    }

    fn file(&self, rel: &str) -> assert_fs::fixture::ChildPath {
        self.tmp.child("build").child(rel)
    }

    /// Write each product and record its use on `day`; returns whether
    /// a collection is due.
    fn build(&self, day: u64, products: &[&str]) -> bool {
        let paths: Vec<Utf8PathBuf> = products
            .iter()
            .map(|rel| {
                self.file(rel).write_str(rel).unwrap();
                Utf8PathBuf::from_path_buf(self.file(rel).to_path_buf()).unwrap()
            })
            .collect();
        record_use(
            &self.build_dir(),
            paths.iter().map(Utf8PathBuf::as_path),
            day,
        )
        .unwrap()
    }

    fn collect(&self, policy: Policy, dry_run: bool) -> CollectReport {
        self.try_collect(policy, dry_run, None).unwrap()
    }

    fn try_collect(
        &self,
        policy: Policy,
        dry_run: bool,
        time_limit: Option<Duration>,
    ) -> Result<CollectReport, GcError> {
        collect(&CollectRequest {
            build_dir: &self.build_dir(),
            workspace_root: &self.tmp.child("ws"),
            package_roots: &[],
            protected_source_paths: &[],
            policy,
            dry_run,
            time_limit,
        })
    }

    fn collected(&self, report: &CollectReport) -> Vec<String> {
        report
            .collected
            .iter()
            .map(|c| {
                let rel = c.path.strip_prefix(self.build_dir()).unwrap();
                rel.to_str().unwrap().replace('\\', "/")
            })
            .collect()
    }
}

fn max_age(days: u32) -> Policy {
    Policy {
        max_age: NonZeroU32::new(days).unwrap(),
        max_size: None,
    }
}

/// Record a use on each of `days` active days, starting the day after
/// `from`, of `products`; returns the last day.
fn work_days(fx: &Fixture, from: u64, days: u64, products: &[&str]) -> u64 {
    for day in from + 1..=from + days {
        fx.build(day, products);
    }
    from + days
}

const OLD_OBJ: &str = "dev/packages/app/obj/app/src/old.cc.o";
const NEW_OBJ: &str = "dev/packages/app/obj/app/src/new.cc.o";
const APP: &str = "dev/packages/app/app";

#[test]
fn unused_products_go_once_older_than_max_age_and_used_ones_stay() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ, APP]);
    // `old.cc` was renamed: later builds use only the new object.
    let day = work_days(&fx, DAY, 2, &[NEW_OBJ, APP]);
    fx.file("dev/packages/app/obj/app/src/notes.txt")
        .write_str("not Cabin's")
        .unwrap();

    // Two active days since its last use: kept by a two-day policy.
    assert!(fx.collect(max_age(2), false).collected.is_empty());
    fx.file(OLD_OBJ).assert(predicate::path::exists());

    work_days(&fx, day, 1, &[NEW_OBJ, APP]);
    let report = fx.collect(max_age(2), false);
    assert_eq!(fx.collected(&report), [OLD_OBJ]);
    assert_eq!(
        report.collected[0].reason,
        Reason::Unused { days: 3 },
        "used on index 1, now index 4"
    );
    assert_eq!(report.collected_bytes(), OLD_OBJ.len() as u64);
    fx.file(OLD_OBJ).assert(predicate::path::missing());
    fx.file(NEW_OBJ).assert(predicate::path::exists());
    fx.file(APP).assert(predicate::path::exists());
    // A file no plan declared is never touched.
    fx.file("dev/packages/app/obj/app/src/notes.txt")
        .assert(predicate::path::exists());
}

#[test]
fn absence_from_a_partial_plan_does_not_collect_a_product() {
    let fx = Fixture::new();
    let lib = "dev/packages/lib/liblib.a";
    fx.build(DAY, &[lib, APP]);
    // `cabin build -p app`-style plans alternate with full ones.
    let day = work_days(&fx, DAY, 3, &[APP]);
    fx.build(day + 1, &[lib, APP]);
    work_days(&fx, day + 1, 3, &[APP]);
    assert!(fx.collect(max_age(3), false).collected.is_empty());
    fx.file(lib).assert(predicate::path::exists());
}

#[test]
fn idle_days_and_clock_jumps_age_nothing() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    // A month off, then a clock briefly set years ahead and back.
    fx.build(DAY + 30, &[APP]);
    fx.build(DAY + 4000, &[APP]);
    fx.build(DAY + 31, &[APP]);
    let report = fx.collect(max_age(2), false);
    assert!(report.collected.is_empty(), "only two active days passed");
    fx.file(OLD_OBJ).assert(predicate::path::exists());
}

#[test]
fn a_spelling_that_differs_only_in_case_counts_as_used() {
    let fx = Fixture::new();
    let old = "dev/packages/app/obj/app/src/Util.cc.o";
    let new = "dev/packages/app/obj/app/src/util.cc.o";
    fx.build(DAY, &[old]);
    // `Util.cc` became `util.cc`; on a case-insensitive file system
    // both spellings name one file.
    work_days(&fx, DAY, 2, &[new]);
    assert!(fx.collect(max_age(1), false).collected.is_empty());
    fx.file(new).assert(predicate::path::exists());
}

#[test]
fn spellings_that_differ_only_in_case_are_sized_once() {
    let fx = Fixture::new();
    let old = "dev/packages/app/obj/app/src/Util.cc.o";
    let new = "dev/packages/app/obj/app/src/util.cc.o";
    fx.build(DAY, &[old, APP]);
    work_days(&fx, DAY, 2, &[new]);
    // Each file holds its own path.
    let fits = (new.len() + APP.len()) as u64;
    let report = fx.collect(
        Policy {
            max_size: Some(fits),
            ..max_age(30)
        },
        false,
    );
    assert!(report.collected.is_empty(), "{:?}", report.collected);
    assert_eq!(report.retained_bytes, Some(fits));
}

#[test]
fn a_missing_spelling_does_not_hide_an_existing_one_from_the_size_budget() {
    let fx = Fixture::new();
    let old = "dev/packages/app/obj/app/src/Util.cc.o";
    let new = "dev/packages/app/obj/app/src/util.cc.o";
    // The old spelling's object is gone (a `cabin clean` of the profile,
    // say) but the catalog still lists it.
    let missing = Utf8PathBuf::from_path_buf(fx.file(old).to_path_buf()).unwrap();
    record_use(&fx.build_dir(), [missing.as_path()], DAY).unwrap();
    fx.build(DAY, &[new]);
    work_days(&fx, DAY, 2, &[APP]);
    let report = fx.collect(
        Policy {
            max_size: Some(0),
            ..max_age(30)
        },
        false,
    );
    assert_eq!(report.collected.len(), 1, "{:?}", report.collected);
    fx.file(new).assert(predicate::path::missing());
}

#[cfg(windows)]
#[test]
fn a_drive_relative_name_is_not_a_relative_path() {
    assert!(!is_valid_relative("dev/C:src/main.cpp"));
    assert!(is_valid_relative("dev/src/main.cpp"));
}

#[test]
fn collection_is_due_once_per_active_day_until_it_completes() {
    let fx = Fixture::new();
    assert!(!fx.build(DAY, &[APP]));
    assert!(!fx.build(DAY, &[APP]));
    assert!(fx.build(DAY + 1, &[APP]));
    assert!(fx.build(DAY + 1, &[APP]), "due until a collection runs");
    fx.collect(Policy::default(), false);
    assert!(!fx.build(DAY + 1, &[APP]));
}

#[test]
fn a_plan_recorded_on_an_earlier_active_day_is_recorded_again() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ, APP]);
    let day = work_days(&fx, DAY, 2, &[NEW_OBJ]);
    fx.build(day, &[OLD_OBJ, APP]);
    assert!(fx.collect(max_age(1), false).collected.is_empty());
}

#[test]
fn a_plan_recorded_today_with_one_more_product_records_it() {
    let fx = Fixture::new();
    fx.build(DAY, &[APP]);
    fx.build(DAY, &[OLD_OBJ, APP]);
    work_days(&fx, DAY, 2, &[APP]);
    assert_eq!(fx.collected(&fx.collect(max_age(1), false)), [OLD_OBJ]);
}

#[test]
fn a_recording_that_failed_is_retried_by_the_next_command() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ, APP]);
    // A directory where the catalog is staged fails its write, after
    // the catalog was read on the next active day.
    let staging = fx
        .build_dir()
        .join(CATALOG_DIR)
        .join(CATALOG_FILE)
        .with_extension(format!("{}.tmp", std::process::id()));
    std::fs::create_dir(&staging).unwrap();
    let products =
        [OLD_OBJ, APP].map(|rel| Utf8PathBuf::from_path_buf(fx.file(rel).to_path_buf()).unwrap());
    record_use(
        &fx.build_dir(),
        products.iter().map(Utf8PathBuf::as_path),
        DAY + 1,
    )
    .unwrap_err();
    std::fs::remove_dir(&staging).unwrap();

    fx.build(DAY + 1, &[OLD_OBJ, APP]);
    work_days(&fx, DAY + 1, 1, &[NEW_OBJ]);
    assert!(
        fx.collect(max_age(1), false).collected.is_empty(),
        "used on the previous active day"
    );
}

#[test]
fn a_plan_recorded_today_does_not_wait_for_another_recording() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ, APP]);
    let _other = BuildDirLock::exclusive(&fx.build_dir().join(CATALOG_DIR), || {}).unwrap();
    let products =
        [OLD_OBJ, APP].map(|rel| Utf8PathBuf::from_path_buf(fx.file(rel).to_path_buf()).unwrap());
    let build_dir = fx.build_dir();
    let (sender, recorded) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = record_use(&build_dir, products.iter().map(Utf8PathBuf::as_path), DAY);
        sender.send(result.map_err(|err| err.to_string())).unwrap();
    });
    recorded
        .recv_timeout(Duration::from_secs(10))
        .expect("recorded again")
        .unwrap();
}

#[test]
fn a_plan_whose_catalog_another_replaced_records_again() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    std::fs::write(fx.build_dir().join(CATALOG_DIR).join(CATALOG_FILE), "{").unwrap();
    // Another plan replaces the unreadable catalog, forgetting `old.cc.o`.
    fx.build(DAY, &[APP]);
    fx.build(DAY, &[OLD_OBJ]);
    work_days(&fx, DAY, 2, &[NEW_OBJ]);
    let mut collected = fx.collected(&fx.collect(max_age(1), false));
    collected.sort();
    assert_eq!(collected, [APP, OLD_OBJ]);
}

/// A command reads the catalog, another rewrites it, and the first writes
/// back its own snapshot: without one-at-a-time recording the second's
/// update is lost.
#[test]
fn a_recording_waits_for_another_and_keeps_its_update() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    let catalog = fx.build_dir().join(CATALOG_DIR);
    let other = BuildDirLock::exclusive(&catalog, || {}).unwrap();

    fx.file(APP).write_str("app").unwrap();
    let app = Utf8PathBuf::from_path_buf(fx.file(APP).to_path_buf()).unwrap();
    let build_dir = fx.build_dir();
    let (sender, recorded) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        sender.send(None).unwrap();
        let result = record_use(&build_dir, [app.as_path()], DAY + 1);
        sender
            .send(Some(result.map_err(|err| err.to_string())))
            .unwrap();
    });
    // Timed from the thread's start, which a loaded machine can delay.
    assert!(recorded.recv().unwrap().is_none());
    assert!(
        recorded.recv_timeout(Duration::from_millis(200)).is_err(),
        "recorded while another command was recording"
    );
    // The other command, from the snapshot it read: `old.cc.o` stamped
    // with the new day, and no `app`.
    store(
        &catalog.join(STATE_FILE),
        &State {
            version: FORMAT_VERSION,
            day: DAY + 1,
            index: 2,
            collected: 1,
            plans: Vec::new(),
        },
    )
    .unwrap();
    store(
        &catalog.join(CATALOG_FILE),
        &Catalog {
            version: FORMAT_VERSION,
            entries: BTreeMap::from([(OLD_OBJ.to_owned(), 2)]),
        },
    )
    .unwrap();
    drop(other);
    recorded.recv().unwrap().unwrap().unwrap();

    // Both were used on the second active day, so neither ages past one
    // more.
    work_days(&fx, DAY + 1, 1, &[NEW_OBJ]);
    assert!(fx.collect(max_age(1), false).collected.is_empty());
    work_days(&fx, DAY + 2, 1, &[NEW_OBJ]);
    let mut collected = fx.collected(&fx.collect(max_age(1), false));
    collected.sort();
    assert_eq!(collected, [APP, OLD_OBJ]);
}

#[test]
fn concurrent_recordings_keep_every_product() {
    let fx = Fixture::new();
    let products: Vec<Utf8PathBuf> = (0..8)
        .map(|n| {
            let rel = format!("dev/packages/app/obj/app/src/{n}.cc.o");
            fx.file(&rel).write_str("o").unwrap();
            Utf8PathBuf::from_path_buf(fx.file(&rel).to_path_buf()).unwrap()
        })
        .collect();
    let build_dir = fx.build_dir();
    let start = std::sync::Barrier::new(products.len());
    std::thread::scope(|scope| {
        for product in &products {
            scope.spawn(|| {
                start.wait();
                record_use(&build_dir, [product.as_path()], DAY).unwrap()
            });
        }
    });
    work_days(&fx, DAY, 2, &[APP]);
    assert_eq!(
        fx.collect(max_age(1), false).collected.len(),
        products.len(),
        "every product is tracked"
    );
}

#[test]
fn a_time_limit_stops_removal_after_the_first_file_and_keeps_the_rest_tracked() {
    let fx = Fixture::new();
    let gone = "dev/packages/gone/libgone.a";
    fx.build(DAY, &[OLD_OBJ, gone]);
    let day = work_days(&fx, DAY, 2, &[APP]);
    let report = fx
        .try_collect(max_age(1), false, Some(Duration::ZERO))
        .unwrap();
    assert!(report.incomplete);
    assert_eq!(fx.collected(&report), [OLD_OBJ], "progress, however short");
    fx.file(gone).assert(predicate::path::exists());
    assert!(fx.build(day, &[APP]), "still due");
    assert_eq!(fx.collected(&fx.collect(max_age(1), false)), [gone]);
}

#[test]
fn dry_run_reports_what_a_collection_removes_and_changes_nothing() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ, "dev/packages/gone/libgone.a"]);
    let day = work_days(&fx, DAY, 2, &[APP]);
    let catalog = fx.build_dir().join(CATALOG_DIR);
    let before = snapshot(&catalog);

    let preview = fx.collect(max_age(1), true);
    assert_eq!(
        fx.collected(&preview),
        [
            "dev/packages/app/obj/app/src/old.cc.o",
            "dev/packages/gone/libgone.a"
        ]
    );
    assert_eq!(snapshot(&catalog), before, "a dry run writes nothing");
    fx.file(OLD_OBJ).assert(predicate::path::exists());
    assert!(fx.build(day, &[APP]), "still due");

    let report = fx.collect(max_age(1), false);
    assert_eq!(fx.collected(&report), fx.collected(&preview));
    assert_eq!(report.collected_bytes(), preview.collected_bytes());
}

#[test]
fn collection_removes_emptied_directories() {
    let fx = Fixture::new();
    let gone_obj = "dev/packages/gone/obj/gone/src/a.c.o";
    let gone_lib = "dev/packages/gone/libgone.a";
    fx.build(DAY, &[gone_obj, gone_lib, OLD_OBJ]);
    work_days(&fx, DAY, 2, &[NEW_OBJ]);
    fx.collect(max_age(1), false);
    fx.file("dev/packages/gone")
        .assert(predicate::path::missing());
    fx.file("dev/packages").assert(predicate::path::is_dir());
    // `src/` still holds the new object.
    fx.file("dev/packages/app/obj/app/src")
        .assert(predicate::path::is_dir());
}

#[test]
fn collection_removes_an_output_directory_that_held_a_scoped_packages_outputs() {
    let fx = Fixture::new();
    // `acme/lib`'s output directory lies inside `acme`'s.
    fx.build(
        DAY,
        &[
            "dev/packages/acme/obj/acme/src/a.c.o",
            "dev/packages/acme/lib/obj/lib/src/b.c.o",
            OLD_OBJ,
        ],
    );
    work_days(&fx, DAY, 2, &[NEW_OBJ]);
    fx.collect(max_age(1), false);
    fx.file("dev/packages/acme")
        .assert(predicate::path::missing());
}

#[test]
fn collection_removes_an_output_directory_whichever_spelling_held_it() {
    let fx = Fixture::new();
    // Profile `Bench` became `bench`; on a case-insensitive file system
    // both spellings name one directory.
    fx.build(
        DAY,
        &[
            "Bench/packages/acme/lib/obj/lib/src/b.c.o",
            "bench/packages/acme/obj/acme/src/a.c.o",
            OLD_OBJ,
        ],
    );
    work_days(&fx, DAY, 2, &[NEW_OBJ]);
    fx.collect(max_age(1), false);
    fx.file("bench/packages/acme")
        .assert(predicate::path::missing());
}

#[test]
fn size_budget_evicts_least_recently_used_but_spares_recent_products() {
    let fx = Fixture::new();
    let a = "dev/packages/a/liba.a";
    let b = "dev/packages/b/libb.a";
    let c = "dev/packages/c/libc.a";
    fx.build(DAY, &[a]);
    fx.build(DAY + 1, &[b]);
    fx.build(DAY + 2, &[c]);
    fx.build(DAY + 3, &[c]);
    // Each file holds its own path: 21 bytes.
    let policy = Policy {
        max_size: Some(30),
        ..max_age(30)
    };
    let report = fx.collect(policy, false);
    assert_eq!(fx.collected(&report), [a, b], "oldest first, until it fits");
    assert!(
        report
            .collected
            .iter()
            .all(|c| c.reason == Reason::SizeBudget)
    );
    assert_eq!(report.retained_bytes, Some(21));

    // Products used on the current or previous active day are kept even
    // when they alone exceed the budget.
    fx.build(DAY + 3, &[a, b]);
    fx.build(DAY + 4, &[c]);
    let report = fx.collect(
        Policy {
            max_size: Some(1),
            ..max_age(30)
        },
        false,
    );
    assert!(report.collected.is_empty());
    assert_eq!(report.retained_bytes, Some(63));
    fx.file(a).assert(predicate::path::exists());
}

#[test]
fn collection_never_follows_or_removes_links_and_directories() {
    let fx = Fixture::new();
    let outside = fx.tmp.child("outside");
    outside
        .child("obj/app/src/old.cc.o")
        .write_str("keep")
        .unwrap();
    fx.build(DAY, &[OLD_OBJ, APP]);
    work_days(&fx, DAY, 2, &["dev/packages/other/libother.a"]);

    // Swap the `obj` directory for a link to an unrelated tree, and
    // the executable for a directory.
    std::fs::remove_dir_all(fx.file("dev/packages/app/obj")).unwrap();
    if !symlink_dir(&outside.child("obj"), &fx.file("dev/packages/app/obj")) {
        return;
    }
    std::fs::remove_file(fx.file(APP)).unwrap();
    fx.file(APP).child("data").write_str("keep").unwrap();

    let report = fx.collect(max_age(1), false);
    assert!(report.collected.is_empty());
    outside
        .child("obj/app/src/old.cc.o")
        .assert(predicate::path::exists());
    fx.file(APP).child("data").assert(predicate::path::exists());
    // Neither path names a product any more, so neither stays tracked:
    // a file later put in the executable's place is not Cabin's.
    std::fs::remove_dir_all(fx.file(APP)).unwrap();
    fx.file(APP).write_str("mine").unwrap();
    assert!(fx.collect(max_age(1), true).collected.is_empty());
}

#[test]
fn a_build_directory_that_is_a_link_is_collected() {
    let fx = Fixture::new();
    let disk = fx.tmp.child("disk");
    disk.create_dir_all().unwrap();
    std::fs::remove_dir(fx.build_dir()).unwrap();
    if !symlink_dir(&disk, &fx.build_dir()) {
        return;
    }
    fx.build(DAY, &[OLD_OBJ]);
    work_days(&fx, DAY, 2, &[APP]);
    assert_eq!(fx.collected(&fx.collect(max_age(1), false)), [OLD_OBJ]);
    disk.child(OLD_OBJ).assert(predicate::path::missing());
    disk.child(APP).assert(predicate::path::exists());
}

#[test]
fn a_link_in_place_of_a_catalog_file_redirects_nothing() {
    for file in [STATE_FILE, CATALOG_FILE] {
        let fx = Fixture::new();
        let outside = fx.tmp.child("outside.json");
        outside.write_str("keep").unwrap();
        let catalog = fx.build_dir().join(CATALOG_DIR);
        std::fs::create_dir_all(&catalog).unwrap();
        if !symlink_file(&outside, &catalog.join(file)) {
            return;
        }
        fx.build(DAY, &[OLD_OBJ]);
        outside.assert("keep");
    }
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> bool {
    // Creating a symlink needs Developer Mode or elevation.
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[test]
fn a_malformed_catalog_authorizes_nothing() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    work_days(&fx, DAY, 2, &[APP]);
    let path = fx.build_dir().join(CATALOG_DIR).join(CATALOG_FILE);
    let victim = fx.tmp.child("victim");
    victim.write_str("keep").unwrap();
    for body in [
        format!(r#"{{"version":{FORMAT_VERSION},"entries":{{"../victim":1}}}}"#),
        format!(
            r#"{{"version":{FORMAT_VERSION},"entries":{{{:?}:1}}}}"#,
            victim.path().to_str().unwrap()
        ),
        format!(r#"{{"version":{FORMAT_VERSION},"entries":{{}},"extra":1}}"#),
        "not json".to_owned(),
    ] {
        std::fs::write(&path, &body).unwrap();
        let report = fx.collect(max_age(1), false);
        assert_eq!(report.unreadable, 1, "{body}");
        assert!(report.collected.is_empty(), "{body}");
        victim.assert(predicate::path::exists());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            body,
            "kept for inspection"
        );
    }
    fx.file(OLD_OBJ).assert(predicate::path::exists());
}

#[test]
fn a_newer_catalog_is_neither_recorded_into_nor_collected() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    let state = fx.build_dir().join(CATALOG_DIR).join(STATE_FILE);
    let newer = format!(r#"{{"version":{},"anything":true}}"#, FORMAT_VERSION + 1);
    std::fs::write(&state, &newer).unwrap();
    let product = Utf8PathBuf::from_path_buf(fx.file(APP).to_path_buf()).unwrap();
    assert!(!record_use(&fx.build_dir(), [product.as_path()], DAY + 9).unwrap());
    assert_eq!(std::fs::read_to_string(&state).unwrap(), newer);
    let err = fx.try_collect(max_age(1), false, None).unwrap_err();
    assert!(
        matches!(err, GcError::UnsupportedVersion { found, .. } if found == FORMAT_VERSION + 1),
        "{err}"
    );
}

#[test]
fn a_lost_or_corrupt_clock_restarts_without_aging_anything_early() {
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    work_days(&fx, DAY, 5, &[APP]);
    let state = fx.build_dir().join(CATALOG_DIR).join(STATE_FILE);
    std::fs::write(&state, "{").unwrap();
    assert!(
        fx.collect(max_age(1), false).collected.is_empty(),
        "no clock, no ages"
    );
    // The clock restarts at 1, so no stamp ages by more than the active
    // days counted since: `app`, stamped 6, now looks newest of all.
    work_days(&fx, DAY + 5, 3, &[NEW_OBJ]);
    let report = fx.collect(max_age(1), false);
    assert_eq!(fx.collected(&report), [OLD_OBJ]);
    fx.file(APP).assert(predicate::path::exists());
}

#[test]
fn an_unsafe_build_directory_is_refused() {
    let fx = Fixture::new();
    let err = collect(&CollectRequest {
        build_dir: &fx.tmp.child("ws"),
        workspace_root: &fx.tmp.child("ws"),
        package_roots: &[],
        protected_source_paths: &[],
        policy: Policy::default(),
        dry_run: true,
        time_limit: None,
    })
    .unwrap_err();
    assert!(matches!(
        err,
        GcError::UnsafeBuildDir(CleanError::WorkspaceRootBuildDir(_))
    ));
}

#[cfg(unix)]
#[test]
fn a_product_that_cannot_be_removed_is_reported_and_stays_tracked() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    fx.build(DAY, &[OLD_OBJ]);
    work_days(&fx, DAY, 2, &[APP]);
    let dir = fx.file("dev/packages/app/obj/app/src");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let report = fx.collect(max_age(1), false);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if report.failed.is_empty() {
        // Running as root: permissions do not stop the removal.
        return;
    }
    assert!(report.collected.is_empty());
    fx.file(OLD_OBJ).assert(predicate::path::exists());
    assert_eq!(fx.collected(&fx.collect(max_age(1), false)), [OLD_OBJ]);
}

#[test]
fn plan_products_lists_every_product_and_depfile() {
    use cabin_driver::{ArchiveAction, CompileAction, CompileArguments, Dialect, LinkAction};
    let compile = |object: &str, mode: CompileMode| {
        BuildAction::Compile(CompileAction {
            standard: cabin_core::LanguageStandard::C(cabin_core::CStandard::C11),
            gnu_extensions: false,
            source: "/src/a.c".into(),
            object: object.into(),
            mode,
            depfile: Some(format!("{object}.d").into()),
            clang_depfile: false,
            compiler: "cc".into(),
            target: None,
            sysroot: None,
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: cabin_core::OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: vec![],
                system_include_dirs: vec![],
                defines: vec![],
                instruction_sets: vec![],
                provider_flags: vec![],
                extra_flags: vec![],
            },
            description: String::new(),
        })
    };
    let graph = BuildGraph {
        actions: vec![
            compile("/b/a.o", CompileMode::Object),
            compile(
                "/b/c.o",
                CompileMode::SyntaxOnly {
                    stamp: "/b/c.o.check".into(),
                },
            ),
            BuildAction::Archive(ArchiveAction {
                archiver: "ar".into(),
                windows_rsp_quoting: false,
                output: "/b/liba.a".into(),
                inputs: vec!["/b/a.o".into()],
                description: String::new(),
            }),
            BuildAction::Link(LinkAction {
                linker: "cc".into(),
                target: None,
                sysroot: None,
                output: "/b/app".into(),
                inputs: vec!["/b/liba.a".into()],
                arguments: vec![],
                link_libs: vec![],
                frameworks: vec![],
                description: String::new(),
            }),
        ],
        dialect: Dialect::GnuLike,
        default_outputs: vec![],
        planned_packages: BTreeSet::new(),
        build_context_packages: BTreeSet::new(),
        compile_commands: vec![],
        standard_violations: vec![],
        standard_compat_violations: vec![],
    };
    let products: Vec<&str> = plan_products(&graph).map(Utf8Path::as_str).collect();
    assert_eq!(
        products,
        [
            "/b/a.o",
            "/b/a.o.d",
            "/b/c.o.check",
            "/b/c.o.d",
            "/b/liba.a",
            "/b/app"
        ]
    );
}

#[test]
fn only_one_process_holds_the_lock_exclusively() {
    let fx = Fixture::new();
    let mut waited = false;
    let first = BuildDirLock::shared(&fx.build_dir(), || waited = true).unwrap();
    let second = BuildDirLock::shared(&fx.build_dir(), || waited = true).unwrap();
    assert!(!waited, "shared holders do not wait for each other");
    assert_eq!(first.try_exclusive(|| ()).unwrap(), None);
    drop(second);
    assert_eq!(first.try_exclusive(|| 7).unwrap(), Some(7));
    // Shared again: another shared holder gets in without waiting.
    let _third = BuildDirLock::shared(&fx.build_dir(), || waited = true).unwrap();
    assert!(!waited);
}

#[cfg(unix)]
#[test]
fn enotsup_means_a_file_system_without_file_locks() {
    let err = io::Error::from_raw_os_error(libc::ENOTSUP);
    assert!(lock::locking_unsupported(&err), "{err}");
}

fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}
