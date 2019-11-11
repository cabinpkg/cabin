#ifndef POAC_OPTS_BUILD_HPP
#define POAC_OPTS_BUILD_HPP

#include <future>
#include <string>
#include <vector>
#include <optional>

#include <poac/core/builder.hpp>
#include <poac/core/except.hpp>
#include <poac/io/config.hpp>
#include <poac/util/argparse.hpp>
#include <poac/util/clap/clap.hpp>

namespace poac::opts::build {
    const clap::subcommand cli =
            clap::subcommand("build")
                .about("Compile a project and all sources that depend on its")
                .arg(clap::opt("release", "Build artifacts in release mode, with optimizations"))
                .arg(clap::arg("verbose").long_("verbose").short_("v"))
                .arg(clap::arg("quite").long_("quite").short_("q"))
            ;

    struct Options {
        core::builder::Mode mode;
        bool verbose;
    };

    [[nodiscard]] std::optional<core::except::Error>
    build(std::future<std::optional<io::config::Config>>&& config, build::Options&& opts) {
        // if (const auto error = core::resolver::install_deps()) {
        //    return error;
        // }
        core::Builder bs(config.get(), opts.mode, opts.verbose);
        if (const auto error = bs.build()) {
            return error;
        }
        return std::nullopt;
    }

    [[nodiscard]] std::optional<core::except::Error>
    exec(std::future<std::optional<io::config::Config>>&& config, std::vector<std::string>&& args) {
        if (args.size() > 1) {
            return core::except::Error::InvalidSecondArg::Build;
        }
        build::Options opts{};
        opts.mode = util::argparse::use(args, "--release")
                        ? core::builder::Mode::Release
                        : core::builder::Mode::Debug;
        opts.verbose = util::argparse::use(args, "-v", "--verbose");
        if (util::argparse::use(args, "-q", "--quite")) {
            // Ref: https://stackoverflow.com/a/30185095
            std::cout.setstate(std::ios_base::failbit);
        }
        return build::build(std::move(config), std::move(opts));
    }
} // end namespace
#endif // !POAC_OPTS_BUILD_HPP
