#include "Init.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"
#include "Common.hpp"
#include "New.hpp"

#include <cstdlib>
#include <fstream>
#include <span>
#include <string>
#include <string_view>

namespace cabin {

static Result<void> initMain(std::span<const std::string_view> args);

const Subcmd INIT_CMD =
    Subcmd{ "init" }
        .setDesc("Create a new cabin package in an existing directory")
        .addOpt(OPT_BIN)
        .addOpt(OPT_LIB)
        .setMainFn(initMain);

static Result<void>
initMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isBin = true;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "init"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (*itr == "-b" || *itr == "--bin") {
      isBin = true;
    } else if (*itr == "-l" || *itr == "--lib") {
      isBin = false;
    } else {
      return INIT_CMD.noSuchArg(*itr);
    }
  }

  Ensure(
      !fs::exists("cabin.toml"), "cannot initialize an existing cabin package"
  );

  const std::string packageName = fs::current_path().stem().string();
  Try(validatePackageName(packageName));

  std::ofstream ofs("cabin.toml");
  ofs << createCabinToml(packageName);

  logger::info(
      "Created", "{} `{}` package", isBin ? "binary (application)" : "library",
      packageName
  );
  return Ok();
}

}  // namespace cabin
