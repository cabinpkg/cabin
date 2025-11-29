#include "Builder/Builder.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Parallelism.hpp"

#include <chrono>
#include <filesystem>
#include <fmt/format.h>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace cabin {

Builder::Builder(fs::path rootPath, BuildProfile buildProfile)
    : basePath(std::move(rootPath)), buildProfile(std::move(buildProfile)),
      depGraph(basePath) {}

rs::Result<void> Builder::schedule(const ScheduleOptions& options) {
  this->options = options;

  rs_try(depGraph.resolve());
  graphState.emplace(rs_try(depGraph.computeBuildGraph(buildProfile)));

  const bool logAnalysis = !options.suppressAnalysisLog;
  if (logAnalysis) {
    Diag::info("Analyzing", "project dependencies...");
  }

  if (options.enableCoverage) {
    graphState->enableCoverage();
  }
  rs_try(
      graphState->installDeps(options.includeDevDeps, options.suppressDepDiag));
  rs_try(graphState->plan(false));
  outDir = graphState->outBasePath();
  return rs::Ok();
}

rs::Result<void> Builder::ensurePlanned() const {
  rs_ensure(graphState.has_value(), "builder.schedule() must be called first");
  return rs::Ok();
}

rs::Result<void> Builder::build() {
  rs_try(ensurePlanned());
  const auto startBuild = std::chrono::steady_clock::now();

  ExitStatus status(EXIT_SUCCESS);
  const Manifest& mf = graphState->manifest();

  if (graphState->hasLibraryTarget()) {
    status = rs_try(
        graphState->buildTargets({ graphState->libraryName() },
                                 fmt::format("{}(lib)", mf.package.name)));
  }

  if (status.success() && graphState->hasBinaryTarget()) {
    status =
        rs_try(graphState->buildTargets({ mf.package.name }, mf.package.name));
  }

  const auto endBuild = std::chrono::steady_clock::now();
  const std::chrono::duration<double> buildElapsed = endBuild - startBuild;

  const Profile& profile = mf.profiles.at(buildProfile);
  rs_ensure(status.success(), "build failed");
  if (!options.suppressFinishLog) {
    Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s",
               buildProfile, profile, buildElapsed.count());
  }
  return rs::Ok();
}

rs::Result<void> Builder::test(std::optional<std::string> testName) {
  rs_try(ensurePlanned());

  const Manifest& mf = graphState->manifest();
  const std::vector<BuildGraph::TestTarget>& targets =
      graphState->testTargets();

  const auto buildStart = std::chrono::steady_clock::now();
  ExitStatus status(EXIT_SUCCESS);

  if (graphState->hasLibraryTarget()) {
    status = rs_try(
        graphState->buildTargets({ graphState->libraryName() },
                                 fmt::format("{}(lib)", mf.package.name)));
    rs_ensure(status.success(), "build failed");
  }

  if (!targets.empty()) {
    std::vector<std::string> names;
    names.reserve(targets.size());
    for (const auto& target : targets) {
      names.push_back(target.ninjaTarget);
    }
    status = rs_try(graphState->buildTargets(
        names, fmt::format("{}(test)", mf.package.name)));
    rs_ensure(status.success(), "build failed");
  } else {
    Diag::warn("No test targets found");
    return rs::Ok();
  }

  const auto buildEnd = std::chrono::steady_clock::now();
  const std::chrono::duration<double> buildElapsed = buildEnd - buildStart;
  const Profile& profile = mf.profiles.at(buildProfile);
  Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
             profile, buildElapsed.count());

  const auto runStart = std::chrono::steady_clock::now();

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
  std::size_t numFilteredOut = 0;
  ExitStatus summaryStatus(EXIT_SUCCESS);

  const auto labelFor = [](BuildGraph::TestKind kind) {
    switch (kind) {
    case BuildGraph::TestKind::Integration:
      return std::string_view("integration");
    case BuildGraph::TestKind::Unit:
      return std::string_view("unit");
    }
    std::unreachable();
  };

  for (const auto& testTarget : targets) {
    if (testName.has_value()
        && !testTarget.ninjaTarget.contains(testName.value())) {
      ++numFilteredOut;
      continue;
    }

    const fs::path absoluteBinary = outDir / testTarget.ninjaTarget;
    const std::string testBinPath =
        fs::relative(absoluteBinary, mf.path.parent_path()).string();
    Diag::info("Running", "{} test {} ({})", labelFor(testTarget.kind),
               testTarget.sourcePath, testBinPath);

    const ExitStatus curExitStatus =
        rs_try(execCmd(Command(absoluteBinary.string())));
    if (curExitStatus.success()) {
      ++numPassed;
    } else {
      ++numFailed;
      summaryStatus = curExitStatus;
    }
  }

  const auto runEnd = std::chrono::steady_clock::now();
  const std::chrono::duration<double> runElapsed = runEnd - runStart;

  const std::string summary =
      fmt::format("{} passed; {} failed; {} filtered out; finished in {:.2f}s",
                  numPassed, numFailed, numFilteredOut, runElapsed.count());
  if (!summaryStatus.success()) {
    return rs::Err(rs::anyhow(summary));
  }
  if (!options.suppressFinishLog) {
    Diag::info("Ok", "{}", summary);
  }
  return rs::Ok();
}

rs::Result<void> Builder::run(const std::vector<std::string>& args) {
  rs_try(build());

  const Manifest& mf = graphState->manifest();
  Diag::info("Running", "`{}/{}`",
             fs::relative(outDir, mf.path.parent_path()).string(),
             mf.package.name);
  const Command command((outDir / mf.package.name).string(), args);
  const ExitStatus exitStatus = rs_try(execCmd(command));
  if (exitStatus.success()) {
    return rs::Ok();
  }
  rs_bail("run {}", exitStatus);
}

const BuildGraph& Builder::graph() const {
  if (!graphState) {
    throw std::logic_error("builder.schedule() must be called first");
  }
  return *graphState;
}

std::string Builder::compdbRoot() const {
  if (!graphState) {
    throw std::logic_error("builder.schedule() must be called first");
  }
  return outDir.parent_path().string();
}

} // namespace cabin
