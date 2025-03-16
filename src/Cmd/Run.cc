#include "Run.hpp"

#include "Algos.hpp"
#include "Build.hpp"
#include "Builder/BuildProfile.hpp"
#include "Cli.hpp"
#include "Command.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"
#include "Rustify/Result.hpp"

#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static Result<void> runMain(CliArgsView args);

const Subcmd RUN_CMD = Subcmd{ "run" }
                           .setShort("r")
                           .setDesc("Build and execute src/main.cc")
                           .addOpt(OPT_RELEASE)
                           .addOpt(OPT_JOBS)
                           .setArg(
                               Arg{ "args" }
                                   .setDesc("Arguments passed to the program")
                                   .setVariadic(true)
                                   .setRequired(false)
                           )
                           .setMainFn(runMain);

static Result<void>
runMain(const CliArgsView args) {
  // Parse args
  BuildProfile buildProfile = BuildProfile::Dev;
  auto itr = args.begin();
  for (; itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "run"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-r" || arg == "--release") {
      buildProfile = BuildProfile::Release;
    } else if (arg == "-j" || arg == "--jobs") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      const std::string_view nextArg = *++itr;

      uint64_t numThreads{};
      auto [ptr, ec] = std::from_chars(
          nextArg.data(), nextArg.data() + nextArg.size(), numThreads
      );
      Ensure(ec == std::errc(), "invalid number of threads: {}", nextArg);
      setParallelism(numThreads);
    } else {
      // Unknown argument is the start of the program arguments.
      break;
    }
  }

  std::vector<std::string> runArgs;
  for (; itr != args.end(); ++itr) {
    runArgs.emplace_back(*itr);
  }

  const auto manifest = Try(Manifest::tryParse());
  std::string outDir;
  Try(buildImpl(manifest, outDir, buildProfile));

  Diag::info(
      "Running", "`{}/{}`",
      fs::relative(outDir, manifest.path.parent_path()).string(),
      manifest.package.name
  );
  const Command command(outDir + "/" + manifest.package.name, runArgs);
  const ExitStatus exitStatus = Try(execCmd(command));
  if (exitStatus.success()) {
    return Ok();
  } else {
    Bail("run {}", exitStatus);
  }
}

}  // namespace cabin
