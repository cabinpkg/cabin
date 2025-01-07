#include "Test.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "../Rustify/Aliases.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <span>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

static int testMain(std::span<const std::string_view> args);

const Subcmd TEST_CMD =  //
    Subcmd{ "test" }
        .setShort("t")
        .setDesc("Run the tests of a local package")
        .addOpt(OPT_DEBUG)
        .addOpt(OPT_RELEASE)
        .addOpt(OPT_JOBS)
        .setMainFn(testMain);

static int
testMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isDebug = true;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "test")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "-d" || *itr == "--debug") {
      isDebug = true;
    } else if (*itr == "-r" || *itr == "--release") {
      logger::warn(
          "Tests in release mode could disable assert macros while speeding up "
          "the runtime."
      );
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
      return TEST_CMD.noSuchArg(*itr);
    }
  }

  const auto start = std::chrono::steady_clock::now();

  const auto manifest = Manifest::tryParse().unwrap();
  const BuildConfig config =
      emitMakefile(manifest, isDebug, /*includeDevDeps=*/true);

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
    return EXIT_SUCCESS;
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
    if (execCmd(checkUpToDateCmd) != EXIT_SUCCESS) {
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
      const int curExitCode = execCmd(testCmd);
      if (curExitCode != EXIT_SUCCESS) {
        exitCode = curExitCode;
      }
    }
  }
  if (exitCode != EXIT_SUCCESS) {
    // Compilation failed; don't proceed to run tests.
    return exitCode;
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

    const int curExitCode = execCmd(Command(target));
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
  return exitCode;
}
