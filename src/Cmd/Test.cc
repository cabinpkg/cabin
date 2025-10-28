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
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <vector>

namespace cabin {

class Test {
  Manifest manifest;
  fs::path outDir;
  std::vector<std::string> unittestTargets;
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
  unittestTargets = config.getTestTargets();

  if (unittestTargets.empty()) {
    Diag::warn("No test targets found");
    return Ok();
  }

  Command baseCmd = getNinjaCommand();
  baseCmd.addArg("-C").addArg(outDir.string());
  const bool needsBuild = Try(ninjaNeedsWork(outDir, unittestTargets));

  if (needsBuild) {
    Diag::info("Compiling", "{} v{} ({})", manifest.package.name,
               manifest.package.version.toString(),
               manifest.path.parent_path().string());

    Command buildCmd(baseCmd);
    for (const std::string& target : unittestTargets) {
      buildCmd.addArg(target);
    }

    const ExitStatus exitStatus = Try(execCmd(buildCmd));
    Ensure(exitStatus.success(), "compilation failed");
  }

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  const Profile& profile = manifest.profiles.at(buildProfile);
  Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
             profile, elapsed.count());

  return Ok();
}

Result<void> Test::runTestTargets() {
  using std::string_view_literals::operator""sv;

  const auto start = std::chrono::steady_clock::now();

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
  ExitStatus exitStatus;
  for (const std::string& target : unittestTargets) {
    static constexpr std::string_view unitPrefix = "unittests/";
    std::string sourcePath;
    if (target.starts_with(unitPrefix)) {
      sourcePath = target.substr(unitPrefix.size());
    } else {
      sourcePath = target;
    }
    if (sourcePath.ends_with(".test"sv)) {
      sourcePath.resize(sourcePath.size() - ".test"sv.size());
    }
    sourcePath.insert(0, "src/");

    const fs::path absoluteBinary = outDir / target;
    const std::string testBinPath =
        fs::relative(absoluteBinary, manifest.path.parent_path()).string();
    Diag::info("Running", "unittests {} ({})", sourcePath, testBinPath);

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
  if (cmd.unittestTargets.empty()) {
    return Ok();
  }

  Try(cmd.runTestTargets());
  return Ok();
}

} // namespace cabin
