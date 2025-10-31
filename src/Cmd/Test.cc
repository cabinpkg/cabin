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
#include <filesystem>
#include <fmt/format.h>
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <vector>

namespace cabin {

class Test {
  Manifest manifest;
  fs::path outDir;
  std::vector<BuildConfig::TestTarget> collectedTargets;
  bool enableCoverage = false;

  explicit Test(Manifest manifest) : manifest(std::move(manifest)) {}

  Result<void> compileTestTargets();
  Result<void> runTestTargets();

public:
  static Result<void> exec(CliArgsView cliArgs);
};

const Subcmd TEST_CMD = //
    Subcmd{ "test" }
        .setShort("t")
        .setDesc("Run the tests of a local package")
        .addOpt(OPT_JOBS)
        .addOpt(Opt{ "--coverage" }.setDesc("Enable code coverage analysis"))
        .setMainFn(Test::exec);

Result<void> Test::compileTestTargets() {
  const auto start = std::chrono::steady_clock::now();

  const BuildProfile buildProfile = BuildProfile::Test;
  const BuildConfig config = Try(emitNinja(
      manifest, buildProfile, /*includeDevDeps=*/true, enableCoverage));
  outDir = config.outBasePath;
  collectedTargets = config.getTestTargets();

  if (collectedTargets.empty()) {
    Diag::warn("No test targets found");
    return Ok();
  }

  std::vector<std::string> testTargets;
  testTargets.reserve(collectedTargets.size());
  for (const BuildConfig::TestTarget& target : collectedTargets) {
    testTargets.push_back(target.ninjaTarget);
  }

  Command baseCmd = getNinjaCommand();
  baseCmd.addArg("-C").addArg(outDir.string());
  const auto buildTargets = [&](const std::vector<std::string>& targets,
                                std::string_view label) -> Result<void> {
    if (targets.empty()) {
      return Ok();
    }
    if (!Try(ninjaNeedsWork(outDir, targets))) {
      return Ok();
    }

    Diag::info("Compiling", "{} v{} ({})",
               fmt::format("{}({})", manifest.package.name, label),
               manifest.package.version.toString(),
               manifest.path.parent_path().string());

    Command buildCmd(baseCmd);
    for (const std::string& target : targets) {
      buildCmd.addArg(target);
    }

    const ExitStatus exitStatus = Try(execCmd(buildCmd));
    Ensure(exitStatus.success(), "compilation failed");
    return Ok();
  };

  if (config.hasLibTarget()) {
    const std::vector<std::string> libTargets{ config.getLibName() };
    Try(buildTargets(libTargets, "lib"));
  }

  Try(buildTargets(testTargets, "test"));

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  const Profile& profile = manifest.profiles.at(buildProfile);
  Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
             profile, elapsed.count());

  return Ok();
}

Result<void> Test::runTestTargets() {
  const auto start = std::chrono::steady_clock::now();

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
  ExitStatus exitStatus;
  const auto labelFor = [](BuildConfig::TestKind kind) {
    switch (kind) {
    case BuildConfig::TestKind::Integration:
      return std::string_view("integration");
    case BuildConfig::TestKind::Unit:
    default:
      return std::string_view("unit");
    }
  };

  for (const BuildConfig::TestTarget& target : collectedTargets) {
    const fs::path absoluteBinary = outDir / target.ninjaTarget;
    const std::string testBinPath =
        fs::relative(absoluteBinary, manifest.path.parent_path()).string();
    Diag::info("Running", "{} test {} ({})", labelFor(target.kind),
               target.sourcePath, testBinPath);

    const ExitStatus curExitStatus =
        Try(execCmd(Command(absoluteBinary.string())));
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
  const std::string summary =
      fmt::format("{} passed; {} failed; finished in {:.2f}s", numPassed,
                  numFailed, elapsed.count());
  if (!exitStatus.success()) {
    return Err(anyhow::anyhow(summary));
  }
  Diag::info("Ok", "{}", summary);
  return Ok();
}

Result<void> Test::exec(const CliArgsView cliArgs) {
  bool enableCoverage = false;

  for (auto itr = cliArgs.begin(); itr != cliArgs.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, cliArgs.end(), "test"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (matchesAny(arg, { "-j", "--jobs" })) {
      if (itr + 1 == cliArgs.end()) {
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
    } else {
      return TEST_CMD.noSuchArg(arg);
    }
  }

  Manifest manifest = Try(Manifest::tryParse());
  Test cmd(std::move(manifest));
  cmd.enableCoverage = enableCoverage;

  Try(cmd.compileTestTargets());
  if (cmd.collectedTargets.empty()) {
    return Ok();
  }

  Try(cmd.runTestTargets());
  return Ok();
}

} // namespace cabin
