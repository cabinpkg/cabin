//! Real-world inputs: a few foundation ports of the pinned catalog
//! (`--ports`), staged into a file registry by the publisher's own
//! staging step (`cabin_publish::upstream`).  The first staging
//! downloads the pinned upstream archives; later runs reuse them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cabin_publish::upstream::{ArchiveFetch, stage_committed_package};

/// Two C and two C++ libraries, one with an inter-port dependency.
pub const PORTS: [(&str, &str); 4] = [
    ("cabin-ports/zlib", "^1.3"),
    ("cabin-ports/libpng", "^1.6"),
    ("cabin-ports/fmt", "^12"),
    ("cabin-ports/spdlog", "^1.17"),
];

pub fn stage_registry(ports: &Path, download_cache: &Path, work: &Path) -> Result<PathBuf> {
    let registry = work.join("registry");
    for (name, _) in PORTS {
        let short = name.trim_start_matches("cabin-ports/");
        // `<prefix>/<name>`: the prefix is the name's first two characters.
        let name_dir = ports.join(&short[..2]).join(short);
        let mut versions = std::fs::read_dir(&name_dir)
            .with_context(|| format!("reading {}", name_dir.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        versions.retain(|path| path.is_dir());
        versions.sort();
        for package_dir in versions {
            stage_committed_package(
                &package_dir,
                download_cache,
                &work.join("src"),
                &registry,
                ArchiveFetch::CacheOrDownload,
            )
            .with_context(|| format!("staging {}", package_dir.display()))?;
        }
    }
    Ok(registry)
}

pub fn write_consumer(dir: &Path) -> Result<()> {
    let mut deps = String::new();
    for (name, req) in PORTS {
        writeln!(deps, "\"{name}\" = \"{req}\"")?;
    }
    let links = PORTS
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(
        dir.join("cabin.toml"),
        format!(
            r#"[package]
name = "consumer"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
{deps}
[target.consumer]
type = "executable"
sources = ["src/main.cc"]
deps = [{links}]
"#
        ),
    )?;
    std::fs::write(dir.join("src/main.cc"), "int main() { return 0; }\n")?;
    Ok(())
}
