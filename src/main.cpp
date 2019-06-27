#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>
#include <variant>

#include <poac/poac.hpp>

template <typename VS>
int handle(std::string&& str, VS&& vs) {
    namespace infer = poac::core::infer;
    namespace except = poac::core::except;
    namespace cli = poac::io::cli;
    using namespace std::string_literals;

    try {
        const auto result = infer::execute(std::forward<std::string>(str), std::forward<VS>(vs));
        if (!result.has_value()) {
            return EXIT_SUCCESS;
        }

        const except::Error err = result.value();
        if (std::holds_alternative<except::Error::InvalidFirstArg>(err.state)) {
            std::cerr << cli::error << "Invalid argument" << std::endl;
            return EXIT_FAILURE;
        }
        else if (std::holds_alternative<except::Error::InvalidSecondArg>(err.state)) {
            const auto e = std::get<except::Error::InvalidSecondArg>(err.state);
            switch (e) {
                case except::Error::InvalidSecondArg::Build:
                    infer::execute("help"s, VS{"build"});
                    break;
                case except::Error::InvalidSecondArg::Cache:
                    infer::execute("help"s, VS{"cache"});
                    break;
                case except::Error::InvalidSecondArg::Cleanup:
                    infer::execute("help"s, VS{"cleanup"});
                    break;
                case except::Error::InvalidSecondArg::Help:
                    infer::execute("help"s, VS{"help"});
                    break;
                case except::Error::InvalidSecondArg::Init:
                    infer::execute("help"s, VS{"init"});
                    break;
                case except::Error::InvalidSecondArg::New:
                    infer::execute("help"s, VS{"new"});
                    break;
                case except::Error::InvalidSecondArg::Publish:
                    infer::execute("help"s, VS{"publish"});
                    break;
                case except::Error::InvalidSecondArg::Search:
                    infer::execute("help"s, VS{"search"});
                    break;
                case except::Error::InvalidSecondArg::Uninstall:
                    infer::execute("help"s, VS{"uninstall"});
                    break;
            }
            return EXIT_FAILURE;
        }
        else if (std::holds_alternative<except::Error::General>(err.state)) {
            const auto e = std::get<except::Error::General>(err.state);
            std::cerr << cli::error << e.what() << std::endl;
            return EXIT_FAILURE;
        }
        return EXIT_FAILURE;
    }
    catch (const except::error& e) {
        std::cerr << cli::error << e.what() << std::endl;
        return EXIT_FAILURE;
    }
    catch (const YAML::BadConversion& e) {
        std::cout << cli::error << "poac.yml " << e.what()
                  << std::endl;
        return EXIT_SUCCESS;
    }
    catch (...) {
        std::cerr << cli::error << "Unexpected error" << std::endl;
        return EXIT_FAILURE;
    }
}

int main(int argc, const char** argv) {
    using namespace std::string_literals;
    // argv[0]: poac, argv[1]: install, argv[2]: 1, ...

    //$ poac install --help => exec("--help", ["install"])
    if (argc == 3 && ((argv[2] == "-h"s) || (argv[2] == "--help"s))) {
        return handle(argv[2], std::vector<std::string>{argv[1]});
    }
    //$ poac install 1 2 3 => exec("install", ["1", "2", "3"])
    else if (argc >= 3) {
        return handle(argv[1], std::vector<std::string>(argv + 2, argv + argc));
    }
    //$ poac install => exec("install", [])
    else if (argc >= 2) {
        return handle(argv[1], std::vector<std::string>{});
    }
    //$ poac => exec("--help", [])
    else {
        handle("help", std::vector<std::string>{});
        return EXIT_FAILURE;
    }
}
