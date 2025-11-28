#include "Init.hpp"

#include "Cli.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "New.hpp"

#include <cstdlib>
#include <fstream>
#include <rs/result.hpp>
#include <string>
#include <string_view>

namespace cabin {

static rs::Result<void> initMain(CliArgsView args);

const Subcmd INIT_CMD =
    Subcmd{ "init" }
        .setDesc("Create a new cabin package in an existing directory")
        .addOpt(OPT_BIN)
        .addOpt(OPT_LIB)
        .setMainFn(initMain);

static rs::Result<void> initMain(const CliArgsView args) {
  // Parse args
  bool isBin = true;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = rs_try(Cli::handleGlobalOpts(itr, args.end(), "init"));
    if (control == Cli::Return) {
      return rs::Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (matchesAny(arg, { "-b", "--bin" })) {
      isBin = true;
    } else if (matchesAny(arg, { "-l", "--lib" })) {
      isBin = false;
    } else {
      return INIT_CMD.noSuchArg(arg);
    }
  }

  rs_ensure(!fs::exists("cabin.toml"),
            "cannot initialize an existing cabin package");

  const fs::path root = fs::current_path();
  const std::string packageName = root.stem().string();
  rs_try(validatePackageName(packageName));

  rs_try(createProjectFiles(isBin, root, packageName, /*skipExisting=*/true));
  return rs::Ok();
}

} // namespace cabin
