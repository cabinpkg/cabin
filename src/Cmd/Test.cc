#include "Test.hpp"

#include "Algos.hpp"
#include "Builder/BuildProfile.hpp"
#include "Builder/Builder.hpp"
#include "Cli.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"
#include "Rustify/Result.hpp"

#include <charconv>
#include <cstdint>
#include <string>
#include <string_view>
#include <system_error>

namespace cabin {

static Result<void> testMain(CliArgsView args);

const Subcmd TEST_CMD = //
    Subcmd{ "test" }
        .setShort("t")
        .setDesc("Run the tests of a local package")
        .addOpt(OPT_JOBS)
        .addOpt(Opt{ "--coverage" }.setDesc("Enable code coverage analysis"))
        .setArg(
            Arg{ "TESTNAME" }.setRequired(false).setDesc("Test name to launch"))
        .setMainFn(testMain);

static Result<void> testMain(const CliArgsView args) {
  bool enableCoverage = false;
  std::optional<std::string> testName;

  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "test"));
    if (control == Cli::Return) {
      return Ok();
    }
    if (control == Cli::Continue) {
      continue;
    }
    if (matchesAny(arg, { "-j", "--jobs" })) {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      const std::string_view nextArg = *++itr;

      uint64_t numThreads{};
      auto [ptr, ec] =
          std::from_chars(nextArg.begin(), nextArg.end(), numThreads);
      Ensure(ec == std::errc(), "invalid number of threads: {}", nextArg);
      setParallelism(numThreads);
    } else if (arg == "--coverage") {
      enableCoverage = true;
    } else if (!testName) {
      testName = arg;
    } else {
      return TEST_CMD.noSuchArg(arg);
    }
  }

  const Manifest manifest = Try(Manifest::tryParse());
  Builder builder(manifest.path.parent_path(), BuildProfile::Test);
  Try(builder.schedule(ScheduleOptions{ .includeDevDeps = true,
                                        .enableCoverage = enableCoverage }));
  return builder.test(testName);
}

} // namespace cabin
