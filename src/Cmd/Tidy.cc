#include "Tidy.hpp"

#include "Algos.hpp"
#include "Builder/BuildProfile.hpp"
#include "Builder/Builder.hpp"
#include "Cli.hpp"
#include "Command.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"

#include <array>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <rs/result.hpp>
#include <string>
#include <string_view>
#include <system_error>

namespace cabin {

namespace fs = std::filesystem;

static Result<void> tidyMain(CliArgsView args);

const Subcmd TIDY_CMD =
    Subcmd{ "tidy" }
        .setDesc("Execute run-clang-tidy")
        .addOpt(Opt{ "--fix" }.setDesc("Automatically apply lint suggestions"))
        .addOpt(OPT_JOBS)
        .setMainFn(tidyMain);

static Result<void> tidyImpl(const Command& makeCmd) {
  const auto start = std::chrono::steady_clock::now();

  const ExitStatus exitStatus = Try(execCmd(makeCmd));

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitStatus.success()) {
    Diag::info("Finished", "run-clang-tidy in {:.2f}s", elapsed.count());
    return Ok();
  }
  Bail("run-clang-tidy {}", exitStatus);
}

static Result<void> tidyMain(const CliArgsView args) {
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
    } else if (matchesAny(arg, { "-j", "--jobs" })) {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      const std::string_view nextArg = *++itr;

      uint64_t numThreads{};
      auto [ptr, ec] =
          std::from_chars(nextArg.begin(), nextArg.end(), numThreads);
      Ensure(ec == std::errc(), "invalid number of threads: {}", nextArg);
      setParallelism(numThreads);
    } else {
      return TIDY_CMD.noSuchArg(arg);
    }
  }

  if (fix && isParallel()) {
    Diag::warn("`--fix` implies `--jobs 1` to avoid race conditions");
    setParallelism(1);
  }

  const auto manifest = Try(Manifest::tryParse());
  const fs::path projectRoot = manifest.path.parent_path();

  // Generate compile_commands for the dev and test profiles so tidy sees both
  // normal and test builds.
  std::string compdbDir;
  const std::array<BuildProfile, 2> profiles{ BuildProfile::Dev,
                                              BuildProfile::Test };
  bool isFirstProfile = true;
  for (const BuildProfile& profile : profiles) {
    Builder builder(projectRoot, profile);
    const bool includeDevDeps = (profile == BuildProfile::Test);
    Try(builder.schedule(ScheduleOptions{
        .includeDevDeps = includeDevDeps,
        .enableCoverage = false,
        .suppressAnalysisLog = !isFirstProfile,
    }));
    compdbDir = builder.compdbRoot();
    isFirstProfile = false;
  }

  std::string runClangTidy = "run-clang-tidy";
  if (const char* tidyEnv = std::getenv("CABIN_TIDY")) {
    runClangTidy = tidyEnv;
  }
  Ensure(commandExists(runClangTidy), "run-clang-tidy is required");

  Command runCmd("");
  if (commandExists("xcrun")) {
    runCmd = Command("xcrun");
    runCmd.addArg(runClangTidy);
  } else {
    runCmd = Command(runClangTidy);
  }
  runCmd.addArg("-p").addArg(compdbDir);
  const fs::path configPath = projectRoot / ".clang-tidy";
  if (fs::exists(configPath)) {
    runCmd.addArg("-config-file=" + configPath.string());
  }
  if (!isVerbose()) {
    runCmd.addArg("-quiet");
  }
  if (fix) {
    runCmd.addArg("-fix");
  }
  const std::size_t jobs = getParallelism();
  if (jobs > 0) {
    runCmd.addArg("-j").addArg(std::to_string(jobs));
  }

  Diag::info("Running", "run-clang-tidy");
  return tidyImpl(runCmd);
}

} // namespace cabin
