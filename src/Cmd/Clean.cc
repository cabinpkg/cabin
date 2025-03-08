#include "Clean.hpp"

#include "Cli.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <cstdlib>
#include <string>
#include <string_view>

namespace cabin {

static Result<void> cleanMain(CliArgsView args) noexcept;

const Subcmd CLEAN_CMD =  //
    Subcmd{ "clean" }
        .setDesc("Remove the built directory")
        .addOpt(
            Opt{ "--profile" }
                .setShort("-p")
                .setDesc("Disable parallel builds")
                .setPlaceholder("<PROFILE>")
        )
        .setMainFn(cleanMain);

static Result<void>
cleanMain(CliArgsView args) noexcept {
  // TODO: share across sources
  fs::path outDir = Try(Manifest::findPath()).parent_path() / "cabin-out";

  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "clean"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-p" || arg == "--profile") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }

      const std::string_view nextArg = *++itr;
      if (!(nextArg == "dev" || nextArg == "release")) {
        Bail("Invalid argument for {}: {}", arg, nextArg);
      }

      outDir /= nextArg;
    } else {
      return CLEAN_CMD.noSuchArg(arg);
    }
  }

  if (fs::exists(outDir)) {
    Diag::info("Removing", "{}", fs::canonical(outDir).string());
    fs::remove_all(outDir);
  }
  return Ok();
}

}  // namespace cabin
