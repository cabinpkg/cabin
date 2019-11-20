#ifndef POAC_OPTS_CACHE_HPP
#define POAC_OPTS_CACHE_HPP

#include <future>
#include <iostream>
#include <stdexcept>
#include <string>
#include <regex>
#include <optional>

#include <boost/range/iterator_range.hpp>

#include <poac/core/except.hpp>
#include <poac/io/filesystem.hpp>
#include <poac/io/term.hpp>
#include <poac/io/config.hpp>
#include <poac/util/argparse.hpp>
#include <poac/util/termcolor2/termcolor2.hpp>

namespace poac::opts::cache {
    inline const clap::subcommand cli =
            clap::subcommand("cache")
                .about("Manipulate cache files")
                .arg(clap::opt("all", "Manipulate all caches").short_("a"))
                .arg(clap::opt("pattern", "Regex pattern").value_name("PATTERN"))
            ;

    struct Options {
        enum class SubCmd {
            Root,
            List,
            Clean,
        };
        SubCmd subcmd;
        std::optional<std::regex> pattern;
        bool all;
        std::vector<std::string> files;
    };

    [[nodiscard]] std::optional<core::except::Error>
    clean(cache::Options&& opts) {
        if (opts.all) {
            io::filesystem::remove_all(io::filesystem::poac_cache_dir);
        } else if (!opts.files.empty()) {
            for (const auto& f : opts.files) {
                const io::filesystem::path cache_package = io::filesystem::poac_cache_dir / f;
                if (io::filesystem::validate_dir(cache_package)) {
                    io::filesystem::remove_all(cache_package);
                    std::cout << cache_package << " is deleted" << std::endl;
                } else {
                    std::cout << termcolor2::red << cache_package << " not found"
                              << termcolor2::reset << std::endl;
                }
            }
        } else {
            return core::except::Error::InvalidSecondArg::Cache;
        }
        return std::nullopt;
    }

    [[nodiscard]] std::optional<core::except::Error>
    list(cache::Options&& opts) {
        if (opts.pattern) {
            for (const auto& e : boost::make_iterator_range(
                    io::filesystem::directory_iterator(io::filesystem::poac_cache_dir), {})
            ) {
                const std::string cache_file = e.path().filename().string();
                if (std::regex_match(cache_file, opts.pattern.value()))
                    std::cout << cache_file << std::endl;
            }
        } else {
            for (const auto& e : boost::make_iterator_range(
                    io::filesystem::directory_iterator(io::filesystem::poac_cache_dir), {})
            ) {
                std::cout << e.path().filename().string() << std::endl;
            }
        }
        return std::nullopt;
    }

    [[nodiscard]] std::optional<core::except::Error>
    root() {
        std::cout << io::filesystem::poac_cache_dir.string() << std::endl;
        return std::nullopt;
    }

    [[nodiscard]] std::optional<core::except::Error>
    cache(cache::Options&& opts) {
        switch (opts.subcmd) {
            case cache::Options::SubCmd::Root:
                return root();
            case cache::Options::SubCmd::List:
                return list(std::move(opts));
            case cache::Options::SubCmd::Clean:
                return clean(std::move(opts));
            default:
                throw std::logic_error(
                        "To access out of range of the "
                        "enumeration values is undefined behavior.");
        }
    }

    [[nodiscard]] std::optional<core::except::Error>
    exec(std::future<std::optional<io::config::Config>>&&, std::vector<std::string>&& args) {
        if (args.empty()) {
            return core::except::Error::InvalidSecondArg::Cache;
        }

        cache::Options opts{};
        if (args[0] == "root" && args.size() == 1) {
            opts.subcmd = cache::Options::SubCmd::Root;
        } else if (args[0] == "list") {
            opts.subcmd = cache::Options::SubCmd::List;
            opts.pattern = util::argparse::use_get(args, "--pattern");
        } else if (args[0] == "clean") {
            opts.subcmd = cache::Options::SubCmd::Clean;
            opts.all = util::argparse::use(args, "-a", "--all");
            opts.files = std::vector<std::string>(args.begin() + 1, args.begin() + args.size());
        } else {
            return core::except::Error::InvalidSecondArg::Cache;
        }
        return cache::cache(std::move(opts));
    }
} // end namespace
#endif // !POAC_OPTS_CACHE_HPP
