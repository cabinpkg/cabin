#include "Lint.hpp"

#include "../Algos.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify.hpp"
#include "Global.hpp"

#include <cstdlib>
#include <fstream>
#include <span>

static constexpr auto LINT_CLI = Subcmd<1>("lint").setDesc(lintDesc).addOpt(
    Opt{ "--exclude" }.setDesc("Exclude files from linting")
);

void
lintHelp() noexcept {
  LINT_CLI.printHelp();
}

static int
lint(const StringRef name, const StringRef cpplintArgs) {
  Logger::info("Linting", name);

  String cpplintCmd = "cpplint";
  cpplintCmd += cpplintArgs;
  if (!isVerbose()) {
    cpplintCmd += " --quiet";
  }

  // Read .gitignore if exists
  if (fs::exists(".gitignore")) {
    std::ifstream ifs(".gitignore");
    String line;
    while (std::getline(ifs, line)) {
      if (line.empty() || line[0] == '#') {
        continue;
      }

      cpplintCmd += " --exclude=";
      cpplintCmd += line;
    }
  }
  cpplintCmd += " --recursive ."; // This should be after `--exclude` options

  const int exitCode = runCmd(cpplintCmd);
  if (exitCode != 0) {
    Logger::error("`cpplint` exited with status ", exitCode);
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}

int
lintMain(const std::span<const StringRef> args) {
  // Parse args
  String cpplintArgs;
  for (usize i = 0; i < args.size(); ++i) {
    const StringRef arg = args[i];
    HANDLE_GLOBAL_OPTS({ { "lint" } })

    else if (arg == "--exclude") {
      if (i + 1 >= args.size()) {
        Logger::error("Missing argument for ", arg);
        return EXIT_FAILURE;
      }

      ++i;
      cpplintArgs += " --exclude=";
      cpplintArgs += args[i];
    }
    else {
      return LINT_CLI.noSuchArg(arg);
    }
  }

  if (!commandExists("cpplint")) {
    Logger::error(
        "lint command requires cpplint; try installing it by:\n"
        "  pip install cpplint"
    );
    return EXIT_FAILURE;
  }

  const String& packageName = getPackageName();
  if (fs::exists("CPPLINT.cfg")) {
    Logger::debug("Using CPPLINT.cfg for lint ...");
    return lint(packageName, cpplintArgs);
  }

  if (fs::exists("include")) {
    cpplintArgs += " --root=include";
  } else if (fs::exists("src")) {
    cpplintArgs += " --root=src";
  }

  const Vec<String>& cpplintFilters = getLintCpplintFilters();
  if (!cpplintFilters.empty()) {
    Logger::debug("Using Poac manifest file for lint ...");
    cpplintArgs += " --filter=";
    for (const StringRef filter : cpplintFilters) {
      cpplintArgs += filter;
      cpplintArgs += ',';
    }
    // Remove last comma
    cpplintArgs.pop_back();
    return lint(packageName, cpplintArgs);
  } else {
    Logger::debug("Using default arguments for lint ...");
    if (2011 < editionToYear(getPackageEdition())) {
      cpplintArgs += " --filter=-build/c++11";
    }
    return lint(packageName, cpplintArgs);
  }
}
