#include "Clean.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

#include <cstdlib>
#include <span>
#include <string>
#include <string_view>

namespace cabin {

static Result<void> cleanMain(std::span<const std::string_view> args) noexcept;

const Subcmd CLEAN_CMD =  //
    Subcmd{ "clean" }
        .setDesc("Remove the built directory")
        .addOpt(Opt{ "--profile" }
                    .setShort("-p")
                    .setDesc("Disable parallel builds")
                    .setPlaceholder("<PROFILE>"))
        .setMainFn(cleanMain);

static Result<void>
cleanMain(const std::span<const std::string_view> args) noexcept {
  // TODO: share across sources
  fs::path outDir = Try(findManifest()).parent_path() / "cabin-out";

  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "clean"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (*itr == "-p" || *itr == "--profile") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgument(*itr);
      }

      ++itr;
      if (!(*itr == "debug" || *itr == "release")) {
        Bail("Invalid argument for {}: {}", *(itr - 1), *itr);
      }

      outDir /= *itr;
    } else {
      return CLEAN_CMD.noSuchArg(*itr);
    }
  }

  if (fs::exists(outDir)) {
    logger::info("Removing", "{}", fs::canonical(outDir).string());
    fs::remove_all(outDir);
  }
  return Ok();
}

}  // namespace cabin
