#include "Test.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "../Rustify/Result.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace cabin {

static Result<void> testMain(CliArgsView args);

const Subcmd TEST_CMD =  //
    Subcmd{ "test" }
        .setShort("t")
        .setDesc("Run the tests of a local package")
        .addOpt(OPT_DEBUG)
        .addOpt(OPT_RELEASE)
        .addOpt(OPT_JOBS)
        .setMainFn(testMain);

static Result<void>
testMain(const CliArgsView args) {
  // Parse args
  bool isDebug = true;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "test"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-d" || arg == "--debug") {
      isDebug = true;
    } else if (arg == "-r" || arg == "--release") {
      logger::warn(
          "Tests in release mode could disable assert macros while speeding up "
          "the runtime."
      );
      isDebug = false;
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
      return TEST_CMD.noSuchArg(arg);
    }
  }

  const auto start = std::chrono::steady_clock::now();

  const auto manifest = Try(Manifest::tryParse());
  const BuildConfig config =
      Try(emitMakefile(manifest, isDebug, /*includeDevDeps=*/true));

  // Collect test targets from the generated Makefile.
  const std::string unittestTargetPrefix =
      (config.outBasePath / "unittests").string() + '/';
  std::vector<std::string> unittestTargets;
  std::ifstream infile(config.outBasePath / "Makefile");
  std::string line;
  while (std::getline(infile, line)) {
    if (!line.starts_with(unittestTargetPrefix)) {
      continue;
    }
    line = line.substr(0, line.find(':'));
    if (!line.ends_with(".test")) {
      continue;
    }
    unittestTargets.push_back(line);
  }

  if (unittestTargets.empty()) {
    logger::warn("No test targets found");
    return Ok();
  }

  const Command baseMakeCmd =
      getMakeCommand().addArg("-C").addArg(config.outBasePath.string());

  // Find not up-to-date test targets, emit compilation status once, and
  // compile them.
  int exitCode{};
  bool alreadyEmitted = false;
  for (const std::string& target : unittestTargets) {
    Command checkUpToDateCmd = baseMakeCmd;
    checkUpToDateCmd.addArg("--question").addArg(target);
    if (Try(execCmd(checkUpToDateCmd)) != EXIT_SUCCESS) {
      // This test target is not up-to-date.
      if (!alreadyEmitted) {
        logger::info(
            "Compiling", "{} v{} ({})", manifest.package.name,
            manifest.package.version.toString(),
            manifest.path.parent_path().string()
        );
        alreadyEmitted = true;
      }

      Command testCmd = baseMakeCmd;
      testCmd.addArg(target);
      const int curExitCode = Try(execCmd(testCmd));
      if (curExitCode != EXIT_SUCCESS) {
        exitCode = curExitCode;
      }
    }
  }
  if (exitCode != EXIT_SUCCESS) {
    // Compilation failed; don't proceed to run tests.
    Bail("compilation failed");
  }

  // Run tests.
  for (const std::string& target : unittestTargets) {
    // `target` always starts with "unittests/" and ends with ".test".
    // We need to replace "unittests/" with "src/" and remove ".test" to get
    // the source file path.
    std::string sourcePath = target;
    sourcePath.replace(0, unittestTargetPrefix.size(), "src/");
    sourcePath.resize(sourcePath.size() - ".test"sv.size());

    const std::string testBinPath =
        fs::relative(target, manifest.path.parent_path()).string();
    logger::info("Running", "unittests {} ({})", sourcePath, testBinPath);

    const int curExitCode = Try(execCmd(Command(target)));
    if (curExitCode != EXIT_SUCCESS) {
      exitCode = curExitCode;
    }
  }

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitCode == EXIT_SUCCESS) {
    logger::info(
        "Finished", "{} test(s) in {}s", modeToString(isDebug), elapsed.count()
    );
  }
  return Ok();
}

}  // namespace cabin
