#include "Clean.hpp"

#include "Cli.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"

#include <cstdlib>
#include <rs/result.hpp>
#include <string>
#include <string_view>

namespace cabin {

static rs::Result<void> cleanMain(CliArgsView args) noexcept;

const Subcmd CLEAN_CMD = //
    Subcmd{ "clean" }
        .setDesc("Remove the built directory")
        .addOpt(Opt{ "--profile" }
                    .setShort("-p")
                    .setDesc("Clean artifacts of the specified profile")
                    .setPlaceholder("<PROFILE>"))
        .setMainFn(cleanMain);

static rs::Result<void> cleanMain(CliArgsView args) noexcept {
  // TODO: share across sources
  fs::path outDir = rs_try(Manifest::findPath()).parent_path() / "cabin-out";

  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control =
        rs_try(Cli::handleGlobalOpts(itr, args.end(), "clean"));
    if (control == Cli::Return) {
      return rs::Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (matchesAny(arg, { "-p", "--profile" })) {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }

      const std::string_view nextArg = *++itr;
      if (!matchesAny(nextArg, { "dev", "release" })) {
        rs_bail("Invalid argument for {}: {}", arg, nextArg);
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
  return rs::Ok();
}

} // namespace cabin
