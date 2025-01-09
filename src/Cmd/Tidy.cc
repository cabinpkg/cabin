#include "Tidy.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Parallelism.hpp"
#include "../Rustify/Result.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <span>
#include <string>
#include <string_view>
#include <system_error>

namespace cabin {

static Result<void> tidyMain(std::span<const std::string_view> args);

const Subcmd TIDY_CMD =
    Subcmd{ "tidy" }
        .setDesc("Run clang-tidy")
        .addOpt(Opt{ "--fix" }.setDesc("Automatically apply lint suggestions"))
        .addOpt(OPT_JOBS)
        .setMainFn(tidyMain);

static Result<void>
tidyImpl(const Command& makeCmd) {
  const auto start = std::chrono::steady_clock::now();

  const int exitCode = execCmd(makeCmd);

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitCode == EXIT_SUCCESS) {
    logger::info("Finished", "clang-tidy in {}s", elapsed.count());
    return Ok();
  }
  Bail("clang-tidy failed with exit code `{}`", exitCode);
}

static Result<void>
tidyMain(const std::span<const std::string_view> args) {
  // Parse args
  bool fix = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "tidy"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (*itr == "--fix") {
      fix = true;
    } else if (*itr == "-j" || *itr == "--jobs") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgument(*itr);
      }
      ++itr;

      uint64_t numThreads{};
      auto [ptr, ec] =
          std::from_chars(itr->data(), itr->data() + itr->size(), numThreads);
      if (ec == std::errc()) {
        setParallelism(numThreads);
      } else {
        Bail("invalid number of threads: {}", *itr);
      }
    } else {
      return TIDY_CMD.noSuchArg(*itr);
    }
  }

  if (!commandExists("clang-tidy")) {
    Bail("clang-tidy not found");
  }

  if (fix && isParallel()) {
    logger::warn("`--fix` implies `--jobs 1` to avoid race conditions");
    setParallelism(1);
  }

  const auto manifest = Try(Manifest::tryParse());
  const BuildConfig config =
      emitMakefile(manifest, /*isDebug=*/true, /*includeDevDeps=*/false);

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

}  // namespace cabin
