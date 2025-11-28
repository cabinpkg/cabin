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

static rs::Result<void> runMain(CliArgsView args);

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

static rs::Result<void> runMain(const CliArgsView args) {
  // Parse args
  BuildProfile buildProfile = BuildProfile::Dev;
  auto itr = args.begin();
  for (; itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = rs_try(Cli::handleGlobalOpts(itr, args.end(), "run"));
    if (control == Cli::Return) {
      return rs::Ok();
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
      rs_ensure(ec == std::errc(), "invalid number of threads: {}", nextArg);
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

  const auto manifest = rs_try(Manifest::tryParse());
  Builder builder(manifest.path.parent_path(), buildProfile);
  rs_try(builder.schedule());
  rs_try(builder.build());

  Diag::info(
      "Running", "`{}/{}`",
      fs::relative(builder.outDirPath(), manifest.path.parent_path()).string(),
      manifest.package.name);
  const Command command((builder.outDirPath() / manifest.package.name).string(),
                        runArgs);
  const ExitStatus exitStatus = rs_try(execCmd(command));
  if (exitStatus.success()) {
    return rs::Ok();
  } else {
    rs_bail("run {}", exitStatus);
  }
}

} // namespace cabin
