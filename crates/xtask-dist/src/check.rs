//! The packaged archive, unpacked and run (`cargo dist-check`): what the
//! release attests and publishes must hold a binary that starts on its
//! own target and reports the workspace version, which a tag must equal.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

/// The workspace version this tool was built from - the one `cabin` was
/// built from in the same run.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Unpack `archive` beside itself and run the `cabin` inside it.
///
/// # Errors
///
/// A tag that is not the workspace version, an archive that does not
/// unpack, or a binary that does not run and print `cabin <version>`.
pub fn run(archive: &Path, ref_name: &str, ref_type: &str) -> Result<()> {
    if ref_type == "tag" {
        ensure!(
            ref_name == VERSION,
            "tag {ref_name:?} is not the workspace version {VERSION}"
        );
    }
    let windows = cfg!(windows);
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .context("the archive path has no UTF-8 file name")?;
    let Some(package) = name.strip_suffix(if windows { ".zip" } else { ".tar.xz" }) else {
        bail!("{name} is not a packaged archive");
    };

    let unpacked = PathBuf::from(format!("{}.unpacked", archive.display()));
    if unpacked.exists() {
        std::fs::remove_dir_all(&unpacked)
            .with_context(|| format!("removing {}", unpacked.display()))?;
    }
    std::fs::create_dir_all(&unpacked)
        .with_context(|| format!("creating {}", unpacked.display()))?;
    let argv = unpack_argv(archive, &unpacked, windows);
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .with_context(|| format!("running {}", argv[0]))?;
    ensure!(status.success(), "{} failed to unpack {name}", argv[0]);

    let binary = unpacked
        .join(package)
        .join(if windows { "cabin.exe" } else { "cabin" });
    let output = Command::new(&binary)
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", binary.display()))?;
    ensure!(
        output.status.success(),
        "{} --version failed: {}",
        binary.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    let printed = printed.trim_end();
    ensure!(
        printed == format!("cabin {VERSION}"),
        "{} printed {printed:?}, not \"cabin {VERSION}\"",
        binary.display()
    );
    println!("{}: {printed}", binary.display());
    Ok(())
}

/// The packager's archivers, reversed: `tar` on Unix and PowerShell's
/// `Expand-Archive` for the `Compress-Archive` zip on Windows.
fn unpack_argv(archive: &Path, into: &Path, windows: bool) -> Vec<String> {
    let (archive, into) = (archive.display().to_string(), into.display().to_string());
    if windows {
        // Neither path may contain `'`, which would end its literal.
        // `package` confines a tag version's characters; the rest of
        // each path is unchecked here and safe because `dist.yml`
        // passes the matrix target, the commit SHA, and its own paths.
        vec![
            "powershell".to_owned(),
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            format!("Expand-Archive -LiteralPath '{archive}' -DestinationPath '{into}'"),
        ]
    } else {
        vec![
            "tar".to_owned(),
            "-xf".to_owned(),
            archive,
            "-C".to_owned(),
            into,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_other_than_the_workspace_version_is_refused_before_unpacking() {
        let err = run(Path::new("missing.tar.xz"), "0.0.0-not-this", "tag").unwrap_err();
        assert!(
            err.to_string().contains("is not the workspace version"),
            "{err}"
        );
    }
}
