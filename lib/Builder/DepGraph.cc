#include "Builder/DepGraph.hpp"

#include "Manifest.hpp"

namespace cabin {

rs::Result<void> DepGraph::resolve() {
  rootManifest.emplace(
      rs_try(Manifest::tryParse(rootPath / Manifest::FILE_NAME)));
  return rs::Ok();
}

rs::Result<BuildGraph>
DepGraph::computeBuildGraph(const BuildProfile& buildProfile) const {
  rs_ensure(rootManifest.has_value(), "dependency graph not resolved");
  return BuildGraph::create(*rootManifest, buildProfile);
}

} // namespace cabin
