#pragma once

#include "Builder/BuildProfile.hpp"
#include "Builder/Compiler.hpp"
#include "Builder/NinjaPlan.hpp"
#include "Builder/Project.hpp"
#include "Builder/SourceLayout.hpp"
#include "Command.hpp"
#include "Manifest.hpp"

#include <cstdint>
#include <filesystem>
#include <optional>
#include <rs/result.hpp>
#include <string>
#include <string_view>
#include <tbb/spin_mutex.h>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

namespace fs = std::filesystem;

class BuildGraph {
public:
  enum class TestKind : std::uint8_t { Unit, Integration };

  struct TestTarget {
    std::string ninjaTarget;
    std::string sourcePath;
    TestKind kind = TestKind::Unit;
  };

  static rs::Result<BuildGraph> create(const Manifest& manifest,
                                       const BuildProfile& buildProfile);

  const fs::path& outBasePath() const { return outBasePath_; }
  const Manifest& manifest() const { return project.manifest; }
  const BuildProfile& buildProfile() const { return buildProfile_; }

  bool hasBinaryTarget() const { return hasBinaryTarget_; }
  bool hasLibraryTarget() const { return hasLibraryTarget_; }
  const std::string& libraryName() const { return libName; }
  const std::vector<TestTarget>& testTargets() const { return testTargets_; }

  rs::Result<void> installDeps(bool includeDevDeps);
  void enableCoverage();
  rs::Result<void> plan(bool logAnalysis = true);
  rs::Result<void> writeBuildFilesIfNeeded() const;
  rs::Result<void> generateCompdb() const;

  rs::Result<bool> needsBuild(const std::vector<std::string>& targets) const;
  Command ninjaCommand(bool dryRun = false) const;
  rs::Result<ExitStatus> buildTargets(const std::vector<std::string>& targets,
                                      std::string_view displayName) const;

private:
  struct CompileUnit {
    std::string source;
    std::unordered_set<std::string> dependencies;
    bool isTest = false;
  };

  struct SourceRoot {
    fs::path directory;
    fs::path objectSubdir;

    explicit SourceRoot(fs::path directory, fs::path objectSubdir = fs::path())
        : directory(std::move(directory)),
          objectSubdir(std::move(objectSubdir)) {}
  };

  BuildGraph(BuildProfile buildProfile, std::string libName, Project project,
             Compiler compiler);

  bool isUpToDate(std::string_view fileName) const;
  std::string mapHeaderToObj(const fs::path& headerPath) const;

  void registerCompileUnit(const std::string& objTarget,
                           const std::string& sourceFile,
                           const std::unordered_set<std::string>& dependencies,
                           bool isTest);

  rs::Result<std::string> runMM(const std::string& sourceFile,
                                bool isTest = false) const;
  rs::Result<bool> containsTestCode(const std::string& sourceFile) const;

  rs::Result<void> processSrc(const fs::path& sourceFilePath,
                              const SourceRoot& root,
                              std::unordered_set<std::string>& buildObjTargets,
                              tbb::spin_mutex* mtx = nullptr);
  rs::Result<std::unordered_set<std::string>>
  processSources(const std::vector<fs::path>& sourceFilePaths,
                 const SourceRoot& root);

  rs::Result<std::optional<TestTarget>>
  processUnittestSrc(const fs::path& sourceFilePath,
                     tbb::spin_mutex* mtx = nullptr);
  rs::Result<std::optional<TestTarget>>
  processIntegrationTestSrc(const fs::path& sourceFilePath,
                            tbb::spin_mutex* mtx = nullptr);

  void collectBinDepObjs( // NOLINT(misc-no-recursion)
      std::unordered_set<std::string>& deps, std::string_view sourceFileName,
      const std::unordered_set<std::string>& objTargetDeps,
      const std::unordered_set<std::string>& buildObjTargets) const;

  rs::Result<void> configure();
  void writeBuildFiles() const;

  fs::path outBasePath_;
  Project project;
  Compiler compiler;
  BuildProfile buildProfile_;
  std::string libName;

  bool hasBinaryTarget_{ false };
  bool hasLibraryTarget_{ false };

  std::unordered_map<std::string, CompileUnit> compileUnits;
  std::vector<TestTarget> testTargets_;
  std::unordered_set<std::string> srcObjectTargets;
  std::unordered_set<std::string> libSupportObjects;
  std::string archiver = "ar";

  std::string cxxFlags;
  std::string defines;
  std::string includes;
  std::string ldFlags;
  std::string libs;

  NinjaPlan ninjaPlan;
};

std::vector<fs::path> listSourceFilePaths(const fs::path& dir);

} // namespace cabin
