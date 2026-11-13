//! pkg-config lookups for the libraries build scripts take from the
//! system.
//!
//! `cabin-system-deps` owns pkg-config executable resolution,
//! subprocess invocation, and flag classification; this module asks
//! it for the modules a script's `pkg-config` lines name.

use anyhow::{Result, bail};

use cabin_core::TargetPlatform;
use cabin_system_deps::{PkgConfigTool, SystemDependencyProbeRequest, probe_system_dependency};

/// The pkg-config that probes libraries for `platform`, checked to run.
fn pkg_config_for(platform: &TargetPlatform) -> Result<PkgConfigTool> {
    let tool = PkgConfigTool::from_env(|key| std::env::var_os(key));
    // A plain pkg-config reports the build machine's libraries, which
    // must not link into a build for another machine.  A triple's `env`
    // counts (a glibc host's `.pc` files do not fit a musl build), and
    // the build machine's `env` is guessed as `gnu` on Linux, so on a
    // musl host an explicit musl `--target` needs one of the overrides.
    if !platform.same_machine(&TargetPlatform::current())
        && std::env::var_os(cabin_env::CABIN_PKG_CONFIG).is_none_or(|value| value.is_empty())
        && std::env::var_os("PKG_CONFIG_LIBDIR").is_none()
    {
        bail!(
            "system libraries of a build for {} need a pkg-config for that platform, not \
             the build machine's: set PKG_CONFIG_LIBDIR (and PKG_CONFIG_SYSROOT_DIR) to the \
             target sysroot's pkg-config directories, or CABIN_PKG_CONFIG to a pkg-config \
             for {}",
            platform.target,
            platform.target,
        );
    }
    // Check availability up-front so the user gets a single
    // actionable diagnostic when pkg-config is missing, regardless of
    // which library is probed first.  Keep the typed error as the
    // anyhow source so the diagnostic registry can render its code and
    // help text, exactly as the probe paths do.
    tool.check_available()?;
    Ok(tool)
}

/// Ask pkg-config for `modules`, which a build script named for a
/// library it supplies, each with its version requirement, and return
/// what they give the library's dependents and links, in their order.
///
/// # Errors
/// As [`pkg_config_for`], and a probe failure, a module at a version
/// outside its requirement among them.
pub(crate) fn probe_reported(
    modules: &[(String, String)],
    platform: &TargetPlatform,
) -> Result<cabin_build::SystemTargetInterface> {
    let tool = pkg_config_for(platform)?;
    let mut interface = cabin_build::SystemTargetInterface::default();
    for (module, requirement) in modules {
        let flags = probe_system_dependency(&SystemDependencyProbeRequest {
            name: module,
            version_requirement: requirement,
            tool: &tool,
        })?
        .flags;
        // `-D` tokens stay among the compile arguments, in pkg-config's
        // order, so a later `-U` still undoes them.
        interface.include_dirs.extend(flags.include_dirs);
        interface
            .system_include_dirs
            .extend(flags.system_include_dirs);
        interface.compile_args.extend(flags.extra_compile_args);
        interface.link_args.extend(flags.ldflags);
    }
    Ok(interface)
}
