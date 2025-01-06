#include "Tidy.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Parallelism.hpp"
#include "../Rustify/Aliases.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <span>
#include <string>
#include <string_view>
#include <system_error>

static int tidyMain(std::span<const std::string_view> args);

const Subcmd TIDY_CMD =
    Subcmd{ "tidy" }
        .setDesc("Run clang-tidy")
        .addOpt(Opt{ "--fix" }.setDesc("Automatically apply lint suggestions"))
        .addOpt(OPT_JOBS)
        .setMainFn(tidyMain);

static int
tidyImpl(const Command& makeCmd) {
  const auto start = std::chrono::steady_clock::now();

  const int exitCode = execCmd(makeCmd);

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitCode == EXIT_SUCCESS) {
    logger::info("Finished", "clang-tidy in {}s", elapsed.count());
  }
  return exitCode;
}

static int
tidyMain(const std::span<const std::string_view> args) {
  // Parse args
  bool fix = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "tidy")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "--fix") {
      fix = true;
    } else if (*itr == "-j" || *itr == "--jobs") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingArgumentForOpt(*itr);
      }
      ++itr;

      uint64_t numThreads{};
      auto [ptr, ec] =
          std::from_chars(itr->data(), itr->data() + itr->size(), numThreads);
      if (ec == std::errc()) {
        setParallelism(numThreads);
      } else {
        logger::error("invalid number of threads: {}", *itr);
        return EXIT_FAILURE;
      }
    } else {
      return TIDY_CMD.noSuchArg(*itr);
    }
  }

  if (!commandExists("clang-tidy")) {
    logger::error("clang-tidy not found");
    return EXIT_FAILURE;
  }

  if (fix && isParallel()) {
    logger::warn("`--fix` implies `--jobs 1` to avoid race conditions");
    setParallelism(1);
  }

  const BuildConfig config =
      emitMakefile(/*isDebug=*/true, /*includeDevDeps=*/false);

  std::string tidyFlags = "CABIN_TIDY_FLAGS=";
  if (!isVerbose()) {
    tidyFlags += "-quiet";
  }
  if (fs::exists(".clang-tidy")) {
    // clang-tidy will run within the cabin-out/debug directory.
    tidyFlags += " --config-file=../../.clang-tidy";
  }
  if (fix) {
    tidyFlags += " -fix";
  }

  Command makeCmd(getMakeCommand());
  makeCmd.addArg("-C");
  makeCmd.addArg(config.outBasePath.string());
  makeCmd.addArg(tidyFlags);
  makeCmd.addArg("tidy");
  if (fix) {
    // Keep going to apply fixes to as many files as possible.
    makeCmd.addArg("--keep-going");
  }

  logger::info("Running", "clang-tidy");
  return tidyImpl(makeCmd);
}
