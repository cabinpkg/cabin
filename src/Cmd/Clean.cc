#include "Clean.hpp"

#include "../Logger.hpp"
#include "Global.hpp"

#include <cstdlib>
#include <span>
#include <string>

static constexpr auto cleanCli =
    Subcommand<1>("clean")
        .setDesc(cleanDesc)
        .setUsage("[OPTIONS]")
        .addOpt(Opt{ "--profile", "-p" }
                    .setDesc("Disable parallel builds")
                    .setPlaceholder("<PROFILE>"));

void
cleanHelp() noexcept {
  cleanCli.printHelp();
}

int
cleanMain(const std::span<const StringRef> args) noexcept {
  Path outDir = "poac-out";

  // Parse args
  for (usize i = 0; i < args.size(); ++i) {
    const StringRef arg = args[i];
    HANDLE_GLOBAL_OPTS({ { "clean" } })

    else if (arg == "-p" || arg == "--profile") {
      if (i + 1 >= args.size()) {
        Logger::error("Missing argument for ", arg);
        return EXIT_FAILURE;
      }

      ++i;

      if (!(args[i] == "debug" || args[i] == "release")) {
        Logger::error("Invalid argument for ", arg, ": ", args[i]);
        return EXIT_FAILURE;
      }

      outDir /= args[1];
    }
    else {
      return cleanCli.noSuchArg(arg);
    }
  }

  if (fs::exists(outDir)) {
    Logger::info("Removing", fs::canonical(outDir).string());
    fs::remove_all(outDir);
  }
  return EXIT_SUCCESS;
}
