#include "Test.hpp"

#include "Algos.hpp"
#include "BuildConfig.hpp"
#include "Builder/BuildProfile.hpp"
#include "Cli.hpp"
#include "Command.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"
#include "Rustify/Result.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fstream>
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <vector>

namespace cabin {

class Test {
  Manifest manifest;
  std::string unittestTargetPrefix;
  std::vector<std::string> unittestTargets;

  explicit Test(Manifest manifest) : manifest(std::move(manifest)) {}

  static Result<void> parseArgs(CliArgsView cliArgs);
  Result<void> compileTestTargets();
  Result<void> runTestTargets();

public:
  static Result<void> exec(CliArgsView cliArgs);
};

const Subcmd TEST_CMD =  //
    Subcmd{ "test" }
        .setShort("t")
        .setDesc("Run the tests of a local package")
        .addOpt(OPT_JOBS)
        .setMainFn(Test::exec);

Result<void>
Test::parseArgs(const CliArgsView cliArgs) {
  for (auto itr = cliArgs.begin(); itr != cliArgs.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, cliArgs.end(), "test"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-j" || arg == "--jobs") {
      if (itr + 1 == cliArgs.end()) {
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

  return Ok();
}

Result<void>
Test::compileTestTargets() {
  const auto start = std::chrono::steady_clock::now();

  const BuildProfile buildProfile = BuildProfile::Test;
  const BuildConfig config =
      Try(emitMakefile(manifest, buildProfile, /*includeDevDeps=*/true));

  // Collect test targets from the generated Makefile.
  unittestTargetPrefix = (config.outBasePath / "unittests").string() + '/';
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
    Diag::warn("No test targets found");
    return Ok();
  }

  const Command baseMakeCmd =
      getMakeCommand().addArg("-C").addArg(config.outBasePath.string());

  // Find not up-to-date test targets, emit compilation status once, and
  // compile them.
  ExitStatus exitStatus;
  bool alreadyEmitted = false;
  for (const std::string& target : unittestTargets) {
    Command checkUpToDateCmd = baseMakeCmd;
    checkUpToDateCmd.addArg("--question").addArg(target);
    if (!Try(execCmd(checkUpToDateCmd)).success()) {
      // This test target is not up-to-date.
      if (!alreadyEmitted) {
        Diag::info(
            "Compiling", "{} v{} ({})", manifest.package.name,
            manifest.package.version.toString(),
            manifest.path.parent_path().string()
        );
        alreadyEmitted = true;
      }

      Command testCmd = baseMakeCmd;
      testCmd.addArg(target);
      const ExitStatus curExitStatus = Try(execCmd(testCmd));
      if (!curExitStatus.success()) {
        exitStatus = curExitStatus;
      }
    }
  }
  // If the compilation failed, don't proceed to run tests.
  Ensure(exitStatus.success(), "compilation failed");

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  const Profile& profile = manifest.profiles.at(buildProfile);
  Diag::info(
      "Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
      profile, elapsed.count()
  );

  return Ok();
}

Result<void>
Test::runTestTargets() {
  using std::string_view_literals::operator""sv;

  const auto start = std::chrono::steady_clock::now();

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
  ExitStatus exitStatus;
  for (const std::string& target : unittestTargets) {
    // `target` always starts with "unittests/" and ends with ".test".
    // We need to replace "unittests/" with "src/" and remove ".test" to get
    // the source file path.
    std::string sourcePath = target;
    sourcePath.replace(0, unittestTargetPrefix.size(), "src/");
    sourcePath.resize(sourcePath.size() - ".test"sv.size());

    const std::string testBinPath =
        fs::relative(target, manifest.path.parent_path()).string();
    Diag::info("Running", "unittests {} ({})", sourcePath, testBinPath);

    const ExitStatus curExitStatus = Try(execCmd(Command(target)));
    if (curExitStatus.success()) {
      ++numPassed;
    } else {
      ++numFailed;
      exitStatus = curExitStatus;
    }
  }

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  // TODO: collect stdout/err's of failed tests and print them here.
  const std::string summary = fmt::format(
      "{} passed; {} failed; finished in {:.2f}s", numPassed, numFailed,
      elapsed.count()
  );
  if (!exitStatus.success()) {
    return Err(anyhow::anyhow(summary));
  }
  Diag::info("Ok", "{}", summary);
  return Ok();
}

Result<void>
Test::exec(const CliArgsView cliArgs) {
  Try(parseArgs(cliArgs));
  Manifest manifest = Try(Manifest::tryParse());
  Test cmd(std::move(manifest));

  Try(cmd.compileTestTargets());
  if (cmd.unittestTargets.empty()) {
    return Ok();
  }

  Try(cmd.runTestTargets());
  return Ok();
}

}  // namespace cabin
