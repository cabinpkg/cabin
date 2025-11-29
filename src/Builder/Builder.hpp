#pragma once

#include "Builder/BuildGraph.hpp"
#include "Builder/BuildProfile.hpp"
#include "Builder/DepGraph.hpp"

#include <filesystem>
#include <optional>
#include <rs/result.hpp>
#include <string>
#include <vector>

namespace cabin {

namespace fs = std::filesystem;

struct ScheduleOptions {
  bool includeDevDeps = false;
  bool enableCoverage = false;
  bool suppressAnalysisLog = false;
  bool suppressFinishLog = false;
};

class Builder {
public:
  Builder(fs::path rootPath, BuildProfile buildProfile);

  rs::Result<void> schedule(const ScheduleOptions& options = {});
  rs::Result<void> build();
  rs::Result<void> test();
  rs::Result<void> run(const std::vector<std::string>& args);

  const BuildGraph& graph() const;
  const fs::path& outDirPath() const { return outDir; }
  std::string compdbRoot() const;

private:
  fs::path basePath;
  BuildProfile buildProfile;
  ScheduleOptions options;

  DepGraph depGraph;
  std::optional<BuildGraph> graphState;
  fs::path outDir;

  rs::Result<void> ensurePlanned() const;
};

} // namespace cabin
