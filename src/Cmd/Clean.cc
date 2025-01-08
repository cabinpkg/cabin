#include "Clean.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Aliases.hpp"

#include <cstdlib>
#include <span>
#include <string>
#include <string_view>

namespace cabin {

static int cleanMain(std::span<const std::string_view> args) noexcept;

const Subcmd CLEAN_CMD =  //
    Subcmd{ "clean" }
        .setDesc("Remove the built directory")
        .addOpt(Opt{ "--profile" }
                    .setShort("-p")
                    .setDesc("Disable parallel builds")
                    .setPlaceholder("<PROFILE>"))
        .setMainFn(cleanMain);

static int
cleanMain(const std::span<const std::string_view> args) noexcept {
  // TODO: share across sources
  fs::path outDir = findManifest().unwrap().parent_path() / "cabin-out";

  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "clean")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "-p" || *itr == "--profile") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingArgumentForOpt(*itr);
      }

      ++itr;
      if (!(*itr == "debug" || *itr == "release")) {
        logger::error("Invalid argument for {}: {}", *(itr - 1), *itr);
        return EXIT_FAILURE;
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
  return EXIT_SUCCESS;
}

}  // namespace cabin
