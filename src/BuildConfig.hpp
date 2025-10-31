#pragma once

#include "Builder/BuildProfile.hpp"
#include "Builder/Project.hpp"
#include "Command.hpp"
#include "Manifest.hpp"

#include <cstdint>
#include <optional>
#include <ostream>
#include <string>
#include <string_view>
#include <tbb/spin_mutex.h>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

// clang-format off
inline const std::unordered_set<std::string> SOURCE_FILE_EXTS{
  ".c", ".c++", ".cc", ".cpp", ".cxx"
};
inline const std::unordered_set<std::string> HEADER_FILE_EXTS{
  ".h", ".h++", ".hh", ".hpp", ".hxx"
};
// clang-format on

class BuildConfig {
public:
  struct TestTarget;
  // NOLINTNEXTLINE(*-non-private-member-variables-in-classes)
  fs::path outBasePath;

private:
  Project project;
  Compiler compiler;
  BuildProfile buildProfile;
  std::string libName;

  bool hasBinaryTarget{ false };
  bool hasLibraryTarget{ false };

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

  struct NinjaEdge {
    std::vector<std::string> outputs;
    std::string rule;
    std::vector<std::string> inputs;
    std::vector<std::string> implicitInputs;
    std::vector<std::string> orderOnlyInputs;
    std::vector<std::pair<std::string, std::string>> bindings;
  };

  std::unordered_map<std::string, CompileUnit> compileUnits;
  std::vector<NinjaEdge> ninjaEdges;
  std::vector<std::string> defaultTargets;
  std::vector<TestTarget> testTargets;
  std::unordered_set<std::string> srcObjectTargets;
  std::string archiver = "ar";

  std::string cxxFlags;
  std::string defines;
  std::string includes;
  std::string ldFlags;
  std::string libs;

  bool isUpToDate(std::string_view fileName) const;
  std::string mapHeaderToObj(const fs::path& headerPath) const;

  void addEdge(NinjaEdge edge);
  void registerCompileUnit(const std::string& objTarget,
                           const std::string& sourceFile,
                           const std::unordered_set<std::string>& dependencies,
                           bool isTest);
  void writeBuildNinja() const;
  void writeConfigNinja() const;
  void writeRulesNinja() const;
  void writeTargetsNinja() const;

  explicit BuildConfig(BuildProfile buildProfile, std::string libName,
                       Project project, Compiler compiler)
      : outBasePath(project.outBasePath), project(std::move(project)),
        compiler(std::move(compiler)), buildProfile(std::move(buildProfile)),
        libName(std::move(libName)) {}

public:
  enum class TestKind : std::uint8_t { Unit, Integration };

  struct TestTarget {
    std::string ninjaTarget;
    std::string sourcePath;
    TestKind kind = TestKind::Unit;
  };

  static Result<BuildConfig>
  init(const Manifest& manifest,
       const BuildProfile& buildProfile = BuildProfile::Dev);

  bool hasBinTarget() const { return hasBinaryTarget; }
  bool hasLibTarget() const { return hasLibraryTarget; }
  const std::string& getLibName() const { return libName; }

  bool ninjaIsUpToDate() const { return isUpToDate("build.ninja"); }
  bool compdbIsUpToDate() const { return isUpToDate("compile_commands.json"); }

  const std::vector<TestTarget>& getTestTargets() const { return testTargets; }

  Result<void> installDeps(bool includeDevDeps);
  void enableCoverage();

  Result<void> processSrc(const fs::path& sourceFilePath,
                          const SourceRoot& root,
                          std::unordered_set<std::string>& buildObjTargets,
                          tbb::spin_mutex* mtx = nullptr);
  Result<std::unordered_set<std::string>>
  processSources(const std::vector<fs::path>& sourceFilePaths,
                 const SourceRoot& root);

  Result<std::optional<TestTarget>>
  processUnittestSrc(const fs::path& sourceFilePath,
                     tbb::spin_mutex* mtx = nullptr);
  Result<std::optional<TestTarget>>
  processIntegrationTestSrc(const fs::path& sourceFilePath,
                            tbb::spin_mutex* mtx = nullptr);

  void collectBinDepObjs( // NOLINT(misc-no-recursion)
      std::unordered_set<std::string>& deps, std::string_view sourceFileName,
      const std::unordered_set<std::string>& objTargetDeps,
      const std::unordered_set<std::string>& buildObjTargets) const;

  Result<void> configureBuild();

  void emitCompdb(std::ostream& os) const;
  Result<std::string> runMM(const std::string& sourceFile,
                            bool isTest = false) const;
  Result<bool> containsTestCode(const std::string& sourceFile) const;

  void writeBuildFiles() const;
};

Result<BuildConfig> emitNinja(const Manifest& manifest,
                              const BuildProfile& buildProfile,
                              bool includeDevDeps, bool enableCoverage = false);
Result<std::string> emitCompdb(const Manifest& manifest,
                               const BuildProfile& buildProfile,
                               bool includeDevDeps);
Command getNinjaCommand();
Result<bool> ninjaNeedsWork(const fs::path& outDir,
                            const std::vector<std::string>& targets);

} // namespace cabin
