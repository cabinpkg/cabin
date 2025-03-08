#pragma once

#include "BuildProfile.hpp"
#include "Compiler.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <filesystem>

namespace cabin {

namespace fs = std::filesystem;

class Project {
  Project(
      const BuildProfile& buildProfile, Manifest manifest, Compiler compiler
  );

  void includeIfExist(const fs::path& path, bool isSystem = false);

public:
  const fs::path rootPath;
  const fs::path outBasePath;
  const fs::path buildOutPath;
  const fs::path unittestOutPath;
  const Manifest manifest;
  Compiler compiler;

  static Result<Project>
  init(const BuildProfile& buildProfile, const fs::path& rootDir);

  static Result<Project>
  init(const BuildProfile& buildProfile, const Manifest& manifest);
};

}  // namespace cabin
