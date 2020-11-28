#ifndef POAC_OPTS_INIT_HPP
#define POAC_OPTS_INIT_HPP

// Std
#include <future>
#include <iostream>
#include <fstream>
#include <string>
#include <string_view>
#include <optional>

// Internal
#include <bin/poac/commands/new.hpp>
#include <poac/io/path.hpp>
#include <poac/io/term.hpp>
#include <poac/io/config.hpp>
#include <poac/core/except.hpp>
#include <poac/core/name.hpp>
#include <poac/util/clap/clap.hpp>
#include <poac/util/termcolor2/termcolor2.hpp>

namespace bin::poac::commands::init {
    inline const clap::subcommand cli =
            clap::subcommand("init")
                .about("Create a new poac package in an existing directory")
                .arg(clap::opt("bin", "Use a binary (application) template [default]"))
                .arg(clap::opt("lib", "Use a library template"))
            ;

    struct Options {
        _new::ProjectType type;
    };

    [[nodiscard]] std::optional<::poac::core::except::Error>
    init(init::Options&& opts) {
        using termcolor2::color_literals::operator""_green;

        if (const auto error = ::poac::core::name::validate_package_name(::poac::io::path::current.stem().string())) {
            return error;
        }

        std::cout << "Created: "_green;
        std::ofstream ofs_config("poac.toml");
        switch (opts.type) {
            case _new::ProjectType::Bin:
                ofs_config << _new::files::bin::poac_toml(::poac::io::path::current.stem().string());
                break;
            case _new::ProjectType::Lib:
                ofs_config << _new::files::lib::poac_toml;
                break;
        }
        std::cout << opts.type << " package" << std::endl;
        return std::nullopt;
    }

    [[nodiscard]] std::optional<::poac::core::except::Error>
    exec(std::future<std::optional<::poac::io::config::Config>>&&, std::vector<std::string>&& args) {
        if (args.size() > 1) {
            return ::poac::core::except::Error::InvalidSecondArg::Init;
        } else if (::poac::io::config::detail::validate_config()) {
            return ::poac::core::except::Error::General{
                "`poac init` cannot be run on existing poac packages"
            };
        }

        init::Options opts{};
        const bool bin = ::poac::util::argparse::use_rm(args, "-b", "--bin");
        const bool lib = ::poac::util::argparse::use_rm(args, "-l", "--lib");
        if (bin && lib) {
            return ::poac::core::except::Error::General{
                "You cannot specify both lib and binary outputs."
            };
        } else if (!bin && lib) {
            opts.type = _new::ProjectType::Lib;
        } else {
            opts.type = _new::ProjectType::Bin;
        }
        return init::init(std::move(opts));
    }
} // end namespace
#endif // !POAC_OPTS_INIT_HPP
