//! Synthetic benchmark inputs: C and C++ projects, a workspace, and a
//! file registry, generated from fixed shapes so every run and every
//! binary under comparison measures identical inputs.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

/// Bumped whenever a generator's output changes, so results from
/// different inputs are never compared silently.
pub const FIXTURE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lang {
    C,
    Cxx,
}

impl Lang {
    pub fn suffix(self) -> &'static str {
        match self {
            Lang::C => "c",
            Lang::Cxx => "cxx",
        }
    }

    fn source_ext(self) -> &'static str {
        match self {
            Lang::C => "c",
            Lang::Cxx => "cc",
        }
    }

    fn header_ext(self) -> &'static str {
        match self {
            Lang::C => "h",
            Lang::Cxx => "hpp",
        }
    }

    fn standards(self) -> &'static str {
        match self {
            Lang::C => "c-standard = \"c11\"\ninterface-c-standard = \"c11\"\n",
            Lang::Cxx => "cxx-standard = \"c++17\"\ninterface-cxx-standard = \"c++17\"\n",
        }
    }

    /// Standard headers each translation unit includes, so a compile
    /// costs what a small real one does rather than nothing.
    fn system_includes(self) -> &'static str {
        match self {
            Lang::C => "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
            Lang::Cxx => "#include <map>\n#include <string>\n#include <vector>\n",
        }
    }
}

/// One package: a `core` library of `sources` translation units, an
/// `app` executable, and `tests` test targets.  Even-numbered sources
/// include `include/bench/common.<h>`; odd-numbered ones do not.
#[derive(Debug, Clone, Copy)]
pub struct ProjectShape {
    pub lang: Lang,
    pub sources: usize,
    pub tests: usize,
}

impl ProjectShape {
    /// Translation units that include the shared header.
    pub fn common_includers(self) -> usize {
        self.sources.div_ceil(2)
    }

    /// Path of an odd-numbered source, which nothing else includes.
    pub fn leaf_source(self) -> String {
        format!("src/core/f1.{}", self.lang.source_ext())
    }

    pub fn common_header(self) -> String {
        format!("include/bench/common.{}", self.lang.header_ext())
    }
}

/// Write the package `shape` describes into `dir`, named `name`.
///
/// # Errors
/// Returns the first file-system failure.
pub fn write_project(dir: &Path, name: &str, shape: ProjectShape) -> Result<()> {
    let ext = shape.lang.source_ext();
    let hdr = shape.lang.header_ext();
    let mut manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n{}\n",
        shape.lang.standards()
    );
    let core_sources = (0..shape.sources)
        .map(|i| format!("\"src/core/f{i}.{ext}\""))
        .collect::<Vec<_>>()
        .join(", ");
    write!(
        manifest,
        "[target.core]\ntype = \"library\"\nsources = [{core_sources}]\ninclude-dirs = [\"include\"]\n\n\
         [target.app]\ntype = \"executable\"\nsources = [\"src/main.{ext}\"]\ndeps = [\"core\"]\n"
    )?;
    for t in 0..shape.tests {
        write!(
            manifest,
            "\n[target.t{t}]\ntype = \"test\"\nsources = [\"tests/t{t}.{ext}\"]\ndeps = [\"core\"]\n"
        )?;
    }
    write(dir, "cabin.toml", &manifest)?;

    let mut decls = String::new();
    for i in 0..shape.sources {
        writeln!(decls, "int bench_f{i}(int x);")?;
    }
    write(
        dir,
        &format!("include/bench/api.{hdr}"),
        &guarded("API", &decls),
    )?;
    write(
        dir,
        &format!("include/bench/common.{hdr}"),
        &guarded(
            "COMMON",
            "static inline int bench_common(int x) { return x * 3 + 1; }\n",
        ),
    )?;
    for i in 0..shape.sources {
        let common = if i.is_multiple_of(2) {
            format!("#include \"bench/common.{hdr}\"\n")
        } else {
            String::new()
        };
        let body = if i.is_multiple_of(2) {
            "bench_common(x)"
        } else {
            "x + 1"
        };
        write(
            dir,
            &format!("src/core/f{i}.{ext}"),
            &format!(
                "{}#include \"bench/api.{hdr}\"\n{common}\nint bench_f{i}(int x) {{ return {body} * {}; }}\n",
                shape.lang.system_includes(),
                i + 1
            ),
        )?;
    }
    write(
        dir,
        &format!("src/main.{ext}"),
        &format!(
            "#include \"bench/api.{hdr}\"\n\nint main(void) {{ return bench_f0(0) == 0 ? 0 : 1; }}\n"
        ),
    )?;
    for t in 0..shape.tests {
        write(
            dir,
            &format!("tests/t{t}.{ext}"),
            &format!(
                "#include \"bench/api.{hdr}\"\n\nint main(void) {{ return bench_f0({t}) < 0; }}\n"
            ),
        )?;
    }
    Ok(())
}

/// A workspace of `members` packages under `packages/`, alternating C
/// and C++, each linking the previous one through a path dependency.
///
/// # Errors
/// Returns the first file-system failure.
pub fn write_workspace(dir: &Path, members: usize) -> Result<()> {
    write(
        dir,
        "cabin.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for i in 0..members {
        let lang = if i.is_multiple_of(2) {
            Lang::C
        } else {
            Lang::Cxx
        };
        let ext = lang.source_ext();
        let mut manifest = format!(
            "[package]\nname = \"m{i:03}\"\nversion = \"0.1.0\"\n{}",
            Lang::C.standards()
        );
        if lang == Lang::Cxx {
            manifest.push_str("cxx-standard = \"c++17\"\n");
        }
        let mut deps = String::new();
        if i > 0 {
            let prev = i - 1;
            write!(
                manifest,
                "\n[dependencies]\nm{prev:03} = {{ path = \"../m{prev:03}\" }}\n"
            )?;
            deps = format!("deps = [{{ name = \"m{prev:03}\", public = true }}]\n");
        }
        write!(
            manifest,
            "\n[target.m{i:03}]\ntype = \"library\"\nsources = [\"src/a.{ext}\", \"src/b.{ext}\"]\n\
             include-dirs = [\"include\"]\n{deps}"
        )?;
        let pkg = dir.join(format!("packages/m{i:03}"));
        write(&pkg, "cabin.toml", &manifest)?;
        // Every member exposes a C header, so a C++ member and a C
        // member can each consume the other.
        write(
            &pkg,
            &format!("include/m{i:03}/api.h"),
            &guarded(
                &format!("M{i:03}"),
                &format!("int m{i:03}_a(int x);\nint m{i:03}_b(int x);\n"),
            ),
        )?;
        let call = if i > 0 {
            format!("m{:03}_a(x)", i - 1)
        } else {
            "x".to_owned()
        };
        let prev_include = if i > 0 {
            format!("#include \"m{:03}/api.h\"\n", i - 1)
        } else {
            String::new()
        };
        for (part, expr) in [("a", call.as_str()), ("b", "x + 2")] {
            write(
                &pkg,
                &format!("src/{part}.{ext}"),
                &format!(
                    "{}{prev_include}#include \"m{i:03}/api.h\"\n\nint m{i:03}_{part}(int x) {{ return {expr}; }}\n",
                    lang.system_includes()
                ),
            )?;
        }
    }
    Ok(())
}

/// Shape of the synthetic registry: every package is
/// `bench/<prefix><index>` and every version `1.<minor>.0`.
#[derive(Debug, Clone, Copy)]
pub struct RegistryFamily {
    pub prefix: &'static str,
    pub packages: usize,
    pub versions: usize,
    /// Each version depends on the next `fanout` packages at `^1`.
    pub fanout: usize,
    /// The newest version of each package requires a minor of its
    /// successor that does not exist, so the solver must reject it
    /// and fall back, as it does with a broken newest release.
    pub unsatisfiable_newest: bool,
    /// Each package's successor is optional, enabled through its
    /// `next` feature: a lazily loaded index discovers one level per
    /// resolution round.
    pub optional_chain: bool,
    /// Extra source files per package, to give archives some weight.
    pub payload_files: usize,
}

impl RegistryFamily {
    pub fn name(&self, index: usize) -> String {
        format!("bench/{}{index:03}", self.prefix)
    }
}

/// The families every generated registry holds.
pub const WIDE: RegistryFamily = RegistryFamily {
    prefix: "w",
    packages: 100,
    versions: 5,
    fanout: 3,
    unsatisfiable_newest: true,
    optional_chain: false,
    payload_files: 0,
};
pub const DEEP: RegistryFamily = RegistryFamily {
    prefix: "d",
    packages: 60,
    versions: 3,
    fanout: 1,
    unsatisfiable_newest: false,
    optional_chain: false,
    payload_files: 0,
};
pub const LAZY: RegistryFamily = RegistryFamily {
    prefix: "o",
    packages: 8,
    versions: 1,
    fanout: 1,
    unsatisfiable_newest: false,
    optional_chain: true,
    payload_files: 0,
};
pub const BLOBS: RegistryFamily = RegistryFamily {
    prefix: "b",
    packages: 20,
    versions: 1,
    fanout: 0,
    unsatisfiable_newest: false,
    optional_chain: false,
    payload_files: 40,
};
pub const FAMILIES: [RegistryFamily; 4] = [WIDE, DEEP, LAZY, BLOBS];

/// Publish every family into a file registry at `registry`, staging
/// package sources under `staging`.  Goes through the same
/// `cabin publish --registry-dir` code path users do, so the index
/// documents and archives are real ones.
///
/// # Errors
/// Returns the first file-system or publish failure.
pub fn write_registry(registry: &Path, staging: &Path) -> Result<()> {
    for family in FAMILIES {
        for index in 0..family.packages {
            for minor in 0..family.versions {
                let dir = staging.join(format!("{}{index:03}-{minor}", family.prefix));
                write_registry_package(&dir, &family, index, minor)?;
                cabin_publish::publish_to_file_registry(cabin_publish::RegistryPublishWorkflow {
                    manifest_path: &dir.join("cabin.toml"),
                    registry_dir: registry,
                    resolved_project: None,
                    workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
                    new_revision: false,
                })
                .with_context(|| format!("publishing {} 1.{minor}.0", family.name(index)))?;
            }
        }
    }
    Ok(())
}

fn write_registry_package(
    dir: &Path,
    family: &RegistryFamily,
    index: usize,
    minor: usize,
) -> Result<()> {
    let lang = if index.is_multiple_of(2) {
        Lang::C
    } else {
        Lang::Cxx
    };
    let ext = lang.source_ext();
    let ident = format!("{}{index:03}", family.prefix);
    let mut manifest = format!(
        "[package]\nname = \"{}\"\nversion = \"1.{minor}.0\"\n{}",
        family.name(index),
        lang.standards()
    );
    let successors: Vec<usize> = (index + 1..=index + family.fanout)
        .filter(|&next| next < family.packages)
        .collect();
    if !successors.is_empty() {
        manifest.push_str("\n[dependencies]\n");
        for &next in &successors {
            let newest = minor + 1 == family.versions;
            let req = if family.unsatisfiable_newest && newest && next == index + 1 {
                format!("^1.{}", family.versions)
            } else {
                "^1".to_owned()
            };
            if family.optional_chain {
                writeln!(
                    manifest,
                    "\"{}\" = {{ version = \"{req}\", optional = true }}",
                    family.name(next)
                )?;
            } else {
                writeln!(manifest, "\"{}\" = \"{req}\"", family.name(next))?;
            }
        }
    }
    if family.optional_chain {
        // The chain's last package declares `next` too, so every
        // `<dep>/next` request names a real feature.
        let next = if index + 1 < family.packages {
            let next = family.name(index + 1);
            format!("\"dep:{next}\", \"{next}/next\"")
        } else {
            String::new()
        };
        write!(manifest, "\n[features]\nnext = [{next}]\n")?;
    }
    let mut sources = vec![format!("\"src/{ident}.{ext}\"")];
    sources.extend((0..family.payload_files).map(|p| format!("\"src/payload{p}.{ext}\"")));
    write!(
        manifest,
        "\n[target.{ident}]\ntype = \"library\"\nsources = [{}]\ninclude-dirs = [\"include\"]\n",
        sources.join(", ")
    )?;
    write(dir, "cabin.toml", &manifest)?;
    write(
        dir,
        &format!("include/{ident}/{ident}.h"),
        &guarded(&ident.to_uppercase(), &format!("int {ident}(int x);\n")),
    )?;
    write(
        dir,
        &format!("src/{ident}.{ext}"),
        &format!(
            "#include \"{ident}/{ident}.h\"\n\nint {ident}(int x) {{ return x + {minor}; }}\n"
        ),
    )?;
    // Distinct, poorly compressible bytes per file so archive size
    // tracks file count.
    for p in 0..family.payload_files {
        let mut body = format!("static const unsigned char {ident}_payload{p}[] = {{\n");
        let mut state = u32::try_from(index * 7919 + p * 104_729 + 1)?;
        for _ in 0..256 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            for byte in state.to_le_bytes() {
                write!(body, "{byte},")?;
            }
            body.push('\n');
        }
        body.push_str("};\n");
        write(dir, &format!("src/payload{p}.{ext}"), &body)?;
    }
    Ok(())
}

/// A consumer of registry packages: one executable whose package
/// depends on `dependencies` (name, requirement, features).
///
/// # Errors
/// Returns the first file-system failure.
pub fn write_consumer(
    dir: &Path,
    lang: Lang,
    dependencies: &[(String, &str, &[&str])],
) -> Result<()> {
    let ext = lang.source_ext();
    let mut manifest = format!(
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n{}\n[dependencies]\n",
        lang.standards()
    );
    for (name, req, features) in dependencies {
        if features.is_empty() {
            writeln!(manifest, "\"{name}\" = \"{req}\"")?;
        } else {
            let features = features
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                manifest,
                "\"{name}\" = {{ version = \"{req}\", features = [{features}] }}"
            )?;
        }
    }
    write!(
        manifest,
        "\n[target.consumer]\ntype = \"executable\"\nsources = [\"src/main.{ext}\"]\n"
    )?;
    write(dir, "cabin.toml", &manifest)?;
    write(
        dir,
        &format!("src/main.{ext}"),
        "int main(void) { return 0; }\n",
    )
}

/// The directory below `base` that generated fixtures are cached in,
/// named after the running executable's size and modification time:
/// the registry is written by the linked `cabin-publish`, so a rebuilt
/// harness may write another format, and a stale copy would still pass
/// every check.  Copies an older build left behind are removed.
///
/// # Errors
/// Returns a file-system failure reading the executable's metadata.
pub fn cache_root(base: &Path) -> Result<PathBuf> {
    let exe = fs::metadata(std::env::current_exe()?)?;
    let modified = exe.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    let key = format!("{:x}-{modified:x}", exe.len());
    let root = base.join("fixtures");
    for entry in fs::read_dir(&root).into_iter().flatten().flatten() {
        if entry.file_name() != key.as_str() {
            // Another process of this build may be pruning it too.
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(root.join(key))
}

/// Generate `dir` once and reuse it afterwards.  Generation happens in
/// a sibling directory renamed into place, so an interrupted run never
/// leaves a half-written fixture and concurrent processes (nextest
/// runs each benchmark in its own) agree on one copy.
///
/// # Errors
/// Returns the generator's failure or a file-system one.
pub fn cached(dir: &Path, generate: impl FnOnce(&Path) -> Result<()>) -> Result<PathBuf> {
    if !dir.exists() {
        let mut staging = dir.as_os_str().to_owned();
        staging.push(format!(".tmp-{}", std::process::id()));
        let staging = PathBuf::from(staging);
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        generate(&staging).with_context(|| format!("generating {}", dir.display()))?;
        if fs::rename(&staging, dir).is_err() && dir.exists() {
            // Another process finished first; its copy is identical.
            fs::remove_dir_all(&staging)?;
        }
    }
    Ok(dir.to_path_buf())
}

/// An include-guarded header with C linkage, so C and C++ sources can
/// define and call each other's functions.
fn guarded(tag: &str, body: &str) -> String {
    format!(
        "#ifndef BENCH_{tag}_H\n#define BENCH_{tag}_H\n\n#ifdef __cplusplus\nextern \"C\" {{\n#endif\n\n{body}\n\
         #ifdef __cplusplus\n}}\n#endif\n\n#endif\n"
    )
}

fn write(dir: &Path, relative: &str, contents: &str) -> Result<()> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}
