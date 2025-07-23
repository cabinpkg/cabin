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

inline const std::string LINK_BIN_COMMAND =
    "$(CXX) $(LDFLAGS) $^ $(LIBS) -o $@";
inline const std::string ARCHIVE_LIB_COMMAND = "ar rcs $@ $^";

enum class VarType : uint8_t {
  Recursive,  // =
  Simple,     // :=
  Cond,       // ?=
  Append,     // +=
  Shell,      // !=
};

struct Variable {
  std::string value;
  VarType type = VarType::Simple;
};

struct Target {
  std::vector<std::string> commands;
  std::optional<std::string> sourceFile;
  std::unordered_set<std::string> remDeps;
};

class BuildConfig {
public:
  // NOLINTNEXTLINE(*-non-private-member-variables-in-classes)
  fs::path outBasePath;

private:
  Project project;
  Compiler compiler;
  BuildProfile buildProfile;
  std::string libName;

  // if we are building an binary
  bool hasBinaryTarget{ false };
  // if we are building a hasLibraryTarget
  bool hasLibraryTarget{ false };

  std::unordered_map<std::string, Variable> variables;
  std::unordered_map<std::string, std::vector<std::string>> varDeps;
  std::unordered_map<std::string, Target> targets;
  std::unordered_map<std::string, std::vector<std::string>> targetDeps;
  std::optional<std::unordered_set<std::string>> phony;
  std::optional<std::unordered_set<std::string>> all;

  bool isUpToDate(std::string_view fileName) const;
  std::string mapHeaderToObj(
      const fs::path& headerPath, const fs::path& buildOutPath
  ) const;

  explicit BuildConfig(
      BuildProfile buildProfile, std::string libName, Project project,
      Compiler compiler
  )
      : outBasePath(project.outBasePath), project(std::move(project)),
        compiler(std::move(compiler)), buildProfile(std::move(buildProfile)),
        libName(std::move(libName)) {}

public:
  static Result<BuildConfig>
  init(const Manifest& manifest, BuildProfile buildProfile = BuildProfile::Dev);

  bool hasBinTarget() const { return hasBinaryTarget; }
  bool hasLibTarget() const { return hasLibraryTarget; }
  const std::string& getLibName() const { return this->libName; }

  bool makefileIsUpToDate() const { return isUpToDate("Makefile"); }
  bool compdbIsUpToDate() const { return isUpToDate("compile_commands.json"); }

  void defineVar(
      const std::string& name, const Variable& value,
      const std::unordered_set<std::string>& dependsOn = {}
  ) {
    variables[name] = value;
    for (const std::string& dep : dependsOn) {
      // reverse dependency
      varDeps[dep].push_back(name);
    }
  }

  void defineSimpleVar(
      const std::string& name, const std::string& value,
      const std::unordered_set<std::string>& dependsOn = {}
  ) {
    defineVar(name, { .value = value, .type = VarType::Simple }, dependsOn);
  }

  void defineCondVar(
      const std::string& name, const std::string& value,
      const std::unordered_set<std::string>& dependsOn = {}
  ) {
    defineVar(name, { .value = value, .type = VarType::Cond }, dependsOn);
  }

  void defineTarget(
      const std::string& name, const std::vector<std::string>& commands,
      const std::unordered_set<std::string>& remDeps = {},
      const std::optional<std::string>& sourceFile = std::nullopt
  ) {
    targets[name] = { .commands = commands,
                      .sourceFile = sourceFile,
                      .remDeps = remDeps };

    if (sourceFile.has_value()) {
      targetDeps[sourceFile.value()].push_back(name);
    }
    for (const std::string& dep : remDeps) {
      // reverse dependency
      targetDeps[dep].push_back(name);
    }
  }

  void addPhony(const std::string& target) {
    if (!phony.has_value()) {
      phony = { target };
    } else {
      phony->insert(target);
    }
  }

  void setAll(const std::unordered_set<std::string>& dependsOn) {
    all = dependsOn;
  }

  void emitVariable(std::ostream& os, const std::string& varName) const;
  Result<void> emitMakefile(std::ostream& os) const;
  void emitCompdb(std::ostream& os) const;
  Result<std::string>
  runMM(const std::string& sourceFile, bool isTest = false) const;
  Result<bool> containsTestCode(const std::string& sourceFile) const;

  Result<void> installDeps(bool includeDevDeps);
  void setVariables();

  Result<void> processSrc(
      const fs::path& sourceFilePath,
      std::unordered_set<std::string>& buildObjTargets,
      tbb::spin_mutex* mtx = nullptr
  );
  Result<std::unordered_set<std::string>>
  processSources(const std::vector<fs::path>& sourceFilePaths);

  void defineCompileTarget(
      const std::string& objTarget, const std::string& sourceFile,
      const std::unordered_set<std::string>& remDeps, bool isTest = false
  );

  void defineOutputTarget(
      const std::unordered_set<std::string>& buildObjTargets,
      const std::string& targetInputPath,
      const std::vector<std::string>& commands,
      const std::string& targetOutputPath
  );

  void collectBinDepObjs(  // NOLINT(misc-no-recursion)
      std::unordered_set<std::string>& deps, std::string_view sourceFileName,
      const std::unordered_set<std::string>& objTargetDeps,
      const std::unordered_set<std::string>& buildObjTargets
  ) const;

  Result<void> processUnittestSrc(
      const fs::path& sourceFilePath,
      const std::unordered_set<std::string>& buildObjTargets,
      std::unordered_set<std::string>& testTargets,
      tbb::spin_mutex* mtx = nullptr
  );

  Result<void> configureBuild();
};

Result<BuildConfig> emitMakefile(
    const Manifest& manifest, const BuildProfile& buildProfile,
    bool includeDevDeps
);
Result<std::string> emitCompdb(
    const Manifest& manifest, const BuildProfile& buildProfile,
    bool includeDevDeps
);
Command getMakeCommand();

}  // namespace cabin
