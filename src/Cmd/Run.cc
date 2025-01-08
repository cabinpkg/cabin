#include "Run.hpp"

#include "../Algos.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "Build.hpp"
#include "Common.hpp"

#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <span>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static int runMain(std::span<const std::string_view> args);

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

static int
runMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isDebug = true;
  auto itr = args.begin();
  for (; itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "run")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "-d" || *itr == "--debug") {
      isDebug = true;
    } else if (*itr == "-r" || *itr == "--release") {
      isDebug = false;
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
      break;
    }
  }

  std::vector<std::string> runArgs;
  for (; itr != args.end(); ++itr) {
    runArgs.emplace_back(*itr);
  }

  const auto manifest = Manifest::tryParse().unwrap();
  std::string outDir;
  if (buildImpl(manifest, outDir, isDebug) != EXIT_SUCCESS) {
    return EXIT_FAILURE;
  }

  const Command command(outDir + "/" + manifest.package.name, runArgs);
  return execCmd(command);
}

}  // namespace cabin
