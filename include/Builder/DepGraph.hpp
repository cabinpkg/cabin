#pragma once

#include "Builder/BuildGraph.hpp"
#include "Builder/BuildProfile.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <filesystem>
#include <optional>
#include <utility>

namespace cabin {

namespace fs = std::filesystem;

class DepGraph {
public:
  explicit DepGraph(fs::path rootPath) : rootPath(std::move(rootPath)) {}

  Result<void> resolve();
  Result<BuildGraph> computeBuildGraph(const BuildProfile& buildProfile) const;

private:
  fs::path rootPath;
  std::optional<Manifest> rootManifest;
};

} // namespace cabin
