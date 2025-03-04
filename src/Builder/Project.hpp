#pragma once

#include "../Compiler.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

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

  void setBuildProfile(bool isDebug);
};

}  // namespace cabin
