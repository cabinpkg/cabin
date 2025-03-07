#include "Build.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Diag.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <string>
#include <string_view>
#include <system_error>

namespace cabin {

static Result<void> buildMain(CliArgsView args);

const Subcmd BUILD_CMD =
    Subcmd{ "build" }
        .setShort("b")
        .setDesc("Compile a local package and all of its dependencies")
        .addOpt(OPT_DEBUG)
        .addOpt(OPT_RELEASE)
        .addOpt(Opt{ "--compdb" }.setDesc(
            "Generate compilation database instead of building"
        ))
        .addOpt(OPT_JOBS)
        .setMainFn(buildMain);

Result<ExitStatus>
runBuildCommand(
    const Manifest& manifest, const std::string& outDir,
    const BuildConfig& config, const std::string& targetName
) {
  const Command makeCmd = getMakeCommand().addArg("-C").addArg(outDir).addArg(
      (config.outBasePath / targetName).string()
  );
  Command checkUpToDateCmd = makeCmd;
  checkUpToDateCmd.addArg("--question");

  ExitStatus exitStatus = Try(execCmd(checkUpToDateCmd));
  if (!exitStatus.success()) {
    // If `targetName` is not up-to-date, compile it.
    Diag::info(
        "Compiling", "{} v{} ({})", targetName,
        manifest.package.version.toString(),
        manifest.path.parent_path().string()
    );
    exitStatus = Try(execCmd(makeCmd));
  }
  return Ok(exitStatus);
}

Result<void>
buildImpl(const Manifest& manifest, std::string& outDir, const bool isDebug) {
  const auto start = std::chrono::steady_clock::now();

  const BuildConfig config =
      Try(emitMakefile(manifest, isDebug, /*includeDevDeps=*/false));
  outDir = config.outBasePath;

  ExitStatus exitStatus;
  if (config.hasBinTarget()) {
    exitStatus =
        Try(runBuildCommand(manifest, outDir, config, manifest.package.name));
  }

  if (config.hasLibTarget() && exitStatus.success()) {
    const std::string& libName = config.getLibName();
    exitStatus = Try(runBuildCommand(manifest, outDir, config, libName));
  }

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitStatus.success()) {
    const Profile& profile = manifest.profiles.at(modeToProfile(isDebug));
    Diag::info(
        "Finished", "`{}` profile [{}] target(s) in {:.2f}s",
        modeToProfile(isDebug), profile, elapsed.count()
    );
  }
  return Ok();
}

static Result<void>
buildMain(const CliArgsView args) {
  // Parse args
  bool isDebug = true;
  bool buildCompdb = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "build"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-d" || arg == "--debug") {
      isDebug = true;
    } else if (arg == "-r" || arg == "--release") {
      isDebug = false;
    } else if (arg == "--compdb") {
      buildCompdb = true;
    } else if (arg == "-j" || arg == "--jobs") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      const std::string_view nextArg = *++itr;

      uint64_t numThreads{};
      auto [ptr, ec] = std::from_chars(
          nextArg.data(), nextArg.data() + nextArg.size(), numThreads
      );
      if (ec == std::errc()) {
        setParallelism(numThreads);
      } else {
        Bail("invalid number of threads: {}", nextArg);
      }
    } else {
      return BUILD_CMD.noSuchArg(arg);
    }
  }

  const auto manifest = Try(Manifest::tryParse());
  if (!buildCompdb) {
    std::string outDir;
    return buildImpl(manifest, outDir, isDebug);
  }

  // Build compilation database
  const std::string outDir =
      Try(emitCompdb(manifest, isDebug, /*includeDevDeps=*/false));
  Diag::info("Generated", "{}/compile_commands.json", outDir);
  return Ok();
}

}  // namespace cabin
