#include "Build.hpp"

#include "Algos.hpp"
#include "Builder/BuildProfile.hpp"
#include "Builder/Builder.hpp"
#include "Cli.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static Result<void> buildMain(CliArgsView args);

const Subcmd BUILD_CMD =
    Subcmd{ "build" }
        .setShort("b")
        .setDesc("Compile a local package and all of its dependencies")
        .addOpt(OPT_RELEASE)
        .addOpt(Opt{ "--compdb" }.setDesc(
            "Generate compilation database instead of building"))
        .addOpt(OPT_JOBS)
        .setMainFn(buildMain);

static Result<void> buildMain(const CliArgsView args) {
  // Parse args
  BuildProfile buildProfile = BuildProfile::Dev;
  bool buildCompdb = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "build"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (matchesAny(arg, { "-r", "--release" })) {
      buildProfile = BuildProfile::Release;
    } else if (arg == "--compdb") {
      buildCompdb = true;
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
      return BUILD_CMD.noSuchArg(arg);
    }
  }

  const Manifest manifest = Try(Manifest::tryParse());
  Builder builder(manifest.path.parent_path(), buildProfile);
  Try(builder.schedule());

  if (buildCompdb) {
    Diag::info("Generated", "{}/compile_commands.json",
               fs::relative(builder.compdbRoot(), manifest.path.parent_path())
                   .string());
    return Ok();
  }

  return builder.build();
}

} // namespace cabin
