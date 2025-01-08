#include "Init.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "Common.hpp"
#include "New.hpp"

#include <cstdlib>
#include <fstream>
#include <span>
#include <string>
#include <string_view>

namespace cabin {

static int initMain(std::span<const std::string_view> args);

const Subcmd INIT_CMD =
    Subcmd{ "init" }
        .setDesc("Create a new cabin package in an existing directory")
        .addOpt(OPT_BIN)
        .addOpt(OPT_LIB)
        .setMainFn(initMain);

static int
initMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isBin = true;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "init")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "-b" || *itr == "--bin") {
      isBin = true;
    } else if (*itr == "-l" || *itr == "--lib") {
      isBin = false;
    } else {
      return INIT_CMD.noSuchArg(*itr);
    }
  }

  if (fs::exists("cabin.toml")) {
    logger::error("cannot initialize an existing cabin package");
    return EXIT_FAILURE;
  }

  const std::string packageName = fs::current_path().stem().string();
  if (const auto err = validatePackageName(packageName)) {
    logger::error("package names {}: `{}`", err.value(), packageName);
    return EXIT_FAILURE;
  }

  std::ofstream ofs("cabin.toml");
  ofs << createCabinToml(packageName);

  logger::info(
      "Created", "{} `{}` package", isBin ? "binary (application)" : "library",
      packageName
  );
  return EXIT_SUCCESS;
}

}  // namespace cabin
