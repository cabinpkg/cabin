#include "Run.hpp"

#include "../Algos.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "../Rustify/Result.hpp"
#include "Build.hpp"
#include "Common.hpp"

#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <span>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static Result<void> runMain(std::span<const std::string_view> args);

const Subcmd RUN_CMD =
    Subcmd{ "run" }
        .setShort("r")
        .setDesc("Build and execute src/main.cc")
        .addOpt(OPT_DEBUG)
        .addOpt(OPT_RELEASE)
        .addOpt(OPT_JOBS)
        .setArg(Arg{ "args" }
                    .setDesc("Arguments passed to the program")
                    .setVariadic(true)
                    .setRequired(false))
        .setMainFn(runMain);

static Result<void>
runMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isDebug = true;
  auto itr = args.begin();
  for (; itr != args.end(); ++itr) {
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "run"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (*itr == "-d" || *itr == "--debug") {
      isDebug = true;
    } else if (*itr == "-r" || *itr == "--release") {
      isDebug = false;
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
      break;
    }
  }

  std::vector<std::string> runArgs;
  for (; itr != args.end(); ++itr) {
    runArgs.emplace_back(*itr);
  }

  const auto manifest = Try(Manifest::tryParse());
  std::string outDir;
  Try(buildImpl(manifest, outDir, isDebug));

  logger::info(
      "Running", "`{}/{}`",
      fs::relative(outDir, manifest.path.parent_path()).string(),
      manifest.package.name
  );
  const Command command(outDir + "/" + manifest.package.name, runArgs);
  const int exitCode = Try(execCmd(command));
  if (exitCode == EXIT_SUCCESS) {
    return Ok();
  } else {
    Bail("run failed with exit code `{}`", exitCode);
  }
}

}  // namespace cabin
