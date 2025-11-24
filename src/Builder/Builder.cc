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

Result<void> Builder::schedule(const ScheduleOptions& options) {
  this->options = options;

  Try(depGraph.resolve());
  graphState.emplace(Try(depGraph.computeBuildGraph(buildProfile)));

  if (options.enableCoverage) {
    graphState->enableCoverage();
  }
  Try(graphState->installDeps(options.includeDevDeps));
  const bool logAnalysis = !options.suppressAnalysisLog;
  Try(graphState->plan(logAnalysis));
  outDir = graphState->outBasePath();
  return Ok();
}

Result<void> Builder::ensurePlanned() const {
  Ensure(graphState.has_value(), "builder.schedule() must be called first");
  return Ok();
}

Result<void> Builder::build() {
  Try(ensurePlanned());
  const auto startBuild = std::chrono::steady_clock::now();

  ExitStatus status(EXIT_SUCCESS);
  const Manifest& mf = graphState->manifest();

  if (graphState->hasLibraryTarget()) {
    status =
        Try(graphState->buildTargets({ graphState->libraryName() },
                                     fmt::format("{}(lib)", mf.package.name)));
  }

  if (status.success() && graphState->hasBinaryTarget()) {
    status =
        Try(graphState->buildTargets({ mf.package.name }, mf.package.name));
  }

  const auto endBuild = std::chrono::steady_clock::now();
  const std::chrono::duration<double> buildElapsed = endBuild - startBuild;

  const Profile& profile = mf.profiles.at(buildProfile);
  Ensure(status.success(), "build failed");
  Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
             profile, buildElapsed.count());
  return Ok();
}

Result<void> Builder::test() {
  Try(ensurePlanned());

  const Manifest& mf = graphState->manifest();
  const std::vector<BuildGraph::TestTarget>& targets =
      graphState->testTargets();

  const auto buildStart = std::chrono::steady_clock::now();
  ExitStatus status(EXIT_SUCCESS);

  if (graphState->hasLibraryTarget()) {
    status =
        Try(graphState->buildTargets({ graphState->libraryName() },
                                     fmt::format("{}(lib)", mf.package.name)));
    Ensure(status.success(), "build failed");
  }

  if (!targets.empty()) {
    std::vector<std::string> names;
    names.reserve(targets.size());
    for (const auto& target : targets) {
      names.push_back(target.ninjaTarget);
    }
    status = Try(graphState->buildTargets(
        names, fmt::format("{}(test)", mf.package.name)));
    Ensure(status.success(), "build failed");
  } else {
    Diag::warn("No test targets found");
    return Ok();
  }

  const auto buildEnd = std::chrono::steady_clock::now();
  const std::chrono::duration<double> buildElapsed = buildEnd - buildStart;
  const Profile& profile = mf.profiles.at(buildProfile);
  Diag::info("Finished", "`{}` profile [{}] target(s) in {:.2f}s", buildProfile,
             profile, buildElapsed.count());

  const auto runStart = std::chrono::steady_clock::now();

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
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

  for (const auto& target : targets) {
    const fs::path absoluteBinary = outDir / target.ninjaTarget;
    const std::string testBinPath =
        fs::relative(absoluteBinary, mf.path.parent_path()).string();
    Diag::info("Running", "{} test {} ({})", labelFor(target.kind),
               target.sourcePath, testBinPath);

    const ExitStatus curExitStatus =
        Try(execCmd(Command(absoluteBinary.string())));
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
      fmt::format("{} passed; {} failed; finished in {:.2f}s", numPassed,
                  numFailed, runElapsed.count());
  if (!summaryStatus.success()) {
    return Err(anyhow::anyhow(summary));
  }
  Diag::info("Ok", "{}", summary);
  return Ok();
}

Result<void> Builder::run(const std::vector<std::string>& args) {
  Try(build());

  const Manifest& mf = graphState->manifest();
  Diag::info("Running", "`{}/{}`",
             fs::relative(outDir, mf.path.parent_path()).string(),
             mf.package.name);
  const Command command((outDir / mf.package.name).string(), args);
  const ExitStatus exitStatus = Try(execCmd(command));
  if (exitStatus.success()) {
    return Ok();
  }
  Bail("run {}", exitStatus);
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
