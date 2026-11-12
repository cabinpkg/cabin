//! The build script these tests run is this test binary itself, which
//! Cabin starts as it starts any script: with no arguments, so every
//! test runs again inside the confinement.  There [`Fixture::new`]
//! returns `None`, which ends each test at once, and [`script`] alone
//! acts, as the `CABIN_SDK_ACTION` it is passed says.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use assert_fs::TempDir;
use cabin_core::{Package, TargetPlatform};
use camino::{Utf8Path, Utf8PathBuf};

use crate::result::{self, Invalid};
use crate::{Error, Script, find, load, run};

const ACTION: &str = "CABIN_SDK_ACTION";
const SECRET: &str = "secret-marker";

const MANIFEST: &str = r#"
[package]
name = "demo"
version = "1.2.3"
c-standard = "c11"

[features]
extras = []
tables = []

[target.demo]
type = "library"
sources = ["src/demo.c"]
interface-defines = ["DEMO_API=1"]

[target.headers]
type = "header-only"
include-dirs = ["include"]
interface-c-standard = "c99"

[target.app]
type = "executable"
sources = ["src/main.c"]

[target.gated]
type = "library"
sources = ["src/gated.c"]
required-features = ["extras"]
"#;

/// A package in a temporary directory, its build directory inside it
/// under `work/`, and this test binary as its build script.  Each path
/// is spelled as resolved.
struct Fixture {
    _temp: TempDir,
    package: Package,
    features: BTreeSet<String>,
    /// The temporary directory, which holds the package in `pkg`.
    base: Utf8PathBuf,
    root: Utf8PathBuf,
    build_dir: Utf8PathBuf,
    dir: Utf8PathBuf,
    executable: Utf8PathBuf,
    tools: Vec<(String, Utf8PathBuf)>,
    inherited: Vec<(OsString, OsString)>,
    platform: TargetPlatform,
}

impl Fixture {
    /// `None` inside the confinement, where this binary is the script.
    fn new(action: &str) -> Option<Self> {
        if std::env::var_os(cabin_env::build_script::CABIN_OUT_DIR).is_some() {
            return None;
        }
        let temp = TempDir::new().unwrap();
        let base = cabin_fs::canonicalize(temp.path()).unwrap();
        let base = Utf8PathBuf::from_path_buf(base).unwrap();
        let root = base.join("pkg");
        for (file, contents) in [
            ("pkg/cabin.toml", MANIFEST),
            ("pkg/src/table.def", "1\n"),
            ("pkg/src/demo.c", "int demo;\n"),
            ("pkg/src/dist/nested.def", ""),
            ("pkg/include/demo.h", ""),
            ("pkg/Dist/config.def", ""),
            ("pkg/.git/config", SECRET),
            ("pkg/work/beside", SECRET),
            ("pkg/work/build-dir/.build-script/out/stale", "stale"),
            ("tools/gen", "generator 1\n"),
            ("sdk/include/sdk.h", ""),
            ("outside", SECRET),
        ] {
            let path = base.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("sdk"), root.join("led")).unwrap();
        let secret = |name: &str| (name.into(), SECRET.into());
        let mut inherited = vec![
            (ACTION.into(), action.into()),
            ("CABIN_SDK_DEMO".into(), base.join("sdk").into()),
            secret("CABIN_REGISTRY_TOKEN"),
            secret("GITHUB_TOKEN"),
            secret("SSH_AUTH_SOCK"),
            secret("HOME"),
        ];
        // What Windows needs to start a program at all.
        inherited.extend(
            ["PATH", "SystemRoot", "ComSpec"]
                .into_iter()
                .filter_map(|name| Some((name.into(), std::env::var_os(name)?))),
        );
        let fixture = Self {
            package: cabin_manifest::parse_manifest_str(MANIFEST)
                .unwrap()
                .package
                .unwrap(),
            features: BTreeSet::from(["tables".to_owned()]),
            executable: Utf8PathBuf::from_path_buf(std::env::current_exe().unwrap()).unwrap(),
            tools: vec![("gen".to_owned(), base.join("tools").join("gen"))],
            inherited,
            platform: TargetPlatform::current(),
            _temp: temp,
            base,
            root,
            build_dir: Utf8PathBuf::new(),
            dir: Utf8PathBuf::new(),
        };
        Some(fixture.built_in("work/build-dir"))
    }

    /// The same package, built in the entry `build_dir` of its root.
    fn built_in(self, build_dir: &str) -> Self {
        // Collected from its components, so spelled as the platform
        // spells the paths a script is told.
        let build_dir: Utf8PathBuf = self.root.join(build_dir).components().collect();
        let dir = build_dir.join(".build-script");
        fs::create_dir_all(&dir).unwrap();
        Self {
            build_dir,
            dir,
            ..self
        }
    }

    fn script(&self) -> Script<'_> {
        Script {
            package: &self.package,
            features: &self.features,
            root: &self.root,
            build_dir: &self.build_dir,
            dir: &self.dir,
            executable: &self.executable,
            profile: "dev",
            platform: &self.platform,
            msvc: false,
            tools: &self.tools,
            inherited: &self.inherited,
        }
    }

    fn out(&self) -> Utf8PathBuf {
        self.dir.join("out")
    }

    /// What reading `text` as a result file refuses, and at which line.
    fn refused(&self, text: impl AsRef<[u8]>) -> (usize, Invalid) {
        let script = self.script();
        let (_, regions) = script.prepare().unwrap();
        result::parse(text.as_ref(), &script, &regions).unwrap_err()
    }
}

fn path_problem(path: impl Into<String>, problem: &'static str) -> Invalid {
    Invalid::Path {
        path: path.into(),
        problem,
    }
}

/// The build script: see the module documentation.
#[test]
fn script() {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let (Some(action), Some(out)) = (var(ACTION), var("CABIN_OUT_DIR")) else {
        return;
    };
    let root = var("CABIN_MANIFEST_DIR").unwrap();
    let result = var("CABIN_RESULT_FILE").unwrap();
    let table = root.join("src").join("table.def");
    match action.to_str().unwrap() {
        "report" => {
            let outcome = |outcome: std::io::Result<()>| match outcome {
                Ok(()) => "done".to_owned(),
                Err(error) => format!("refused ({error})"),
            };
            let read = |path: &Path| outcome(fs::read(path).map(drop));
            // The relative path shows where the script starts.
            let mut seen = format!(
                "table: {}generator: {}",
                fs::read_to_string("src/table.def").unwrap(),
                fs::read_to_string(var("CABIN_BIN_GEN").unwrap()).unwrap(),
            );
            for (name, outcome) in [
                ("read .git", read(&root.join(".git").join("config"))),
                ("read beside the build", read(&root.join("work/beside"))),
                (
                    "read outside",
                    read(&root.parent().unwrap().join("outside")),
                ),
                // Absent on Windows, where the fixture makes no symlink.
                ("read through a link", read(&root.join("led/include/sdk.h"))),
                ("write the package", outcome(fs::write(&table, "2\n"))),
                (
                    "write a stamp",
                    outcome(fs::write(out.with_file_name("stamp"), "")),
                ),
                (
                    "write the script",
                    outcome(fs::write(std::env::current_exe().unwrap(), "")),
                ),
                (
                    "write a home",
                    outcome(fs::write(var("HOME").unwrap().join("file"), "")),
                ),
            ] {
                writeln!(seen, "{name}: {outcome}").unwrap();
            }
            for (name, value) in std::env::vars_os() {
                writeln!(seen, "env {}={}", name.display(), value.display()).unwrap();
            }
            fs::create_dir_all(out.join("include")).unwrap();
            fs::create_dir_all(out.join("gen")).unwrap();
            fs::write(out.join("gen").join("table.c"), "int table[1];\n").unwrap();
            fs::write(out.join("seen"), seen).unwrap();
            let (out, table) = (out.display(), table.display());
            let report = format!(
                r"# What the table generates.

include-dir demo {out}/include
include-dir app {out}/include
source demo {out}/gen/table.c
define demo TABLE_NAME=the table
interface-define demo HAS_TABLE
interface-define demo HAS_TABLE
rerun-if-changed {table}
"
            );
            // Its first two lines end as Windows ends them.
            fs::write(result, report.replacen('\n', "\r\n", 2)).unwrap();
        }
        "fail" => {
            // Not `eprintln!`, which the test harness holds back.
            writeln!(std::io::stderr(), "table.def: a row is \u{1b}[31mbad").unwrap();
            std::process::exit(3);
        }
        "nothing" => {}
        // Reports nothing, in no result file.
        "same" => {
            // Not the `dist` an archive leaves out, on any file system.
            fs::read(root.join("Dist").join("config.def")).unwrap();
            let same = out.join("same.h");
            fs::write(&same, "#define SAME 1\n").unwrap();
            // Its output is the script's to read too.
            let copy = [fs::read(same).unwrap(), fs::read(&table).unwrap()].concat();
            fs::write(out.join("table.h"), copy).unwrap();
            fs::create_dir(out.join("a")).unwrap();
            fs::create_dir(out.join("b")).unwrap();
        }
        // A link in the output directory, or for the result file.
        action @ ("link" | "result link") => {
            let link = if action == "link" {
                out.join("link")
            } else {
                result
            };
            fs::create_dir(out.join("real")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(out.join("real"), link).unwrap();
            #[cfg(windows)]
            assert!(
                std::process::Command::new(var("ComSpec").unwrap())
                    .args(["/C", "mklink", "/J"])
                    .args([link, out.join("real")])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        // A second name for a package file, which no platform allows.
        "second name" => drop(fs::hard_link(&table, out.join("table.def"))),
        action => panic!("unknown action {action}"),
    }
}

#[test]
fn a_script_reports_through_its_result_file_from_inside_its_grants() {
    let Some(fixture) = Fixture::new("report") else {
        return;
    };
    let script = fixture.script();
    // The test harness keeps what a failed `script` printed on its
    // standard output.
    let outputs = run(&script).unwrap_or_else(|error| {
        let printed = fs::read_to_string(fixture.dir.join("stdout"));
        panic!("{error}\n{}", printed.unwrap_or_default())
    });

    let out = fixture.out();
    assert_eq!(
        outputs.keys().collect::<Vec<_>>(),
        ["app", "demo"],
        "{outputs:?}"
    );
    assert_eq!(outputs["app"].include_dirs, [out.join("include")]);
    let demo = &outputs["demo"];
    assert_eq!(demo.include_dirs, [out.join("include")]);
    assert_eq!(demo.sources, [Utf8Path::new("gen").join("table.c")]);
    assert_eq!(demo.defines, ["TABLE_NAME=the table"]);
    assert_eq!(demo.interface_defines, ["HAS_TABLE"]);
    assert!(
        !out.join("stale").exists(),
        "a run starts from an empty output directory"
    );

    let seen = fs::read_to_string(out.join("seen")).unwrap();
    let line = |start: &str| {
        let line = seen.lines().find(|line| line.starts_with(start));
        line.unwrap_or_else(|| panic!("no `{start}` in:\n{seen}"))
    };
    assert_eq!(line("table: "), "table: 1");
    assert_eq!(line("generator: "), "generator: generator 1");
    for refused in [
        "read .git",
        "read beside the build",
        "read outside",
        "read through a link",
        "write the package",
        "write a stamp",
        "write the script",
    ] {
        assert!(line(refused).contains(": refused"), "{}", line(refused));
    }
    assert_eq!(line("write a home"), "write a home: done");
    assert!(!seen.contains(SECRET), "{seen}");
    for set in [
        format!("env CABIN_SDK_DEMO={}", fixture.base.join("sdk")),
        format!("env CABIN_BIN_GEN={}", fixture.tools[0].1),
        format!("env CABIN_MANIFEST_DIR={}", fixture.root),
        "env CABIN_PACKAGE_VERSION=1.2.3".to_owned(),
        "env CABIN_FEATURE_TABLES=1".to_owned(),
        format!("env HOME={}", fixture.dir.join("home")),
    ] {
        assert!(
            seen.lines().any(|line| line == set),
            "no `{set}` in:\n{seen}"
        );
    }
    assert!(!seen.contains("CABIN_FEATURE_EXTRAS"), "{seen}");

    // The run stands until something it depended on changes: here the
    // one file the script named, not the rest of the package.
    let reloaded = load(&script).unwrap();
    assert_eq!(format!("{reloaded:?}"), format!("{outputs:?}"));
    fs::write(fixture.root.join("src").join("demo.c"), "int changed;\n").unwrap();
    assert!(load(&script).is_some());
    fs::write(fixture.root.join("src").join("table.def"), "2\n").unwrap();
    assert!(load(&script).is_none());
    run(&script).unwrap();
    assert!(load(&script).is_some());
    fs::write(&fixture.tools[0].1, "generator 2\n").unwrap();
    assert!(load(&script).is_none());
    run(&script).unwrap();
    // An output directory that has come to hold a link is not used, nor
    // one that lost a file, even a file no line of the result names.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&fixture.root, out.join("led")).unwrap();
        assert!(load(&script).is_none());
        fs::remove_file(out.join("led")).unwrap();
    }
    assert!(load(&script).is_some());
    fs::remove_file(out.join("seen")).unwrap();
    assert!(load(&script).is_none());
}

#[test]
fn a_failed_run_leaves_no_result() {
    let Some(fixture) = Fixture::new("same") else {
        return;
    };
    let script = fixture.script();
    run(&script).unwrap();
    assert!(load(&script).is_some());
    // One output is not taken for another: here a link stands where a
    // directory did, to where the next one lay, and then a pipe where
    // an empty directory did.
    #[cfg(unix)]
    {
        let [a, b] = ["a", "b"].map(|name| fixture.out().join(name));
        fs::remove_dir(&a).unwrap();
        fs::remove_dir(&b).unwrap();
        std::os::unix::fs::symlink(b, &a).unwrap();
        assert!(load(&script).is_none());
        run(&script).unwrap();
        fs::remove_dir(&a).unwrap();
        let mkfifo = std::process::Command::new("mkfifo").arg(a).status();
        assert!(mkfifo.unwrap().success());
        assert!(load(&script).is_none());
        run(&script).unwrap();
    }

    let acting = |action: &str| -> Vec<_> {
        let rest = fixture.inherited.iter().skip(1).cloned();
        [(ACTION.into(), action.into())]
            .into_iter()
            .chain(rest)
            .collect()
    };
    let fail = acting("fail");
    let failing = Script {
        inherited: &fail,
        ..script
    };
    match run(&failing).unwrap_err() {
        Error::Failed {
            status,
            stderr,
            log,
        } => {
            assert_eq!(status.code(), Some(3));
            assert!(
                stderr.ends_with("table.def: a row is \\u{1b}[31mbad"),
                "{stderr}"
            );
            assert_eq!(log, fixture.dir.join("stderr"));
        }
        other => panic!("{other}"),
    }
    // The run before it answers no longer: its output is gone.
    assert!(load(&script).is_none());
    assert!(load(&failing).is_none());
    // Nor does one that left no output, which a failed run leaves as it
    // was.
    let nothing = acting("nothing");
    let idle = Script {
        inherited: &nothing,
        ..script
    };
    assert!(run(&idle).unwrap().is_empty());
    assert!(load(&idle).is_some());
    run(&failing).unwrap_err();
    assert!(load(&idle).is_none());
}

#[test]
fn an_unchanged_output_keeps_its_time_and_any_file_the_script_reads_reruns_one_that_names_none() {
    let Some(fixture) = Fixture::new("same") else {
        return;
    };
    // Built where a build is by default, an entry a package archive
    // leaves out by name, and where the test can make one, through a
    // symlink the script may not read.
    #[cfg(unix)]
    {
        let scratch = fixture.base.join("scratch");
        fs::create_dir(&scratch).unwrap();
        std::os::unix::fs::symlink(scratch, fixture.root.join("build")).unwrap();
    }
    let fixture = fixture.built_in("build");
    let script = fixture.script();
    assert!(run(&script).unwrap().is_empty());
    let [same, copied] = ["same.h", "table.h"].map(|name| fixture.out().join(name));
    let earlier = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    for file in [&same, &copied] {
        let file = fs::File::options().write(true).open(file).unwrap();
        file.set_modified(earlier).unwrap();
    }
    let table = fixture.root.join("src").join("table.def");
    fs::write(table, "2\n").unwrap();

    run(&script).unwrap();
    let modified = |file: &Utf8Path| fs::metadata(file).unwrap().modified().unwrap();
    assert_eq!(modified(&same), earlier);
    assert_ne!(modified(&copied), earlier);

    assert!(load(&script).is_some());
    // Another executable, or another environment, is another run.
    let rebuilt = Script {
        executable: &fixture.tools[0].1,
        ..script
    };
    assert!(load(&rebuilt).is_none());
    let reconfigured = Script {
        profile: "release",
        ..script
    };
    assert!(load(&reconfigured).is_none());
    // What the script cannot read is none of its files; what it can
    // is, under whatever name.
    fs::write(fixture.root.join(".git").join("config"), "changed").unwrap();
    assert!(load(&script).is_some());
    // Nor may a symlink in the root come to lead elsewhere, though the
    // script is granted none.
    #[cfg(unix)]
    {
        let led = fixture.root.join("led");
        fs::remove_file(&led).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("src"), led).unwrap();
        assert!(load(&script).is_none());
        run(&script).unwrap();
    }
    let nested = fixture.root.join("src").join("dist").join("nested.def");
    fs::write(nested, "changed").unwrap();
    assert!(load(&script).is_none());
}

#[test]
fn what_a_script_links_into_its_directories_is_refused() {
    for (action, link) in [("link", "out/link"), ("result link", "result/result")] {
        let Some(fixture) = Fixture::new(action) else {
            return;
        };
        let link = fixture.dir.join(link);
        match run(&fixture.script()).unwrap_err() {
            Error::Link(path) => assert_eq!(path, link),
            other => panic!("{other}"),
        }
        // Nor may a directory itself have become one.
        let refused = crate::scan(link.as_std_path()).unwrap_err();
        assert!(matches!(refused, Error::Link(path) if path == link));
    }

    // A confined script cannot give a file of the package a second
    // name.
    let Some(fixture) = Fixture::new("second name") else {
        return;
    };
    let second = fixture.out().join("table.def");
    run(&fixture.script()).unwrap();
    assert!(!second.exists());
    // So the check itself, where the test can make one.
    #[cfg(unix)]
    {
        fs::hard_link(fixture.root.join("src").join("table.def"), &second).unwrap();
        let refused = crate::scan(fixture.out().as_std_path()).unwrap_err();
        assert!(matches!(refused, Error::Link(path) if path == second));
    }
}

#[test]
#[allow(clippy::too_many_lines)] // one independent case per rule
fn a_result_file_is_refused_at_its_first_bad_line() {
    let Some(fixture) = Fixture::new("") else {
        return;
    };
    let (root, out) = (&fixture.root, fixture.out());
    fs::create_dir_all(out.join("include")).unwrap();
    fs::write(out.join("gen.c"), "").unwrap();
    let (not_offered, unwatched) = (result::NOT_OFFERED, result::UNWATCHED);
    let cases: Vec<(String, usize, Invalid)> = vec![
        (
            "link-file demo /usr/lib/libz.a".into(),
            1,
            Invalid::UnknownDirective("link-file".into()),
        ),
        (
            " define demo A".into(),
            1,
            Invalid::UnknownDirective(String::new()),
        ),
        ("define demo".into(), 1, Invalid::MissingValue("define")),
        (
            "# fine\n\ndefine demo A\ndefine other A".into(),
            4,
            Invalid::UnknownTarget("other".into()),
        ),
        (
            "define gated A".into(),
            1,
            Invalid::DisabledTarget {
                target: "gated".into(),
                missing: vec!["extras".into()],
            },
        ),
        (
            "define headers A".into(),
            1,
            Invalid::Kind {
                directive: "define",
                target: "headers".into(),
                kind: "header-only",
            },
        ),
        (
            format!("source headers {out}/gen.c"),
            1,
            Invalid::Kind {
                directive: "source",
                target: "headers".into(),
                kind: "header-only",
            },
        ),
        (
            "interface-define app A".into(),
            1,
            Invalid::Kind {
                directive: "interface-define",
                target: "app".into(),
                kind: "executable",
            },
        ),
        ("define demo 1A".into(), 1, Invalid::Define("1A".into())),
        (
            "interface-define demo A=\u{1b}".into(),
            1,
            Invalid::Define("A=\u{1b}".into()),
        ),
        (
            "interface-define demo DEMO_API=1".into(),
            1,
            Invalid::Redefined {
                target: "demo".into(),
                name: "DEMO_API".into(),
            },
        ),
        (
            "interface-define demo A=1\ninterface-define demo A=1\r\ninterface-define demo A=2"
                .into(),
            3,
            Invalid::Redefined {
                target: "demo".into(),
                name: "A".into(),
            },
        ),
        (
            "include-dir demo include".into(),
            1,
            path_problem("include", "is not an absolute path"),
        ),
        (
            format!("include-dir demo {out}/missing"),
            1,
            path_problem(
                format!("{out}/missing"),
                "does not lead to anything that exists",
            ),
        ),
        (
            format!("include-dir demo {out}/gen.c"),
            1,
            path_problem(format!("{out}/gen.c"), "is not a directory"),
        ),
        (
            format!("include-dir demo {root}/include"),
            1,
            path_problem(format!("{root}/include"), not_offered),
        ),
        (
            format!("include-dir demo {}", fixture.dir),
            1,
            path_problem(fixture.dir.as_str(), not_offered),
        ),
        (
            format!("source demo {root}/src/demo.c"),
            1,
            path_problem(format!("{root}/src/demo.c"), result::NOT_OUTPUT),
        ),
        // Refused by its spelling, before anything is asked about it.
        (
            format!("source demo {}/missing.c", fixture.base),
            1,
            path_problem(format!("{}/missing.c", fixture.base), result::NOT_OUTPUT),
        ),
        (
            format!("source demo {out}/include/../gen.c"),
            1,
            path_problem(format!("{out}/include/../gen.c"), "has a `..` in it"),
        ),
        (
            format!("source demo {out}/include"),
            1,
            path_problem(format!("{out}/include"), "is not a file"),
        ),
        (
            format!("rerun-if-changed {root}/.git/config"),
            1,
            path_problem(format!("{root}/.git/config"), unwatched),
        ),
        (
            format!("rerun-if-changed {root}/work/beside"),
            1,
            path_problem(format!("{root}/work/beside"), unwatched),
        ),
        (
            format!("rerun-if-changed {root}"),
            1,
            path_problem(root.as_str(), unwatched),
        ),
        (
            format!("rerun-if-changed {}/outside", fixture.base),
            1,
            path_problem(format!("{}/outside", fixture.base), unwatched),
        ),
    ];
    for (text, line, invalid) in cases {
        assert_eq!(fixture.refused(&text), (line, invalid), "{text}");
    }
    assert_eq!(
        fixture.refused(b"define demo A\ndefine demo \xff\n"),
        (2, Invalid::NotUtf8)
    );
    // A path is judged by where it leads.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("src"), out.join("led")).unwrap();
        assert_eq!(
            fixture.refused(format!("source demo {out}/led/demo.c")),
            (
                1,
                path_problem(format!("{out}/led/demo.c"), result::NOT_OUTPUT)
            )
        );
        assert_eq!(
            fixture.refused(format!("include-dir demo {out}/led")),
            (1, path_problem(format!("{out}/led"), not_offered))
        );
    }

    // What the script may read of the package is what it may watch, and
    // the rest stays unread wherever the package lies.
    let script = fixture.script();
    let (_, regions) = script.prepare().unwrap();
    let read = regions.readable().unwrap();
    let granted: BTreeSet<_> = read
        .iter()
        .filter_map(|path| path.strip_prefix(&regions.root).ok()?.to_str())
        .collect();
    assert_eq!(
        granted,
        BTreeSet::from(["Dist", "cabin.toml", "include", "src"])
    );
    let hidden = regions.hidden().unwrap();
    assert!(hidden.contains(&regions.root.join(".git")));

    // What lies outside the package is the user's to offer, and one
    // file is one source however a script spells it.
    let sdk = fixture.base.join("sdk").join("include");
    let text = format!(
        r"include-dir headers {sdk}
rerun-if-changed {sdk}/sdk.h
source demo {out}/gen.c
source demo {out}/./gen.c
"
    );
    let report = result::parse(text.as_bytes(), &script, &regions).unwrap();
    assert_eq!(report.targets["headers"].include_dirs, [sdk.as_path()]);
    assert_eq!(report.targets["demo"].sources, [Utf8Path::new("gen.c")]);
    assert_eq!(report.inputs, [sdk.join("sdk.h")]);

    // A package in the build directory would have the build to read.
    let around = Script {
        build_dir: &fixture.base,
        ..script
    };
    assert!(matches!(run(&around), Err(Error::PackageInBuildDir(at)) if at == *root));
}

#[test]
fn a_build_script_is_found_by_its_exact_name() {
    let Some(fixture) = Fixture::new("") else {
        return;
    };
    let root = &fixture.root;
    assert_eq!(find(root).unwrap(), None);
    fs::write(root.join("Build.c"), "").unwrap();
    assert_eq!(find(root).unwrap(), None);
    fs::write(root.join("build.cpp"), "").unwrap();
    assert_eq!(find(root).unwrap(), Some("build.cpp"));
    fs::remove_file(root.join("Build.c")).unwrap();
    fs::create_dir(root.join("build.c")).unwrap();
    assert_eq!(find(root).unwrap(), Some("build.cpp"));
    fs::remove_dir(root.join("build.c")).unwrap();
    fs::write(root.join("build.c"), "").unwrap();
    assert!(matches!(find(root).unwrap_err(), Error::Ambiguous(at) if at == *root));
    fs::remove_file(root.join("build.cpp")).unwrap();
    assert_eq!(find(root).unwrap(), Some("build.c"));
}
