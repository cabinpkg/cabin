#pragma once

#include "Builder/BuildGraph.hpp"
#include "Builder/BuildProfile.hpp"
#include "Manifest.hpp"

#include <filesystem>
#include <optional>
#include <rs/result.hpp>
#include <utility>

namespace cabin {

namespace fs = std::filesystem;

class DepGraph {
public:
  explicit DepGraph(Manifest manifest) : rootManifest(std::move(manifest)) {}

  rs::Result<BuildGraph>
  computeBuildGraph(const BuildProfile& buildProfile) const;

private:
  Manifest rootManifest;
};

} // namespace cabin
