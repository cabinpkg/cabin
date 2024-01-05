#include "Init.hpp"

#include "../Logger.hpp"
#include "Global.hpp"
#include "New.hpp"

#include <cstdlib>
#include <fstream>
#include <iostream>
#include <span>
#include <string>

int initMain(const std::span<const StringRef> args) {
  // Parse args
  bool isBin = true;
  for (usize i = 0; i < args.size(); ++i) {
    const StringRef arg = args[i];
    HANDLE_GLOBAL_OPTS({ { "init" } })

    else if (arg == "-b" || arg == "--bin") {
      isBin = true;
    }
    else if (arg == "-l" || arg == "--lib") {
      isBin = false;
    }
    else {
      Logger::error("invalid argument: ", arg);
      return EXIT_FAILURE;
    }
  }

  if (fs::exists("poac.toml")) {
    Logger::error("cannot initialize an existing poac package");
    return EXIT_FAILURE;
  }

  const String packageName = fs::current_path().stem().string();
  if (!verifyPackageName(packageName)) {
    return EXIT_FAILURE;
  }

  std::ofstream ofs("poac.toml");
  ofs << getPoacToml(packageName);

  Logger::info(
      "Created", isBin ? "binary (application) `" : "library `", packageName,
      "` package"
  );
  return EXIT_SUCCESS;
}

void initHelp() noexcept {
  std::cout << initDesc << '\n';
  std::cout << '\n';
  printUsage("init", "[OPTIONS]");
  std::cout << '\n';
  printHeader("Options:");
  printGlobalOpts();
  printOption("--bin", "-b", "Use a binary (application) template [default]");
  printOption("--lib", "-l", "Use a library template");
}
