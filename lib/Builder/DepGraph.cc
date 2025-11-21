#include "Builder/DepGraph.hpp"

#include "Manifest.hpp"

namespace cabin {

Result<void> DepGraph::resolve() {
  rootManifest.emplace(Try(Manifest::tryParse(rootPath / Manifest::FILE_NAME)));
  return Ok();
}

Result<BuildGraph>
DepGraph::computeBuildGraph(const BuildProfile& buildProfile) const {
  Ensure(rootManifest.has_value(), "dependency graph not resolved");
  return BuildGraph::create(*rootManifest, buildProfile);
}

} // namespace cabin
