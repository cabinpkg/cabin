#pragma once

#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"
#include "BuildProfile.hpp"
#include "Compiler.hpp"

#include <filesystem>
#include <utility>

namespace cabin {

namespace fs = std::filesystem;

class Project {
  Project(Manifest manifest, Compiler compiler)
      : manifest(std::move(manifest)), compiler(std::move(compiler)) {};

public:
  const Manifest manifest;
  Compiler compiler;

  static Result<Project> init(const fs::path& rootDir);

  void setBuildProfile(const BuildProfile& buildProfile);
};

}  // namespace cabin
