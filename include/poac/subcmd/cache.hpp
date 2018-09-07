#ifndef POAC_SUBCMD_CACHE_HPP
#define POAC_SUBCMD_CACHE_HPP

#include <iostream>
#include <string>
#include <regex>

#include <boost/filesystem.hpp>
#include <boost/range/iterator_range.hpp>

#include "../core/exception.hpp"
#include "../io/file.hpp"
#include "../io/cli.hpp"


namespace poac::subcmd { struct cache {
        static const std::string summary() { return "Manipulate cache files."; }
        static const std::string options() { return "<command>"; }

        template <typename VS, typename = std::enable_if_t<std::is_rvalue_reference_v<VS&&>>>
        void operator()(VS&& vs) { _main(std::move(vs)); }
        template <typename VS, typename = std::enable_if_t<std::is_rvalue_reference_v<VS&&>>>
        void _main(VS&& argv) {
            namespace except = core::exception;

            check_arguments(argv);
            if (argv[0] == "root" && argv.size() == 1)
                root();
            else if (argv[0] == "list")
                list(std::vector<std::string>(argv.begin()+1, argv.begin()+argv.size()));
            else if (argv[0] == "clean")
                clean(std::vector<std::string>(argv.begin()+1, argv.begin()+argv.size()));
            else
                throw except::invalid_second_arg("cache");
        }

        // TODO: --all, -a optionが無いとわかりづらい
        void clean(const std::vector<std::string>& argv) {
            namespace fs = boost::filesystem;
            if (argv.empty()) {
                fs::remove_all(io::file::path::poac_cache_dir);
            }
            else {
                for (const auto& v : argv) {
                    const fs::path pkg = io::file::path::poac_cache_dir / v;
                    if (io::file::path::validate_dir(pkg))
                        fs::remove_all(pkg);
                    else
                        std::cout << io::cli::red << v << " not found" << io::cli::reset << std::endl;
                }
            }
        }

        void list(const std::vector<std::string>& argv) {
            namespace fs = boost::filesystem;
            if (argv.empty()) {
                for (const auto& e : boost::make_iterator_range(fs::directory_iterator(io::file::path::poac_cache_dir), {})) {
                    std::cout << e.path().filename().string() << std::endl;
                }
            }
            else if (argv.size() == 2 && argv[0] == "--pattern") {
                std::regex pattern(argv[1]);
                for (const auto& e : boost::make_iterator_range(fs::directory_iterator(io::file::path::poac_cache_dir), {})) {
                    const std::string cachefile = e.path().filename().string();
                    if (std::regex_match(cachefile, pattern))
                        std::cout << cachefile << std::endl;
                }
            }
        }

        void root() {
            std::cout << io::file::path::poac_cache_dir.string() << std::endl;
        }

        void check_arguments(const std::vector<std::string>& argv) {
            namespace except = core::exception;
            if (argv.empty()) throw except::invalid_second_arg("cache");
        }
    };} // end namespace
#endif // !POAC_SUBCMD_CACHE_HPP
