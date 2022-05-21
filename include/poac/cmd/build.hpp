#ifndef POAC_CMD_BUILD_HPP
#define POAC_CMD_BUILD_HPP

// external
#include <spdlog/spdlog.h>
#include <spdlog/stopwatch.h>
#include <structopt/app.hpp>
#include <toml.hpp>

// internal
#include <poac/poac.hpp>
#include <poac/core/builder/ninja/build.hpp>
#include <poac/core/resolver.hpp>
#include <poac/core/validator.hpp>
#include <poac/data/manifest.hpp>
#include <poac/util/pretty.hpp>
#include <poac/config.hpp>

namespace poac::cmd::build {
    using core::builder::ninja::build::Mode;
    using core::resolver::ResolvedDeps;

    struct Options: structopt::sub_command {
        /// Build artifacts in release mode, with optimizations
        Option<bool> release = false;
    };

    using FailedToBuild =
        Error<"failed to build package `{}`", String>;
    using FailedToInstallDeps =
        Error<"failed to install dependencies">;

    [[nodiscard]] Result<fs::path>
    build_impl(const toml::value& manifest, const Mode& mode, const ResolvedDeps& resolved_deps) {
        spdlog::stopwatch sw;
        const fs::path output_path = Try(
            core::builder::ninja::build::start(manifest, mode, resolved_deps)
        );

        spdlog::info(
            "{:>25} {} target(s) in {}",
            "Finished"_bold_green,
            to_string(mode),
            util::pretty::to_time(sw.elapsed().count())
        );
        return Ok(output_path);
    }

    [[nodiscard]] Result<Option<fs::path>>
    build(const Options& opts, const toml::value& manifest) {
        spdlog::trace("Resolving dependencies ...");
        const auto resolved_deps =
            Try(
            core::resolver::install_deps(manifest)
                .with_context([]{
                    return Err<FailedToInstallDeps>().get();
                })
        );

        // TODO: We have to keep in mind a case of only dependencies require to
        // be built, but this package does not.
        if (!fs::exists(config::path::main_cpp_file)) {
            spdlog::info(
                "{:>25} no build target(s) found",
                "Finished"_bold_green
            );
            return Ok(None);
        }

        const Mode mode = opts.release.value() ? Mode::release : Mode::debug;
        const fs::path output_path =
            Try(
            build_impl(manifest, mode, resolved_deps)
        );
        return Ok(output_path);
    }

    [[nodiscard]] Result<void>
    exec(const Options& opts) {
        spdlog::trace("Checking if required config exists ...");
        Try(core::validator::required_config_exists().map_err(to_anyhow));

        spdlog::trace("Parsing the manifest file ...");
        // TODO: parse as a static type rather than toml::value
        const toml::value manifest = toml::parse(data::manifest::name);

        Try(
            build(opts, manifest).with_context([&manifest]{
                return Err<FailedToBuild>(
                    toml::find<String>(manifest, "package", "name")
                ).get();
            })
        );
        return Ok();
    }
} // end namespace

STRUCTOPT(poac::cmd::build::Options, release);

#endif // !POAC_CMD_BUILD_HPP
