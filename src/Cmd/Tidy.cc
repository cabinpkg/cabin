#include "Tidy.hpp"

#include "Algos.hpp"
#include "BuildConfig.hpp"
#include "Builder/BuildProfile.hpp"
#include "Cli.hpp"
#include "Command.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Parallelism.hpp"
#include "Rustify/Result.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <string>
#include <string_view>
#include <system_error>

namespace cabin {

static Result<void> tidyMain(CliArgsView args);

const Subcmd TIDY_CMD =
    Subcmd{ "tidy" }
        .setDesc("Run clang-tidy")
        .addOpt(Opt{ "--fix" }.setDesc("Automatically apply lint suggestions"))
        .addOpt(OPT_JOBS)
        .setMainFn(tidyMain);

static Result<void>
tidyImpl(const Command& makeCmd) {
  const auto start = std::chrono::steady_clock::now();

  const ExitStatus exitStatus = Try(execCmd(makeCmd));

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitStatus.success()) {
    Diag::info("Finished", "clang-tidy in {}s", elapsed.count());
    return Ok();
  }
  Bail("clang-tidy {}", exitStatus);
}

static Result<void>
tidyMain(const CliArgsView args) {
  // Parse args
  bool fix = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "tidy"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "--fix") {
      fix = true;
    } else if (arg == "-j" || arg == "--jobs") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      const std::string_view nextArg = *++itr;

      uint64_t numThreads{};
      auto [ptr, ec] = std::from_chars(
          nextArg.data(), nextArg.data() + nextArg.size(), numThreads);
      Ensure(ec == std::errc(), "invalid number of threads: {}", nextArg);
      setParallelism(numThreads);
    } else {
      return TIDY_CMD.noSuchArg(arg);
    }
  }

  Ensure(commandExists("clang-tidy"), "clang-tidy is required");
  if (fix && isParallel()) {
    Diag::warn("`--fix` implies `--jobs 1` to avoid race conditions");
    setParallelism(1);
  }

  const auto manifest = Try(Manifest::tryParse());
  const BuildConfig config =
      Try(emitMakefile(manifest, BuildProfile::Dev, /*includeDevDeps=*/false));

  std::string tidyFlags = "CABIN_TIDY_FLAGS=";
  if (!isVerbose()) {
    tidyFlags += "-quiet";
  }
  if (fs::exists(".clang-tidy")) {
    // clang-tidy will run within the cabin-out/dev directory.
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

  Diag::info("Running", "clang-tidy");
  return tidyImpl(makeCmd);
}

}  // namespace cabin
