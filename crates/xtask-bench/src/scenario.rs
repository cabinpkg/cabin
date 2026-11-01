//! The scenario catalog: what each benchmark runs, the state it
//! starts from, and how its result is checked before it is timed.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use cabin_build::gc;
use camino::Utf8PathBuf;
use xtask_bench::fixture::{self, Lang, ProjectShape};

pub const PROJECT: [ProjectShape; 2] = [
    ProjectShape {
        lang: Lang::C,
        sources: 24,
        tests: 12,
    },
    ProjectShape {
        lang: Lang::Cxx,
        sources: 24,
        tests: 12,
    },
];
pub const WORKSPACE_MEMBERS: usize = 40;
/// Where [`Mutation::Stale`] plants its outputs.
const STALE_DIR: &str = "build/stale";

/// The input directory a scenario starts from.  Each run gets its own
/// copy, so one binary's build state never leaks into another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fixture {
    Empty,
    Project(Lang),
    Workspace,
    Consumer(&'static str),
    PortsConsumer,
}

impl Fixture {
    pub fn dir_name(self) -> String {
        match self {
            Fixture::Empty => "empty".to_owned(),
            Fixture::Project(lang) => format!("project-{}", lang.suffix()),
            Fixture::Workspace => "workspace".to_owned(),
            Fixture::Consumer(name) => format!("consumer-{name}"),
            Fixture::PortsConsumer => "consumer-ports".to_owned(),
        }
    }

    pub fn needs_registry(self) -> bool {
        matches!(self, Fixture::Consumer(_))
    }
}

/// Resets the state a timed run starts from; runs before every
/// iteration, outside the measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    /// Advance a file's modification time to now.
    Touch(String),
    /// Delete a file or directory if it exists.
    Remove(String),
    /// Plant this many outputs under [`STALE_DIR`], recorded as unused
    /// for just over the default max-age, and leave today a new active
    /// day for the build directory.
    Stale(usize),
}

impl Mutation {
    pub fn apply(&self, dir: &Path) -> io::Result<()> {
        match self {
            Mutation::Touch(path) => fs::File::options()
                .write(true)
                .open(dir.join(path))?
                .set_modified(SystemTime::now()),
            Mutation::Remove(path) => {
                let path = dir.join(path);
                let result = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                match result {
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            }
            Mutation::Stale(count) => {
                let stale = dir.join(STALE_DIR);
                fs::create_dir_all(&stale)?;
                let mut outputs = Vec::with_capacity(*count);
                for i in 0..*count {
                    let path = stale.join(format!("{i}.o"));
                    fs::write(&path, b"")?;
                    outputs.push(Utf8PathBuf::try_from(path).map_err(io::Error::other)?);
                }
                record_stale(&dir.join("build"), &outputs).map_err(io::Error::other)?;
                // On macOS, once any directory is removed or renamed, the
                // next open or removal of each file costs about 0.14
                // million more kernel instructions.  Outputs a real
                // collection removes are past that point; without this,
                // a run is cheaper whenever no process on the machine
                // removed or renamed a directory since they were planted.
                if cfg!(target_os = "macos") {
                    let reset = dir.join("build/.bench-reset");
                    fs::create_dir(&reset)?;
                    fs::remove_dir(&reset)?;
                }
                Ok(())
            }
        }
    }

    pub fn args(&self) -> [String; 2] {
        match self {
            Mutation::Touch(path) => ["touch".to_owned(), path.clone()],
            Mutation::Remove(path) => ["remove".to_owned(), path.clone()],
            Mutation::Stale(count) => ["stale".to_owned(), count.to_string()],
        }
    }
}

/// Record `outputs` as used just over the default max-age active days
/// before yesterday, with every day since an active one.
fn record_stale(build: &Path, outputs: &[Utf8PathBuf]) -> Result<(), gc::GcError> {
    let days = u64::from(gc::DEFAULT_MAX_AGE.get()) + 1;
    let yesterday = gc::today() - 1;
    gc::record_use(
        build,
        outputs.iter().map(Utf8PathBuf::as_path),
        yesterday - days,
    )?;
    for day in yesterday - days + 1..=yesterday {
        gc::record_use(build, std::iter::empty(), day)?;
    }
    Ok(())
}

/// What a correct run of the scenario does, checked once on an
/// untimed run so a fast-but-wrong result is never reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Exit status 0.
    Success,
    /// Exit status 0 and Ninja ran no action.
    NoActions,
    /// Exit status 0 and Ninja compiled exactly this many objects.
    Compiles(usize),
    /// Exit status 0 and Ninja compiled at least one object.
    Builds,
    /// Exit status 0 and `cabin.lock` lists exactly this many packages.
    Locks(usize),
    /// Exit status 0 and this directory holds exactly this many entries.
    Entries(String, usize),
    /// Exit status 0, Ninja ran no action, and the `JUnit` report at
    /// this path lists exactly this many test cases.
    TestCases(String, usize),
    /// Exit status 0, `removed` no longer exists, and `kept` still does.
    Collects { removed: String, kept: String },
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    /// Part of the quick subset.
    pub smoke: bool,
    /// Needs the real foundation ports, staged from the network on
    /// first use; selected only on request.
    pub ports: bool,
    pub fixture: Fixture,
    /// Cabin invocations run once before timing (a first build).
    pub setup: Vec<Vec<String>>,
    pub prepare: Option<Mutation>,
    /// Cabin arguments; `{registry}`, `{http}`, `{http-rtt}` (the
    /// same server, each response after a round trip), `{ports}`, and
    /// `{traces}` are substituted per run.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub expect: Expect,
}

impl Scenario {
    fn new(name: impl Into<String>, fixture: Fixture, args: &[&str]) -> Self {
        Self {
            name: name.into(),
            smoke: false,
            ports: false,
            fixture,
            setup: Vec::new(),
            prepare: None,
            args: strings(args),
            env: Vec::new(),
            expect: Expect::Success,
        }
    }

    fn smoke(mut self) -> Self {
        self.smoke = true;
        self
    }

    fn setup(mut self, args: &[&str]) -> Self {
        self.setup.push(strings(args));
        self
    }

    fn prepare(mut self, mutation: Mutation) -> Self {
        self.prepare = Some(mutation);
        self
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_owned(), value.to_owned()));
        self
    }

    fn expect(mut self, expect: Expect) -> Self {
        self.expect = expect;
        self
    }

    pub fn uses_http(&self) -> bool {
        self.args.iter().any(|arg| arg.starts_with("{http"))
    }

    /// File-system-safe form of the name.
    pub fn slug(&self) -> String {
        self.name.replace('/', "--")
    }
}

/// One verified archive per dependency in the `--cache-dir cache`.
fn archives(count: usize) -> Expect {
    Expect::Entries("cache/archives/sha256".to_owned(), count)
}

/// The planted outputs are gone and `kept`, which a collection keeps
/// and a whole `cabin clean` would not, survives.
fn collects(kept: &str) -> Expect {
    Expect::Collects {
        removed: STALE_DIR.to_owned(),
        kept: kept.to_owned(),
    }
}

fn unlock() -> Mutation {
    Mutation::Remove("cabin.lock".to_owned())
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|&arg| arg.to_owned()).collect()
}

const BUILD: &[&str] = &["build", "--no-compiler-wrapper"];
const TEST: &[&str] = &["test", "--no-compiler-wrapper", "--junit", "junit.xml"];
const RESOLVE: &[&str] = &["resolve", "--index-path", "{registry}"];
const RESOLVE_HTTP: &[&str] = &["resolve", "--index-url", "{http}"];
const RESOLVE_HTTP_RTT: &[&str] = &["resolve", "--index-url", "{http-rtt}"];
const FETCH: &[&str] = &[
    "fetch",
    "--index-path",
    "{registry}",
    "--cache-dir",
    "cache",
];

pub fn catalog() -> Vec<Scenario> {
    let mut all = vec![Scenario::new("startup/version", Fixture::Empty, &["--version"]).smoke()];
    all.extend(PROJECT.into_iter().flat_map(project_scenarios));
    all.extend(shared_scenarios());
    all.extend(ports_scenarios());
    all
}

/// The single-package scenarios, once per language.
fn project_scenarios(shape: ProjectShape) -> [Scenario; 8] {
    let lang = shape.lang;
    let s = lang.suffix();
    let project = Fixture::Project(lang);
    let cxx = lang == Lang::Cxx;
    [
        Scenario::new(format!("inspect/metadata-{s}"), project, &["metadata"]).smoke(),
        Scenario::new(format!("build/clean-{s}"), project, BUILD)
            .prepare(Mutation::Remove("build".to_owned()))
            .expect(Expect::Compiles(shape.sources + 1)),
        Scenario::new(format!("build/noop-{s}"), project, BUILD)
            .setup(BUILD)
            .expect(Expect::NoActions)
            .smoke(),
        Scenario {
            smoke: cxx,
            ..Scenario::new(format!("build/touch-source-{s}"), project, BUILD)
                .setup(BUILD)
                .prepare(Mutation::Touch(shape.leaf_source()))
                .expect(Expect::Compiles(1))
        },
        Scenario::new(format!("build/touch-header-{s}"), project, BUILD)
            .setup(BUILD)
            .prepare(Mutation::Touch(shape.common_header()))
            .expect(Expect::Compiles(shape.common_includers())),
        Scenario::new(format!("test/run-{s}"), project, TEST)
            .setup(TEST)
            .prepare(Mutation::Remove("junit.xml".to_owned()))
            .expect(Expect::TestCases("junit.xml".to_owned(), shape.tests)),
        Scenario::new(
            format!("package/package-{s}"),
            project,
            &["package", "--output-dir", "dist"],
        )
        .prepare(Mutation::Remove("dist".to_owned()))
        // The archive and its metadata.
        .expect(Expect::Entries("dist".to_owned(), 2)),
        // The first build of an active day: every record of the plan is
        // rewritten, and a collection runs.
        Scenario::new(format!("gc/new-day-{s}"), project, BUILD)
            .setup(BUILD)
            .prepare(Mutation::Stale(1))
            .expect(collects("build/dev/packages/bench/obj")),
    ]
}

/// The workspace, registry, and capture-overhead scenarios.
fn shared_scenarios() -> Vec<Scenario> {
    vec![
        Scenario::new("workspace/metadata", Fixture::Workspace, &["metadata"]),
        Scenario::new("workspace/tree", Fixture::Workspace, &["tree"]),
        Scenario::new("workspace/build-noop", Fixture::Workspace, BUILD)
            .setup(BUILD)
            .expect(Expect::NoActions),
        Scenario::new(
            "gc/clean-10k",
            Fixture::Project(Lang::Cxx),
            &["clean", "--gc"],
        )
        // `cabin clean` refuses a build directory with no owner record.
        .setup(BUILD)
        .prepare(Mutation::Stale(10_000))
        .expect(collects(&format!("build/{}", gc::CATALOG_DIR))),
        // `cabin resolve` writes the lockfile, and a locked re-resolve
        // skips the search these scenarios exist to time.
        Scenario::new("resolve/wide", Fixture::Consumer("wide"), RESOLVE)
            .prepare(unlock())
            .expect(Expect::Locks(fixture::WIDE.packages))
            .smoke(),
        Scenario::new("resolve/deep", Fixture::Consumer("deep"), RESOLVE)
            .prepare(unlock())
            .expect(Expect::Locks(fixture::DEEP.packages)),
        Scenario::new(
            "resolve/update-wide",
            Fixture::Consumer("wide"),
            &["update", "--index-path", "{registry}"],
        )
        .prepare(unlock())
        .expect(Expect::Locks(fixture::WIDE.packages)),
        Scenario::new("index/http-wide", Fixture::Consumer("wide"), RESOLVE_HTTP)
            .prepare(unlock())
            .expect(Expect::Locks(fixture::WIDE.packages)),
        Scenario::new("index/http-lazy", Fixture::Consumer("lazy"), RESOLVE_HTTP)
            .prepare(unlock())
            .expect(Expect::Locks(fixture::LAZY.packages)),
        Scenario::new(
            "index/rtt-wide",
            Fixture::Consumer("wide"),
            RESOLVE_HTTP_RTT,
        )
        .prepare(unlock())
        .expect(Expect::Locks(fixture::WIDE.packages)),
        Scenario::new(
            "index/rtt-deep",
            Fixture::Consumer("deep"),
            RESOLVE_HTTP_RTT,
        )
        .prepare(unlock())
        .expect(Expect::Locks(fixture::DEEP.packages)),
        Scenario::new(
            "index/rtt-lazy",
            Fixture::Consumer("lazy"),
            RESOLVE_HTTP_RTT,
        )
        .prepare(unlock())
        .expect(Expect::Locks(fixture::LAZY.packages)),
        Scenario::new(
            "index/rtt-lazy-pair",
            Fixture::Consumer("lazy-pair"),
            RESOLVE_HTTP_RTT,
        )
        .prepare(unlock())
        .expect(Expect::Locks(fixture::LAZY.packages)),
        Scenario::new("artifact/fetch-cold", Fixture::Consumer("blobs"), FETCH)
            .prepare(Mutation::Remove("cache".to_owned()))
            .expect(archives(fixture::BLOBS.packages)),
        Scenario::new("artifact/fetch-warm", Fixture::Consumer("blobs"), FETCH)
            .setup(FETCH)
            .expect(archives(fixture::BLOBS.packages))
            .smoke(),
        Scenario::new(
            "package/vendor",
            Fixture::Consumer("blobs"),
            &[
                "vendor",
                "--index-path",
                "{registry}",
                "--cache-dir",
                "cache",
                "--vendor-dir",
                "vendor",
            ],
        )
        .setup(FETCH)
        .prepare(Mutation::Remove("vendor".to_owned()))
        .expect(Expect::Entries(
            "vendor/artifacts/bench".to_owned(),
            fixture::BLOBS.packages,
        )),
        // Capture overhead: compare with build/noop-cxx.
        Scenario::new("trace/noop-cxx-info", Fixture::Project(Lang::Cxx), BUILD)
            .setup(BUILD)
            .env("CABIN_TRACE_DIR", "{traces}")
            .expect(Expect::NoActions)
            .smoke(),
        Scenario::new("trace/noop-cxx-debug", Fixture::Project(Lang::Cxx), BUILD)
            .setup(BUILD)
            .env("CABIN_TRACE_DIR", "{traces}")
            .env("CABIN_TRACE_FILTER", "debug")
            .expect(Expect::NoActions),
    ]
}

fn ports_scenarios() -> Vec<Scenario> {
    let ports_fetch: &[&str] = &["fetch", "--index-path", "{ports}", "--cache-dir", "cache"];
    let ports_build: &[&str] = &[
        "build",
        "--no-compiler-wrapper",
        "--index-path",
        "{ports}",
        "--cache-dir",
        "cache",
    ];
    [
        Scenario::new(
            "ports/resolve",
            Fixture::PortsConsumer,
            &["resolve", "--index-path", "{ports}"],
        )
        .prepare(unlock())
        .expect(Expect::Locks(crate::ports::PORTS.len())),
        Scenario::new("ports/fetch-cold", Fixture::PortsConsumer, ports_fetch)
            .prepare(Mutation::Remove("cache".to_owned()))
            .expect(archives(crate::ports::PORTS.len())),
        Scenario::new("ports/build-clean", Fixture::PortsConsumer, ports_build)
            .setup(ports_fetch)
            .prepare(Mutation::Remove("build".to_owned()))
            .expect(Expect::Builds),
        Scenario::new("ports/build-noop", Fixture::PortsConsumer, ports_build)
            .setup(ports_build)
            .expect(Expect::NoActions),
    ]
    .into_iter()
    .map(|scenario| Scenario {
        ports: true,
        ..scenario
    })
    .collect()
}

/// Write the fixture `kind` into `dir`.  Registry consumers reference
/// the families `fixture::FAMILIES` publishes.
pub fn write_fixture(kind: Fixture, dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    match kind {
        Fixture::Empty => Ok(()),
        Fixture::Project(lang) => {
            let shape = PROJECT.into_iter().find(|s| s.lang == lang).unwrap();
            fixture::write_project(dir, "bench", shape)
        }
        Fixture::Workspace => fixture::write_workspace(dir, WORKSPACE_MEMBERS),
        Fixture::Consumer(which) => {
            let deps: Vec<(String, &str, &[&str])> = match which {
                "wide" => (0..fixture::WIDE.packages)
                    .map(|i| (fixture::WIDE.name(i), "^1", &[][..]))
                    .collect(),
                "deep" => vec![(fixture::DEEP.name(0), "^1", &[][..])],
                "lazy" => vec![(fixture::LAZY.name(0), "^1", &["next"][..])],
                // Two halves of the chain, each needing its own rounds.
                "lazy-pair" => [0, fixture::LAZY.packages / 2]
                    .map(|i| (fixture::LAZY.name(i), "^1", &["next"][..]))
                    .into(),
                "blobs" => (0..fixture::BLOBS.packages)
                    .map(|i| (fixture::BLOBS.name(i), "^1", &[][..]))
                    .collect(),
                other => unreachable!("unknown consumer {other}"),
            };
            fixture::write_consumer(dir, Lang::Cxx, &deps)
        }
        Fixture::PortsConsumer => crate::ports::write_consumer(dir),
    }
}
