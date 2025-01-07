#include "Lint.hpp"

#include "../Algos.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <span>
#include <string>
#include <string_view>
#include <vector>

static int lintMain(std::span<const std::string_view> args);

const Subcmd LINT_CMD = Subcmd{ "lint" }
                            .setDesc("Lint codes using cpplint")
                            .addOpt(Opt{ "--exclude" }
                                        .setDesc("Exclude files from linting")
                                        .setPlaceholder("<FILE>"))
                            .setMainFn(lintMain);

struct LintArgs {
  std::vector<std::string> excludes;
};

static int
lint(const std::string_view name, const std::vector<std::string>& cpplintArgs) {
  logger::info("Linting", "{}", name);

  Command cpplintCmd("cpplint", cpplintArgs);
  if (!isVerbose()) {
    cpplintCmd.addArg("--quiet");
  }

  // Read .gitignore if exists
  if (fs::exists(".gitignore")) {
    std::ifstream ifs(".gitignore");
    std::string line;
    while (std::getline(ifs, line)) {
      if (line.empty() || line[0] == '#') {
        continue;
      }

      cpplintCmd.addArg("--exclude=" + line);
    }
  }
  // NOTE: This should come after the `--exclude` options.
  cpplintCmd.addArg("--recursive");
  cpplintCmd.addArg(".");
  return execCmd(cpplintCmd);
}

static int
lintMain(const std::span<const std::string_view> args) {
  LintArgs lintArgs;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "lint")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "--exclude") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingArgumentForOpt(*itr);
      }

      lintArgs.excludes.push_back("--exclude=" + std::string(*++itr));
    } else {
      return LINT_CMD.noSuchArg(*itr);
    }
  }

  if (!commandExists("cpplint")) {
    logger::error(
        "lint command requires cpplint; try installing it by:\n"
        "  pip install cpplint"
    );
    return EXIT_FAILURE;
  }

  const auto manifest = Manifest::tryParse().unwrap();

  std::vector<std::string> cpplintArgs = lintArgs.excludes;
  if (fs::exists("CPPLINT.cfg")) {
    logger::debug("Using CPPLINT.cfg for lint ...");
    return lint(manifest.package.name, cpplintArgs);
  }

  if (fs::exists("include")) {
    cpplintArgs.emplace_back("--root=include");
  } else if (fs::exists("src")) {
    cpplintArgs.emplace_back("--root=src");
  }

  const std::vector<std::string>& cpplintFilters =
      manifest.lint.cpplint.filters;
  if (!cpplintFilters.empty()) {
    logger::debug("Using Cabin manifest file for lint ...");
    std::string filterArg = "--filter=";
    for (const std::string_view filter : cpplintFilters) {
      filterArg += filter;
      filterArg += ',';
    }
    // Remove last comma
    filterArg.pop_back();
    cpplintArgs.push_back(filterArg);
    return lint(manifest.package.name, cpplintArgs);
  } else {
    logger::debug("Using default arguments for lint ...");
    if (Edition::Cpp11 < manifest.package.edition) {
      // Disable C++11-related lints
      cpplintArgs.emplace_back("--filter=-build/c++11");
    }
    return lint(manifest.package.name, cpplintArgs);
  }
}
