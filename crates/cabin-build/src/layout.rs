//! Where build outputs live under the build directory.  Every command
//! that writes, reads, or removes outputs derives their paths here:
//!
//! ```text
//! <build-dir>/<profile>/                     the build machine's outputs
//! <build-dir>/.targets/<triple>/<profile>/   `--target <triple>` outputs
//! <build-dir>/.cabin-gc/                     usage catalog (crate::gc)
//! <build-dir>/.cabin-build.lock              lock (crate::gc::BuildDirLock)
//! <build-dir>/.cabin-workspace               owner record (crate::owner)
//!
//! <profile-dir>/build.ninja, compile_commands.json, cabin-toolchain.json
//! <profile-dir>/packages/<pkg>/              the target context's units
//! <profile-dir>/build-machine/packages/<pkg>/   the build context's units
//! ```
//!
//! Profile names cannot start with `.`, so no profile directory is
//! `.targets` or another internal entry, and a triple only ever names a
//! directory inside `.targets`.  The triple grammar rejects spellings
//! Windows or a case-insensitive filesystem would alias, and profile
//! resolution refuses two profiles that would, so two distinct
//! profiles or triples never share a directory.

use std::path::{Path, PathBuf};

use cabin_core::{PackageName, ProfileName, TargetPlatform};

/// The build-directory entry holding one directory per `--target` triple.
pub const TARGETS_DIR: &str = ".targets";

/// The directory under a profile directory that holds the build
/// context's units.
pub const BUILD_CONTEXT_DIR: &str = "build-machine";

/// The directory holding one platform's profile directories:
/// `build_dir` itself for the build machine, `.targets/<triple>` under
/// it for a `--target` build.
pub fn platform_dir(build_dir: &Path, target: Option<&TargetPlatform>) -> PathBuf {
    match target {
        Some(target) => build_dir.join(TARGETS_DIR).join(&target.target),
        None => build_dir.to_path_buf(),
    }
}

/// The directory one profile's build writes for a platform.
pub fn profile_dir(platform_dir: &Path, profile: &ProfileName) -> PathBuf {
    platform_dir.join(profile.as_str())
}

/// One package's output directory in a profile directory, in the target
/// context or the build context.  A scoped package nests as
/// `packages/<scope>/<name>`; the full name is never one path component.
pub fn package_dir(profile_dir: &Path, build_context: bool, package: &PackageName) -> PathBuf {
    let packages = if build_context {
        profile_dir.join(BUILD_CONTEXT_DIR).join("packages")
    } else {
        profile_dir.join("packages")
    };
    package
        .path_components()
        .fold(packages, |dir, component| dir.join(component))
}

/// A directory below `profile_dir` holding a `build.ninja` of its own,
/// when there is one: `profile_dir` is then not a profile's tree but a
/// `--target` triple's from before `.targets`, when a cross build wrote
/// `<build-dir>/<triple>/<profile>/`.  Nothing Cabin writes now puts a
/// `build.ninja` one level into a profile directory.
pub fn previous_target_tree(profile_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(profile_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| dir.join("build.ninja").is_file())
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_builds_live_apart_from_every_profile() {
        let build = Path::new("build");
        let triple = TargetPlatform::from_triple("x86_64-unknown-linux-gnu").unwrap();
        // A profile may be named like a triple; its tree still differs.
        let profile = ProfileName::new("x86_64-unknown-linux-gnu".to_owned()).unwrap();
        let native = profile_dir(&platform_dir(build, None), &profile);
        let cross = profile_dir(&platform_dir(build, Some(&triple)), &profile);
        assert_eq!(native, Path::new("build/x86_64-unknown-linux-gnu"));
        assert_eq!(
            cross,
            Path::new("build/.targets/x86_64-unknown-linux-gnu/x86_64-unknown-linux-gnu")
        );
        assert!(!cross.starts_with(&native) && !native.starts_with(&cross));
    }

    #[test]
    fn a_profile_tree_is_told_from_a_previous_target_tree() {
        let dir = assert_fs::TempDir::new().unwrap();
        let profile = dir.path().join("x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(profile.join("packages/app")).unwrap();
        std::fs::create_dir_all(profile.join(BUILD_CONTEXT_DIR)).unwrap();
        std::fs::write(profile.join("build.ninja"), "").unwrap();
        assert_eq!(previous_target_tree(&profile), None);

        std::fs::create_dir_all(profile.join("dev")).unwrap();
        std::fs::write(profile.join("dev/build.ninja"), "").unwrap();
        assert_eq!(previous_target_tree(&profile), Some(profile.join("dev")));
        assert_eq!(previous_target_tree(&dir.path().join("missing")), None);
    }
}
