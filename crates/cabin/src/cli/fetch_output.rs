//! Human and JSON output rendering for `cabin fetch`.

use std::path::Path;

use anyhow::Result;
use cabin_artifact::FetchedPackage;

use crate::cli::ResolveFormat;

pub(crate) fn emit_fetch_output(
    fetched: &[FetchedPackage],
    format: ResolveFormat,
    cache_dir: &Path,
    manifest_path: &Path,
) -> Result<()> {
    match format {
        ResolveFormat::Human => {
            print_fetch_human(fetched, cache_dir, manifest_path);
            Ok(())
        }
        ResolveFormat::Json => print_fetch_json(fetched, cache_dir, manifest_path),
    }
}

fn print_fetch_human(fetched: &[FetchedPackage], cache_dir: &Path, manifest_path: &Path) {
    if fetched.is_empty() {
        println!("Fetched artifacts:");
        println!("  (no registry dependencies to fetch)");
        return;
    }
    println!("Fetched artifacts:");
    for pkg in fetched {
        let source = display_relative(&pkg.source_dir, cache_dir, manifest_path);
        println!("  {} {} -> {}", pkg.name.as_str(), pkg.version, source);
    }
}

fn print_fetch_json(
    fetched: &[FetchedPackage],
    cache_dir: &Path,
    manifest_path: &Path,
) -> Result<()> {
    let packages: Vec<_> = fetched
        .iter()
        .map(|pkg| {
            let source_dir = display_relative(&pkg.source_dir, cache_dir, manifest_path);
            serde_json::json!({
                "name": pkg.name.as_str(),
                "version": pkg.version.to_string(),
                "checksum": pkg.checksum,
                "source_dir": source_dir,
            })
        })
        .collect();
    crate::print_pretty_json(
        &serde_json::json!({ "packages": packages }),
        "failed to serialize fetch output as JSON",
    )
}

/// Best-effort short representation of a path for display.  Trim the
/// manifest's package root so output stays readable; if the path is
/// not under that root, return it unchanged.
fn display_relative(path: &Path, _cache_dir: &Path, manifest_path: &Path) -> String {
    let project_root = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Ok(stripped) = path.strip_prefix(&project_root) {
        return stripped.display().to_string();
    }
    path.display().to_string()
}
