//! Garbage collection of Cabin's own build outputs.
//!
//! Three facts are kept apart:
//!
//! - **Ownership.**  A path can be collected only if a plan Cabin ran
//!   declared it as an action product ([`plan_products`]) and the run
//!   recorded it in the catalog under `<build-dir>/.cabin-gc/`.  Nothing
//!   is inferred from a directory, a file name, or a suffix, so files
//!   Cabin did not plan - and outputs built before the catalog existed -
//!   are never collected.
//! - **Liveness.**  Commands that use build outputs hold [`BuildDirLock`]
//!   shared from before they record their use until they stop using the
//!   outputs; collection holds it exclusively.
//! - **Age.**  Use is counted in *active days*: UTC days on which some
//!   command recorded a use of this build directory.  An idle period
//!   ages nothing, and a clock jump in either direction moves the count
//!   by at most one day (a clock that keeps crossing midnight back and
//!   forth, as when machines whose clocks disagree share the directory,
//!   counts every forward crossing).  Products recorded as used on the
//!   current or the previous active day are never collected, whatever
//!   the policy, which also covers children (Ninja, a compiler, a test)
//!   that outlive a killed `cabin` for at least a day.
//!
//! Whether existing bytes are fresh enough to reuse stays Ninja's call:
//! collection only removes files (and directories left empty), and
//! Ninja rebuilds a missing output.
//!
//! The catalog is one small file per output directory (normally the
//! package directory [`crate::layout`] puts a product in; see
//! `split` for the one ambiguous layout), so recording a use
//! reads only the shards of the directories the plan writes, rewriting
//! one only to raise a stamp to the current active day.  Catalog files
//! are replaced by rename but not synced (see `store`); one that cannot
//! be read, or was written by a newer format, authorizes no removal.
//! Commands record one at a time under the catalog's own lock, since
//! two rewriting one shard from the same snapshot would lose one's
//! update.  A plan already
//! recorded on the current active day is remembered by digest and not
//! recorded again: collection never lowers a stamp of the current day,
//! and replacing an unreadable shard, which drops its stamps, forgets
//! every remembered plan, so a remembered plan's shards still hold
//! everything it would write.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use camino::Utf8Path;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use cabin_driver::{BuildAction, CompileMode};

use crate::clean::CleanError;
use crate::graph::BuildGraph;

mod lock;
#[cfg(test)]
mod tests;

pub use lock::{BuildDirLock, LOCK_FILENAME};

/// Directory under the build directory that holds the catalog.
pub const CATALOG_DIR: &str = ".cabin-gc";
const STATE_FILE: &str = "state.json";
const RECORDED_FILE: &str = "recorded.json";
const SHARDS_DIR: &str = "dirs";
/// Plans [`Recorded`] remembers per active day; recording another
/// forgets the oldest.
const RECORDED_PLANS: usize = 16;
const FORMAT_VERSION: u32 = 1;
/// Products used this many active days ago or more recently are never
/// collected.
const PROTECTED_AGE: u64 = 1;

/// Default [`Policy::max_age`], in active days.
pub const DEFAULT_MAX_AGE: NonZeroU32 = NonZeroU32::new(30).unwrap();

/// What a collection removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Remove products last used more than this many active days ago.
    pub max_age: NonZeroU32,
    /// After the age rule, remove the least recently used products
    /// until the tracked products' logical size fits, sparing those
    /// used on the current or previous active day.
    pub max_size: Option<u64>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_age: DEFAULT_MAX_AGE,
            max_size: None,
        }
    }
}

/// Errors from recording use or collecting.
#[derive(Debug, Error)]
pub enum GcError {
    #[error(transparent)]
    UnsafeBuildDir(#[from] CleanError),

    #[error("refusing to use {}: it is not a directory", .0.display())]
    UnsafeCatalog(PathBuf),

    #[error(
        "{} was written by a newer Cabin (format {found}); collect with that version, or run `cabin clean`",
        path.display()
    )]
    UnsupportedVersion { path: PathBuf, found: u32 },

    #[error("failed to access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> GcError + '_ {
    move |source| GcError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The active-day clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    /// UTC day (days since the Unix epoch) of the latest recorded use.
    day: u64,
    /// Active days counted so far; products record the index they were
    /// last used at.
    index: u64,
    /// Index at which the latest complete collection ran.
    collected: u64,
}

impl State {
    fn new(today: u64) -> Self {
        Self {
            version: FORMAT_VERSION,
            day: today,
            index: 1,
            collected: 1,
        }
    }

    /// Count `today` as an active day.  A clock that went backwards
    /// only moves `day`, so neither a jump forward nor its correction
    /// adds more than one active day.
    fn advance(&mut self, today: u64) -> bool {
        if today == self.day {
            return false;
        }
        if today > self.day {
            self.index += 1;
        }
        self.day = today;
        true
    }

    fn age(&self, index: u64) -> u64 {
        self.index.saturating_sub(index)
    }
}

/// The products one output directory's plans declared, with the active
/// day each was last used.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shard {
    version: u32,
    /// Output directory, relative to the build directory, `/`-separated;
    /// empty for the build directory itself.
    dir: String,
    /// Product paths relative to `dir` -> active-day index of last use.
    entries: BTreeMap<String, u64>,
}

impl Shard {
    fn is_valid(&self) -> bool {
        (self.dir.is_empty() || is_valid_relative(&self.dir))
            && self.entries.keys().all(|rel| is_valid_relative(rel))
    }
}

/// The plans recorded on active day `index`, whose products then held a
/// stamp of at least `index` (higher after a lost clock), so recording
/// one of them again on that day changes nothing.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    version: u32,
    index: u64,
    /// [`plan_digest`]s, oldest first.
    plans: Vec<String>,
}

#[derive(Deserialize)]
struct Header {
    version: u32,
}

enum Loaded<T> {
    Missing,
    Unreadable,
    Newer(u32),
    Ok(T),
}

fn load<T: DeserializeOwned>(path: &Path) -> Result<Loaded<T>, GcError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(err) => return Err(io_err(path)(err)),
    };
    let Ok(header) = serde_json::from_slice::<Header>(&bytes) else {
        return Ok(Loaded::Unreadable);
    };
    if header.version > FORMAT_VERSION {
        return Ok(Loaded::Newer(header.version));
    }
    Ok(match serde_json::from_slice(&bytes) {
        Ok(value) if header.version == FORMAT_VERSION => Loaded::Ok(value),
        _ => Loaded::Unreadable,
    })
}

/// Staged in a file `create_new` makes, which never opens through a
/// symlink, and renamed over `path`, which replaces whatever is there
/// rather than writing through it.  Not synced: the catalog guides the
/// collection policy, never a build.  An OS crash can lose or reorder
/// these writes, so a product can look older than it is (a lost stamp,
/// or a remembered plan whose stamps were lost) until its plan records
/// again on a later active day, which costs at most a rebuild; a file
/// the crash leaves unparsable authorizes no removal.
fn store<T: Serialize>(path: &Path, value: &T) -> Result<(), GcError> {
    use std::io::Write as _;
    let bytes = serde_json::to_vec(value).expect("catalog records serialize");
    let staging = path.with_extension(format!("{}.tmp", std::process::id()));
    // Left by a process that died with this id.
    let _ = fs::remove_file(&staging);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(io_err(&staging))?;
    fs::rename(&staging, path).map_err(io_err(path))
}

/// Today as a UTC day number, the clock [`record_use`] takes.
#[must_use]
pub fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400)
}

/// Every file `graph`'s actions write with Cabin's own flags: each
/// action's product and depfile.  This is the complete ownership
/// evidence collection accepts, so an action kind that writes another
/// file must list it here.  Files user flags add, such as the `.pdb`
/// and `.ilk` an MSVC link writes for `/DEBUG`, are never collected.
pub fn plan_products(graph: &BuildGraph) -> impl Iterator<Item = &Utf8Path> {
    graph.actions.iter().flat_map(|action| {
        let (product, depfile) = match action {
            BuildAction::Compile(compile) => (
                match &compile.mode {
                    CompileMode::Object => &compile.object,
                    CompileMode::SyntaxOnly { stamp } => stamp,
                },
                compile.depfile.as_ref(),
            ),
            BuildAction::Assemble(assemble) => (&assemble.object, assemble.depfile.as_ref()),
            BuildAction::Archive(archive) => (&archive.output, None),
            BuildAction::Link(link) => (&link.output, None),
        };
        std::iter::once(product.as_path()).chain(depfile.map(camino::Utf8PathBuf::as_path))
    })
}

/// Record that the products under `build_dir` were used today: count
/// `today` as an active day and stamp each product with it.  The caller
/// holds [`BuildDirLock`] shared, from before this call until it stops
/// using the products.  Returns whether a collection is due: the first
/// use on a new active day makes one due, until a complete collection
/// runs.  A newer Cabin's catalog is left alone and never makes one due.
///
/// # Errors
/// [`GcError::Io`] when the catalog cannot be read or written, and
/// [`GcError::UnsafeCatalog`] when it is not a real directory.  Callers
/// fail the command: a use that is not recorded could let a later
/// collection treat these products as unused.
#[tracing::instrument(level = "debug", skip_all, fields(products = tracing::field::Empty, shards_written = tracing::field::Empty))]
pub fn record_use<'a>(
    build_dir: &Path,
    products: impl IntoIterator<Item = &'a Utf8Path>,
    today: u64,
) -> Result<bool, GcError> {
    let catalog = build_dir.join(CATALOG_DIR);
    let shards_dir = catalog.join(SHARDS_DIR);
    fs::create_dir_all(&shards_dir).map_err(io_err(&shards_dir))?;
    // `create_dir_all` accepts a symlinked directory; writing through
    // one would let it redirect the catalog.
    for dir in [&catalog, &shards_dir] {
        if !is_real_dir(dir)? {
            return Err(GcError::UnsafeCatalog(dir.clone()));
        }
    }

    let products: Vec<&Utf8Path> = products.into_iter().collect();
    let plan = plan_digest(&products);
    let state_path = catalog.join(STATE_FILE);
    let recorded_path = catalog.join(RECORDED_FILE);
    if let Loaded::Ok(state) = load::<State>(&state_path)?
        && state.day == today
        && let Loaded::Ok(recorded) = load::<Recorded>(&recorded_path)?
        && recorded.index == state.index
        && recorded.plans.contains(&plan)
    {
        return Ok(state.collected < state.index);
    }

    // The build directory lock's mechanism, on the catalog: held only
    // while recording, so commands sharing the build directory wait on
    // each other no longer than that.  A whole `cabin clean` removes its
    // file with the catalog, which no command holds or waits on then:
    // Cabin records only under the build directory lock held shared,
    // and that clean holds it exclusively.
    let _recording = BuildDirLock::exclusive(&catalog, || {})?;
    let (mut state, mut state_changed) = match load::<State>(&state_path)? {
        Loaded::Ok(state) => (state, false),
        // A lost clock restarts at 1; products stamped under the old
        // one look newer than everything and are kept.
        Loaded::Missing | Loaded::Unreadable => (State::new(today), true),
        Loaded::Newer(_) => return Ok(false),
    };
    state_changed |= state.advance(today);
    if state_changed {
        store(&state_path, &state)?;
    }

    let relative: Vec<String> = products
        .iter()
        .filter_map(|product| relative_to(build_dir, product))
        .collect();
    let mut by_dir: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for rel in &relative {
        let (dir, rest) = split(rel);
        by_dir.entry(dir).or_default().insert(rest);
    }
    let mut written = 0;
    for (dir, rests) in by_dir {
        let path = shard_path(&shards_dir, dir);
        let mut shard = match load::<Shard>(&path)? {
            Loaded::Ok(shard) if shard.is_valid() => shard,
            Loaded::Newer(_) => continue,
            // Replacing an unreadable shard forgets what it listed: those
            // files are never collected, the safe direction.  Plans
            // recorded today may be among them, so they are forgotten
            // first, and record again even if this recording fails.
            loaded => {
                if !matches!(loaded, Loaded::Missing) {
                    match fs::remove_file(&recorded_path) {
                        Err(err) if err.kind() != io::ErrorKind::NotFound => {
                            return Err(io_err(&recorded_path)(err));
                        }
                        _ => {}
                    }
                }
                Shard {
                    version: FORMAT_VERSION,
                    dir: dir.to_owned(),
                    entries: BTreeMap::new(),
                }
            }
        };
        let mut changed = false;
        for rest in rests {
            let index = shard.entries.entry(rest.to_owned()).or_insert(0);
            if *index < state.index {
                *index = state.index;
                changed = true;
            }
        }
        if changed {
            store(&path, &shard)?;
            written += 1;
        }
    }
    let mut recorded = match load::<Recorded>(&recorded_path)? {
        Loaded::Ok(recorded) if recorded.index == state.index => recorded,
        _ => Recorded {
            version: FORMAT_VERSION,
            index: state.index,
            plans: Vec::new(),
        },
    };
    if !recorded.plans.contains(&plan) {
        if recorded.plans.len() == RECORDED_PLANS {
            recorded.plans.remove(0);
        }
        recorded.plans.push(plan);
        store(&recorded_path, &recorded)?;
    }
    let span = tracing::Span::current();
    span.record("products", relative.len());
    span.record("shards_written", written);
    Ok(state.collected < state.index)
}

/// Inputs to [`collect`].  The safety fields match
/// [`crate::clean::CleanRequest`]'s, and refuse the same build
/// directories except a link (see [`validate_build_dir`]).
#[derive(Debug, Clone)]
pub struct CollectRequest<'a> {
    /// Resolved absolute build directory (without a `--target` triple).
    pub build_dir: &'a Path,
    pub workspace_root: &'a Path,
    pub package_roots: &'a [PathBuf],
    pub protected_source_paths: &'a [PathBuf],
    pub policy: Policy,
    /// Report what would be removed without removing or recording
    /// anything.
    pub dry_run: bool,
    /// Stop removing files once removal has taken this long; the rest
    /// waits for a later collection.  Timed from the first removal, not
    /// the catalog scan, and checked between files, so every collection
    /// with something to remove makes progress.
    pub time_limit: Option<Duration>,
}

/// Why a product is collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Last used this many active days ago, more than the policy keeps.
    Unused { days: u64 },
    /// Among the least recently used when the tracked products exceeded
    /// the size budget.
    SizeBudget,
}

/// One collected product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    pub path: PathBuf,
    /// Logical size (the file's length), not allocated storage.
    pub bytes: u64,
    pub reason: Reason,
}

/// Outcome of [`collect`].
#[derive(Debug, Default)]
pub struct CollectReport {
    /// Products removed - for a dry run, those that would be - sorted by
    /// path.
    pub collected: Vec<Collected>,
    /// Products that could not be removed; the catalog keeps them for a
    /// later collection.
    pub failed: Vec<(PathBuf, io::Error)>,
    /// Catalog shards that could not be read; their products are kept.
    pub unreadable: usize,
    /// Logical size of the tracked products left, when a size budget
    /// was evaluated; above the budget only when products used on the
    /// current or previous active day alone exceed it.
    pub retained_bytes: Option<u64>,
    /// The time limit cut the collection short.
    pub incomplete: bool,
}

impl CollectReport {
    #[must_use]
    pub fn collected_bytes(&self) -> u64 {
        self.collected.iter().map(|c| c.bytes).sum()
    }
}

/// Refuse the build directories `cabin clean` refuses, except a link:
/// collection removes only files beneath the directory (and directories
/// left empty there), never the directory itself, so one linked to
/// another disk is fine.
///
/// # Errors
/// [`GcError::UnsafeBuildDir`] for a refused directory.
pub fn validate_build_dir(req: &CollectRequest<'_>) -> Result<(), GcError> {
    match crate::clean::validate_safe_build_dir(
        req.build_dir,
        req.workspace_root,
        req.package_roots,
        req.protected_source_paths,
    ) {
        Ok(()) | Err(CleanError::SymlinkBuildDir(_)) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Remove the products the catalog shows unused under `req.policy`.
/// The caller holds [`BuildDirLock`] exclusively, except for a dry run,
/// which only reads.  Every candidate is checked again just before it
/// is removed: only a regular file reached through real directories is
/// removed, and anything else drops out of the catalog untouched.
///
/// # Errors
/// [`GcError::UnsafeBuildDir`] when [`validate_build_dir`] refuses the
/// build directory, [`GcError::UnsupportedVersion`] when a
/// newer Cabin owns the catalog, [`GcError::UnsafeCatalog`] when the
/// catalog is not a real directory, and [`GcError::Io`] when the
/// catalog cannot be read or rewritten.  A product that cannot be
/// removed is reported in [`CollectReport::failed`] instead.
#[tracing::instrument(level = "info", skip_all, fields(tracked = tracing::field::Empty, collected = tracing::field::Empty, bytes = tracing::field::Empty, failed = tracing::field::Empty))]
pub fn collect(req: &CollectRequest<'_>) -> Result<CollectReport, GcError> {
    validate_build_dir(req)?;
    let mut report = CollectReport::default();
    let catalog = req.build_dir.join(CATALOG_DIR);
    match fs::symlink_metadata(&catalog) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(err) => return Err(io_err(&catalog)(err)),
        Ok(meta) if !meta.is_dir() => return Err(GcError::UnsafeCatalog(catalog)),
        Ok(_) => {}
    }
    let state_path = catalog.join(STATE_FILE);
    let mut state = match load::<State>(&state_path)? {
        Loaded::Ok(state) => state,
        // Without the clock nothing can be judged old.
        Loaded::Missing | Loaded::Unreadable => return Ok(report),
        Loaded::Newer(found) => {
            return Err(GcError::UnsupportedVersion {
                path: state_path,
                found,
            });
        }
    };

    let shards_dir = catalog.join(SHARDS_DIR);
    if shards_dir.exists() && !is_real_dir(&shards_dir)? {
        return Err(GcError::UnsafeCatalog(shards_dir));
    }
    let shards = load_shards(&shards_dir, &mut report)?;
    // Spellings that differ only in case can name one file (the macOS
    // and Windows defaults), so a path counts as used when any spelling
    // of it was: after `Util.cc` becomes `util.cc`, removing the old
    // spelling's object would remove the new one.
    let mut by_folded: BTreeMap<String, u64> = BTreeMap::new();
    let mut newest: BTreeMap<String, u64> = BTreeMap::new();
    for (_, shard) in &shards {
        for (rest, &index) in &shard.entries {
            let path = join(&shard.dir, rest);
            let used = by_folded.entry(path.to_lowercase()).or_insert(index);
            *used = (*used).max(index);
            newest.insert(path, 0);
        }
    }
    for (path, index) in &mut newest {
        *index = by_folded[&path.to_lowercase()];
    }

    let max_age = u64::from(req.policy.max_age.get());
    let mut unused: Vec<(&str, u64)> = newest
        .iter()
        .filter(|&(_, &index)| state.age(index) > max_age)
        .map(|(path, &index)| (path.as_str(), index))
        .collect();
    unused.sort_by_key(|&(path, index)| (index, path));
    let mut evict: Vec<(&str, Reason)> = unused
        .iter()
        .map(|&(path, index)| {
            (
                path,
                Reason::Unused {
                    days: state.age(index),
                },
            )
        })
        .collect();
    if let Some(budget) = req.policy.max_size {
        let mut sized: BTreeSet<String> = BTreeSet::new();
        let mut kept: Vec<(u64, &str, u64)> = newest
            .iter()
            .filter(|&(_, &index)| state.age(index) <= max_age)
            .filter_map(|(path, &index)| {
                // Sizing only: removal checks the path again.
                let meta = fs::symlink_metadata(req.build_dir.join(path)).ok()?;
                // Size a file once, whatever spellings name it.
                (meta.is_file() && sized.insert(path.to_lowercase())).then_some((
                    index,
                    path.as_str(),
                    meta.len(),
                ))
            })
            .collect();
        kept.sort_unstable();
        let mut total: u64 = kept.iter().map(|&(_, _, bytes)| bytes).sum();
        for (index, path, bytes) in kept {
            if total <= budget || state.age(index) <= PROTECTED_AGE {
                break;
            }
            evict.push((path, Reason::SizeBudget));
            total -= bytes;
        }
        report.retained_bytes = Some(total);
    }

    if req.dry_run {
        for (path, reason) in evict {
            if let Ok(Some((path, bytes))) = regular_file(req.build_dir, path) {
                report.collected.push(Collected {
                    path,
                    bytes,
                    reason,
                });
            }
        }
    } else {
        let mut dropped: BTreeSet<&str> = BTreeSet::new();
        // Every directory a removal may have emptied, up to and including
        // its output directory, each tried once after all removals,
        // however many files were removed beneath it.
        let mut emptied: BTreeSet<PathBuf> = BTreeSet::new();
        let deadline = req.time_limit.map(|limit| Instant::now() + limit);
        for (n, (rel, reason)) in evict.into_iter().enumerate() {
            if n > 0 && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                report.incomplete = true;
                break;
            }
            match remove_product(req.build_dir, rel) {
                Ok(Some((path, bytes))) => {
                    let stop = req.build_dir.join(split(rel).0);
                    for dir in path.ancestors().skip(1) {
                        if dir == req.build_dir || !dir.starts_with(&stop) {
                            break;
                        }
                        if !emptied.contains(dir) {
                            emptied.insert(dir.to_path_buf());
                        }
                    }
                    report.collected.push(Collected {
                        path,
                        bytes,
                        reason,
                    });
                    dropped.insert(rel);
                }
                Ok(None) => {
                    dropped.insert(rel);
                }
                Err(err) => report.failed.push((req.build_dir.join(rel), err)),
            }
        }
        // Deepest first, so each directory is tried after the ones it held,
        // whichever spelling a case-insensitive file system saw them under.
        // `remove_dir` never removes a directory that still holds anything.
        let mut emptied = Vec::from_iter(emptied);
        emptied.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in &emptied {
            let _ = fs::remove_dir(dir);
        }
        for (path, mut shard) in shards {
            let before = shard.entries.len();
            shard
                .entries
                .retain(|rest, _| !dropped.contains(join(&shard.dir, rest).as_str()));
            if shard.entries.len() == before {
                continue;
            }
            if shard.entries.is_empty() {
                match fs::remove_file(&path) {
                    Err(err) if err.kind() != io::ErrorKind::NotFound => {
                        return Err(io_err(&path)(err));
                    }
                    _ => {}
                }
            } else {
                store(&path, &shard)?;
            }
        }
        if !report.incomplete && state.collected != state.index {
            state.collected = state.index;
            store(&state_path, &state)?;
        }
    }
    report.collected.sort_by(|a, b| a.path.cmp(&b.path));
    let span = tracing::Span::current();
    span.record("tracked", newest.len());
    span.record("collected", report.collected.len());
    span.record("bytes", report.collected_bytes());
    span.record("failed", report.failed.len());
    Ok(report)
}

fn load_shards(dir: &Path, report: &mut CollectReport) -> Result<Vec<(PathBuf, Shard)>, GcError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_err(dir)(err)),
    };
    let mut shards = Vec::new();
    for entry in entries {
        let path = entry.map_err(io_err(dir))?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match load::<Shard>(&path)? {
            Loaded::Ok(shard) if shard.is_valid() => shards.push((path, shard)),
            Loaded::Missing => {}
            _ => report.unreadable += 1,
        }
    }
    Ok(shards)
}

/// Remove the product at `rel`: `Some` with its path and size when a
/// regular file was removed, `None` when nothing Cabin wrote is there
/// any more.
fn remove_product(build_dir: &Path, rel: &str) -> io::Result<Option<(PathBuf, u64)>> {
    let Some((path, bytes)) = regular_file(build_dir, rel)? else {
        return Ok(None);
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(Some((path, bytes))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The regular file at `rel` under `build_dir` and its length, when
/// every directory on the way is a real directory.  A symlink or
/// junction anywhere on the path means it no longer names a file Cabin
/// wrote, so the answer is `None` and nothing is followed:
/// `symlink_metadata` reports a link as neither a file nor a directory.
fn regular_file(build_dir: &Path, rel: &str) -> io::Result<Option<(PathBuf, u64)>> {
    let mut path = build_dir.to_path_buf();
    let mut components = rel.split('/').peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if components.peek().is_none() {
            return Ok(meta.is_file().then_some((path, meta.len())));
        }
        if !meta.is_dir() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn is_real_dir(path: &Path) -> Result<bool, GcError> {
    Ok(fs::symlink_metadata(path).map_err(io_err(path))?.is_dir())
}

/// `product` relative to `build_dir`, `/`-separated, when it lies under
/// it and every component is valid UTF-8 without separators.
fn relative_to(build_dir: &Path, product: &Utf8Path) -> Option<String> {
    let rel = product.as_std_path().strip_prefix(build_dir).ok()?;
    let mut out = String::new();
    for component in rel.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(name.to_str()?);
    }
    is_valid_relative(&out).then_some(out)
}

/// Whether every `/`-separated name in `rel` is one plain path
/// component on this platform.  Windows parses `C:x` as a drive prefix,
/// and pushing one replaces the whole path.
fn is_valid_relative(rel: &str) -> bool {
    rel.split('/').all(|name| {
        let mut components = Path::new(name).components();
        matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
    })
}

/// Split a product path into its output directory, the package
/// directory [`crate::layout`] puts it in, and the rest: the cut is
/// before the package's `obj` directory, else at the product's parent.
/// A profile, package, scope, target, or source directory may be named
/// `obj`, so the cut is at the first `obj` past the package name's first
/// component with a target directory and a file below it.  That reads
/// `packages/s/obj/obj/t/f.o` as package `s`'s, though the scoped
/// package `s/obj` would write the same path.
fn split(rel: &str) -> (&str, &str) {
    let names: Vec<&str> = rel.split('/').collect();
    // `packages` follows the profile, and `build-machine` after it in
    // the build context.
    let mut packages = if names[0] == crate::layout::TARGETS_DIR {
        3
    } else {
        1
    };
    if names.get(packages) == Some(&crate::layout::BUILD_CONTEXT_DIR) {
        packages += 1;
    }
    let cut = (packages + 2..names.len().saturating_sub(2))
        .find(|&i| names[i] == "obj")
        .unwrap_or(names.len() - 1);
    let offset: usize = names[..cut].iter().map(|name| name.len() + 1).sum();
    (rel[..offset].trim_end_matches('/'), &rel[offset..])
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_owned()
    } else {
        format!("{dir}/{rest}")
    }
}

/// Names a plan in [`Recorded`]: a digest of its products in plan order.
fn plan_digest(products: &[&Utf8Path]) -> String {
    let mut bytes = Vec::new();
    for product in products {
        bytes.extend_from_slice(product.as_str().as_bytes());
        bytes.push(0);
    }
    cabin_core::hash::hash_reader(bytes.as_slice()).expect("reading a slice cannot fail")
}

fn shard_path(shards_dir: &Path, dir: &str) -> PathBuf {
    let digest =
        cabin_core::hash::hash_reader(dir.as_bytes()).expect("reading a slice cannot fail");
    shards_dir.join(format!("{}.json", &digest[..32]))
}
