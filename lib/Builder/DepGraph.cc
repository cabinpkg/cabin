#include "Builder/DepGraph.hpp"

#include "Manifest.hpp"

namespace cabin {

rs::Result<BuildGraph>
DepGraph::computeBuildGraph(const BuildProfile& buildProfile) const {
  return BuildGraph::create(rootManifest, buildProfile);
}

} // namespace cabin
