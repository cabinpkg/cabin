#pragma once

#include "Builder/BuildProfile.hpp"
#include "Builder/Compiler.hpp"
#include "Manifest.hpp"

#include <filesystem>
#include <rs/result.hpp>

namespace cabin {

namespace fs = std::filesystem;

class Project {
  Project(const BuildProfile& buildProfile, Manifest manifest,
          CompilerOpts compilerOpts);

  void includeIfExist(const fs::path& path, bool isSystem = false);

public:
  const fs::path rootPath;
  const fs::path outBasePath;
  const fs::path buildOutPath;
  const fs::path unittestOutPath;
  const fs::path integrationTestOutPath;
  const Manifest manifest;
  CompilerOpts compilerOpts;

  static rs::Result<Project> init(const BuildProfile& buildProfile,
                                  const fs::path& rootDir);

  static rs::Result<Project> init(const BuildProfile& buildProfile,
                                  const Manifest& manifest);
};

} // namespace cabin
