#include "Build.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Command.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Parallelism.hpp"
#include "Common.hpp"

#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <span>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

static int buildMain(std::span<const std::string_view> args);

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

int
runBuildCommand(
    const Manifest& manifest, const std::string& outDir,
    const BuildConfig& config, const std::string& targetName
) {
  const Command makeCmd = getMakeCommand().addArg("-C").addArg(outDir).addArg(
      (config.outBasePath / targetName).string()
  );
  Command checkUpToDateCmd = makeCmd;
  checkUpToDateCmd.addArg("--question");

  int exitCode = execCmd(checkUpToDateCmd);
  if (exitCode != EXIT_SUCCESS) {
    // If `targetName` is not up-to-date, compile it.
    logger::info(
        "Compiling", "{} v{} ({})", targetName,
        manifest.package.version.toString(),
        manifest.path.parent_path().string()
    );
    exitCode = execCmd(makeCmd);
  }
  return exitCode;
}

int
buildImpl(const Manifest& manifest, std::string& outDir, const bool isDebug) {
  const auto start = std::chrono::steady_clock::now();

  const BuildConfig config =
      emitMakefile(manifest, isDebug, /*includeDevDeps=*/false);
  outDir = config.outBasePath;

  int exitCode = 0;
  if (config.hasBinTarget()) {
    exitCode = runBuildCommand(manifest, outDir, config, manifest.package.name);
  }

  if (config.hasLibTarget() && exitCode == 0) {
    const std::string& libName = config.getLibName();
    exitCode = runBuildCommand(manifest, outDir, config, libName);
  }

  const auto end = std::chrono::steady_clock::now();
  const std::chrono::duration<double> elapsed = end - start;

  if (exitCode == EXIT_SUCCESS) {
    const Profile& profile =
        isDebug ? manifest.profiles.at("dev") : manifest.profiles.at("release");

    std::vector<std::string_view> profiles;
    if (profile.optLevel == 0) {
      profiles.emplace_back("unoptimized");
    } else {
      profiles.emplace_back("optimized");
    }
    if (profile.debug) {
      profiles.emplace_back("debuginfo");
    }

    logger::info(
        "Finished", "`{}` profile [{}] target(s) in {:.2f}s",
        modeToProfile(isDebug), fmt::join(profiles, " + "), elapsed.count()
    );
  }
  return exitCode;
}

static int
buildMain(const std::span<const std::string_view> args) {
  // Parse args
  bool isDebug = true;
  bool buildCompdb = false;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "build")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "-d" || *itr == "--debug") {
      isDebug = true;
    } else if (*itr == "-r" || *itr == "--release") {
      isDebug = false;
    } else if (*itr == "--compdb") {
      buildCompdb = true;
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
      return BUILD_CMD.noSuchArg(*itr);
    }
  }

  const auto manifest = Manifest::tryParse().unwrap();
  if (!buildCompdb) {
    std::string outDir;
    return buildImpl(manifest, outDir, isDebug);
  }

  // Build compilation database
  const std::string outDir =
      emitCompdb(manifest, isDebug, /*includeDevDeps=*/false);
  logger::info("Generated", "{}/compile_commands.json", outDir);
  return EXIT_SUCCESS;
}
