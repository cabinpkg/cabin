#include "Run.hpp"

#include "Algos.hpp"
#include "Builder/BuildProfile.hpp"
#include "Builder/Builder.hpp"
#include "Cli.hpp"
#include "Command.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"

#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <rs/result.hpp>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static Result<void> runMain(CliArgsView args);

const Subcmd RUN_CMD =
    Subcmd{ "run" }
        .setShort("r")
        .setDesc("Build and execute src/main.cc")
        .addOpt(OPT_RELEASE)
        .addOpt(OPT_JOBS)
        .setArg(Arg{ "args" }
                    .setDesc("Arguments passed to the program")
                    .setVariadic(true)
                    .setRequired(false))
        .setMainFn(runMain);

static Result<void> runMain(const CliArgsView args) {
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
    } else if (matchesAny(arg, { "-r", "--release" })) {
      buildProfile = BuildProfile::Release;
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
      // Unknown argument is the start of the program arguments.
      break;
    }
  }

  std::vector<std::string> runArgs;
  for (; itr != args.end(); ++itr) {
    runArgs.emplace_back(*itr);
  }

  const auto manifest = Try(Manifest::tryParse());
  Builder builder(manifest.path.parent_path(), buildProfile);
  Try(builder.schedule());
  Try(builder.build());

  Diag::info(
      "Running", "`{}/{}`",
      fs::relative(builder.outDirPath(), manifest.path.parent_path()).string(),
      manifest.package.name);
  const Command command((builder.outDirPath() / manifest.package.name).string(),
                        runArgs);
  const ExitStatus exitStatus = Try(execCmd(command));
  if (exitStatus.success()) {
    return Ok();
  } else {
    Bail("run {}", exitStatus);
  }
}

} // namespace cabin
