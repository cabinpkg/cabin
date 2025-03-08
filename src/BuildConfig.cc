#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Builder/Compiler.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"

#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/core.h>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <fstream>
#include <iomanip>
#include <optional>
#include <ostream>
#include <queue>
#include <ranges>
#include <spdlog/spdlog.h>
#include <sstream>
#include <string>
#include <string_view>
#include <tbb/blocked_range.h>
#include <tbb/concurrent_vector.h>
#include <tbb/parallel_for.h>
#include <tbb/spin_mutex.h>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

std::ostream&
operator<<(std::ostream& os, VarType type) {
  switch (type) {
    case VarType::Recursive:
      os << "=";
      break;
    case VarType::Simple:
      os << ":=";
      break;
    case VarType::Cond:
      os << "?=";
      break;
    case VarType::Append:
      os << "+=";
      break;
    case VarType::Shell:
      os << "!=";
      break;
  }
  return os;
}

Result<BuildConfig>
BuildConfig::init(const Manifest& manifest, BuildProfile buildProfile) {
  using std::string_view_literals::operator""sv;

  std::string libName;
  if (manifest.package.name.starts_with("lib")) {
    libName = fmt::format("{}.a", manifest.package.name);
  } else {
    libName = fmt::format("lib{}.a", manifest.package.name);
  }

  Project project = Try(Project::init(buildProfile, fs::current_path()));
  return Ok(BuildConfig(
      manifest, std::move(buildProfile), std::move(libName), std::move(project)
  ));
}

static void
emitDep(std::ostream& os, std::size_t& offset, const std::string_view dep) {
  constexpr std::size_t maxLineLen = 80;
  if (offset + dep.size() + 2 > maxLineLen) {  // 2 for space and \.
    // \ for line continuation. \ is the 80th character.
    os << std::setw(static_cast<int>(maxLineLen + 3 - offset)) << " \\\n ";
    offset = 2;
  }
  os << ' ' << dep;
  offset += dep.size() + 1;  // space
}

static void
emitTarget(
    std::ostream& os, const std::string_view target,
    const std::unordered_set<std::string>& dependsOn,
    const std::optional<std::string>& sourceFile = std::nullopt,
    const std::vector<std::string>& commands = {}
) {
  std::size_t offset = 0;

  os << target << ':';
  offset += target.size() + 2;  // : and space

  if (sourceFile.has_value()) {
    emitDep(os, offset, sourceFile.value());
  }
  for (const std::string_view dep : dependsOn) {
    emitDep(os, offset, dep);
  }
  os << '\n';

  for (const std::string_view cmd : commands) {
    os << '\t';
    if (!cmd.starts_with('@')) {
      os << "$(Q)";
    }
    os << cmd << '\n';
  }
  os << '\n';
}

void
BuildConfig::emitVariable(std::ostream& os, const std::string& varName) const {
  std::ostringstream oss;  // TODO: implement an elegant way to get type size.
  oss << varName << ' ' << variables.at(varName).type;
  const std::string left = oss.str();
  os << left << ' ';

  constexpr std::size_t maxLineLen = 80;  // TODO: share across sources?
  std::size_t offset = left.size() + 1;   // space
  std::string value;
  for (const char c : variables.at(varName).value) {
    if (c == ' ') {
      // Emit value
      if (offset + value.size() + 2 > maxLineLen) {  // 2 for space and '\'
        os << std::setw(static_cast<int>(maxLineLen + 3 - offset)) << "\\\n  ";
        offset = 2;
      }
      os << value << ' ';
      offset += value.size() + 1;
      value.clear();
    } else {
      value.push_back(c);
    }
  }

  if (!value.empty()) {
    if (offset + value.size() + 2 > maxLineLen) {  // 2 for space and '\'
      os << std::setw(static_cast<int>(maxLineLen + 3 - offset)) << "\\\n  ";
    }
    os << value;
  }
  os << '\n';
}

template <typename T>
Result<std::vector<std::string>>
topoSort(
    const std::unordered_map<std::string, T>& list,
    const std::unordered_map<std::string, std::vector<std::string>>& adjList
) {
  std::unordered_map<std::string, uint32_t> inDegree;
  for (const auto& var : list) {
    inDegree[var.first] = 0;
  }
  for (const auto& edge : adjList) {
    if (!list.contains(edge.first)) {
      continue;  // Ignore nodes not in list
    }
    if (!inDegree.contains(edge.first)) {
      inDegree[edge.first] = 0;
    }
    for (const auto& neighbor : edge.second) {
      inDegree[neighbor]++;
    }
  }

  std::queue<std::string> zeroInDegree;
  for (const auto& var : inDegree) {
    if (var.second == 0) {
      zeroInDegree.push(var.first);
    }
  }

  std::vector<std::string> res;
  while (!zeroInDegree.empty()) {
    const std::string node = zeroInDegree.front();
    zeroInDegree.pop();
    res.push_back(node);

    if (!adjList.contains(node)) {
      // No dependencies
      continue;
    }
    for (const std::string& neighbor : adjList.at(node)) {
      inDegree[neighbor]--;
      if (inDegree[neighbor] == 0) {
        zeroInDegree.push(neighbor);
      }
    }
  }

  if (res.size() != list.size()) {
    // Cycle detected
    Bail("too complex build graph");
  }
  return Ok(res);
}

Result<void>
BuildConfig::emitMakefile(std::ostream& os) const {
  const std::vector<std::string> sortedVars = Try(topoSort(variables, varDeps));
  for (const std::string& varName : sortedVars) {
    emitVariable(os, varName);
  }
  if (!sortedVars.empty() && !targets.empty()) {
    os << '\n';
  }

  if (phony.has_value()) {
    emitTarget(os, ".PHONY", phony.value());
  }
  if (all.has_value()) {
    emitTarget(os, "all", all.value());
  }

  const std::vector<std::string> sortedTargets =
      Try(topoSort(targets, targetDeps));
  for (const auto& sortedTarget : std::ranges::reverse_view(sortedTargets)) {
    emitTarget(
        os, sortedTarget, targets.at(sortedTarget).remDeps,
        targets.at(sortedTarget).sourceFile, targets.at(sortedTarget).commands
    );
  }
  return Ok();
}

void
BuildConfig::emitCompdb(std::ostream& os) const {
  const fs::path directory = manifest.path.parent_path();
  const std::string indent1(2, ' ');
  const std::string indent2(4, ' ');

  std::ostringstream oss;
  for (const auto& [target, targetInfo] : targets) {
    if (phony->contains(target)) {
      // Ignore phony dependencies.
      continue;
    }

    bool isCompileTarget = false;
    for (const std::string_view cmd : targetInfo.commands) {
      if (!cmd.starts_with("$(CXX)") && !cmd.starts_with("@$(CXX)")) {
        continue;
      }
      if (cmd.find("-c") == std::string_view::npos) {
        // Ignore link commands.
        continue;
      }
      isCompileTarget = true;
    }
    if (!isCompileTarget) {
      continue;
    }

    // We don't check the std::optional value because we know the first
    // dependency always exists for compile targets.
    const std::string sourceFile =
        fs::relative(targetInfo.sourceFile.value(), directory);
    // The output is the target.
    const std::string objFile = fs::relative(target, directory);
    const std::string cmd = project.compiler.getCompileCmd(sourceFile, objFile)
                                .addArg("-DCABIN_TEST")
                                .toString();

    oss << indent1 << "{\n";
    oss << indent2 << "\"directory\": " << directory << ",\n";
    oss << indent2 << "\"file\": " << std::quoted(sourceFile) << ",\n";
    oss << indent2 << "\"output\": " << std::quoted(objFile) << ",\n";
    oss << indent2 << "\"command\": " << std::quoted(cmd) << "\n";
    oss << indent1 << "},\n";
  }

  std::string output = oss.str();
  if (!output.empty()) {
    // Remove the last comma.
    output.pop_back();  // \n
    output.pop_back();  // ,
  }

  os << "[\n";
  os << output << '\n';
  os << "]\n";
}

Result<std::string>
BuildConfig::runMM(const std::string& sourceFile, const bool isTest) const {
  Command command = project.compiler.getMMCmd(sourceFile);
  if (isTest) {
    command.addArg("-DCABIN_TEST");
  }
  command.setWorkingDirectory(outBasePath);
  return getCmdOutput(command);
}

static std::unordered_set<std::string>
parseMMOutput(const std::string& mmOutput, std::string& target) {
  std::istringstream iss(mmOutput);
  std::getline(iss, target, ':');

  std::string dependency;
  std::unordered_set<std::string> deps;
  bool isFirst = true;
  while (std::getline(iss, dependency, ' ')) {
    if (!dependency.empty() && dependency.front() != '\\') {
      // Remove trailing newline if it exists
      if (dependency.back() == '\n') {
        dependency.pop_back();
      }
      // Drop the first dependency because it is the source file itself,
      // which we already know.
      if (isFirst) {
        isFirst = false;
        continue;
      }
      deps.insert(dependency);
    }
  }
  return deps;
}

bool
BuildConfig::isUpToDate(const std::string_view fileName) const {
  const fs::path filePath = outBasePath / fileName;

  if (!fs::exists(filePath)) {
    return false;
  }

  const fs::file_time_type makefileTime = fs::last_write_time(filePath);
  // Makefile depends on all files in ./src and cabin.toml.
  const fs::path srcDir = manifest.path.parent_path() / "src";
  for (const auto& entry : fs::recursive_directory_iterator(srcDir)) {
    if (fs::last_write_time(entry.path()) > makefileTime) {
      return false;
    }
  }
  return fs::last_write_time(manifest.path.parent_path() / "cabin.toml")
         <= makefileTime;
}

Result<bool>
BuildConfig::containsTestCode(const std::string& sourceFile) const {
  std::ifstream ifs(sourceFile);
  std::string line;
  while (std::getline(ifs, line)) {
    if (line.find("CABIN_TEST") != std::string::npos) {
      Command command = project.compiler.getPreprocessCmd(sourceFile);
      const std::string src = Try(getCmdOutput(command));

      command.addArg("-DCABIN_TEST");
      const std::string testSrc = Try(getCmdOutput(command));

      // If the source file contains CABIN_TEST, by processing the source
      // file with -E, we can check if the source file contains CABIN_TEST
      // or not semantically.  If the source file contains CABIN_TEST, the
      // test source file should be different from the original source
      // file.
      const bool containsTest = src != testSrc;
      if (containsTest) {
        spdlog::trace("Found test code: {}", sourceFile);
      }
      return Ok(containsTest);
    }
  }
  return Ok(false);
}

void
BuildConfig::defineCompileTarget(
    const std::string& objTarget, const std::string& sourceFile,
    const std::unordered_set<std::string>& remDeps, const bool isTest
) {
  std::vector<std::string> commands;
  commands.emplace_back("@mkdir -p $(@D)");
  commands.emplace_back("$(CXX) $(CXXFLAGS) $(DEFINES) $(INCLUDES)");
  if (isTest) {
    commands.back() += " -DCABIN_TEST";
  }
  commands.back() += " -c $< -o $@";
  defineTarget(objTarget, commands, remDeps, sourceFile);
}

void
BuildConfig::defineOutputTarget(
    const std::unordered_set<std::string>& buildObjTargets,
    const std::string& targetInputPath,
    const std::vector<std::string>& commands,
    const std::string& targetOutputPath
) {
  // Project binary target.
  std::unordered_set<std::string> projTargetDeps = { targetInputPath };
  collectBinDepObjs(
      projTargetDeps, "",
      targets.at(targetInputPath).remDeps,  // we don't need sourceFile
      buildObjTargets
  );

  defineTarget(targetOutputPath, commands, projTargetDeps);
}

// Map a path to header file to the corresponding object file.
//
// e.g., src/path/to/header.h -> cabin.d/path/to/header.o
std::string
BuildConfig::mapHeaderToObj(
    const fs::path& headerPath, const fs::path& buildOutPath
) const {
  fs::path objBaseDir = fs::relative(
      headerPath.parent_path(), manifest.path.parent_path() / "src"
  );
  if (objBaseDir != ".") {
    objBaseDir = buildOutPath / objBaseDir;
  } else {
    objBaseDir = buildOutPath;
  }
  return (objBaseDir / headerPath.stem()).string() + ".o";
}

// Recursively collect depending object files for a binary target.
// We know the binary depends on some header files.  We need to find
// if there is the corresponding object file for the header file.
// If it is, we should depend on the object file and recursively
// collect depending object files of the object file.
//
// Header files are known via -MM outputs.  Each -MM output is run
// for each source file.  So, we need objTargetDeps, which is the
// depending header files for the source file.
void
BuildConfig::collectBinDepObjs(  // NOLINT(misc-no-recursion)
    std::unordered_set<std::string>& deps,
    const std::string_view sourceFileName,
    const std::unordered_set<std::string>& objTargetDeps,
    const std::unordered_set<std::string>& buildObjTargets
) const {
  for (const fs::path headerPath : objTargetDeps) {
    if (sourceFileName == headerPath.stem()) {
      // We shouldn't depend on the original object file (e.g.,
      // cabin.d/path/to/file.o). We should depend on the test object
      // file (e.g., unittests/path/to/file.o).
      continue;
    }
    if (!HEADER_FILE_EXTS.contains(headerPath.extension())) {
      // We only care about header files.
      continue;
    }

    const std::string objTarget =
        mapHeaderToObj(headerPath, project.buildOutPath);
    if (deps.contains(objTarget)) {
      // We already added this object file.
      continue;
    }
    if (!buildObjTargets.contains(objTarget)) {
      // If the header file is not included in the source file, we
      // should not depend on the object file corresponding to the
      // header file.
      continue;
    }

    deps.insert(objTarget);
    collectBinDepObjs(
        deps, sourceFileName,
        targets.at(objTarget).remDeps,  // we don't need sourceFile
        buildObjTargets
    );
  }
}

Result<void>
BuildConfig::installDeps(const bool includeDevDeps) {
  const std::vector<CompilerOptions> depsCompOpts =
      Try(manifest.installDeps(includeDevDeps));

  // Flatten depsCompOpts into this->compiler.opts.
  for (const CompilerOptions& depOpts : depsCompOpts) {
    project.compiler.opts.merge(depOpts);
  }
  return Ok();
}

void
BuildConfig::setVariables() {
  defineSimpleVar("CXX", project.compiler.cxx);
  defineSimpleVar(
      "CXXFLAGS",
      fmt::format("{}", fmt::join(project.compiler.opts.cFlags.others, " "))
  );

  defineSimpleVar(
      "DEFINES",
      fmt::format("{}", fmt::join(project.compiler.opts.cFlags.macros, " "))
  );
  defineSimpleVar(
      "INCLUDES",
      fmt::format(
          "{}", fmt::join(project.compiler.opts.cFlags.includeDirs, " ")
      )
  );
  defineSimpleVar(
      "LDFLAGS",
      fmt::format(
          "{} {}", fmt::join(project.compiler.opts.ldFlags.others, " "),
          fmt::join(project.compiler.opts.ldFlags.libDirs, " ")
      )
  );
  defineSimpleVar(
      "LIBS",
      fmt::format("{}", fmt::join(project.compiler.opts.ldFlags.libs, " "))
  );
}

Result<void>
BuildConfig::processSrc(
    const fs::path& sourceFilePath,
    std::unordered_set<std::string>& buildObjTargets, tbb::spin_mutex* mtx
) {
  std::string objTarget;  // source.o
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath)), objTarget);

  const fs::path targetBaseDir = fs::relative(
      sourceFilePath.parent_path(), manifest.path.parent_path() / "src"
  );
  fs::path buildTargetBaseDir = project.buildOutPath;
  if (targetBaseDir != ".") {
    buildTargetBaseDir /= targetBaseDir;
  }

  const std::string buildObjTarget = buildTargetBaseDir / objTarget;

  if (mtx) {
    mtx->lock();
  }
  buildObjTargets.insert(buildObjTarget);
  defineCompileTarget(buildObjTarget, sourceFilePath, objTargetDeps);
  if (mtx) {
    mtx->unlock();
  }
  return Ok();
}

Result<std::unordered_set<std::string>>
BuildConfig::processSources(const std::vector<fs::path>& sourceFilePaths) {
  std::unordered_set<std::string> buildObjTargets;

  if (isParallel()) {
    tbb::concurrent_vector<std::string> results;
    tbb::spin_mutex mtx;
    tbb::parallel_for(
        tbb::blocked_range<std::size_t>(0, sourceFilePaths.size()),
        [&](const tbb::blocked_range<std::size_t>& rng) {
          for (std::size_t i = rng.begin(); i != rng.end(); ++i) {
            std::ignore = processSrc(sourceFilePaths[i], buildObjTargets, &mtx)
                              .map_err([&results](const auto& err) {
                                results.push_back(err->what());
                              });
          }
        }
    );
    if (!results.empty()) {
      Bail("{}", fmt::join(results, "\n"));
    }
  } else {
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      Try(processSrc(sourceFilePath, buildObjTargets));
    }
  }

  return Ok(buildObjTargets);
}

Result<void>
BuildConfig::processUnittestSrc(
    const fs::path& sourceFilePath,
    const std::unordered_set<std::string>& buildObjTargets,
    std::unordered_set<std::string>& testTargets, tbb::spin_mutex* mtx
) {
  if (!Try(containsTestCode(sourceFilePath))) {
    return Ok();
  }

  std::string objTarget;  // source.o
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

  const fs::path targetBaseDir = fs::relative(
      sourceFilePath.parent_path(), manifest.path.parent_path() / "src"
  );
  fs::path testTargetBaseDir = project.unittestOutPath;
  if (targetBaseDir != ".") {
    testTargetBaseDir /= targetBaseDir;
  }

  const std::string testObjTarget = testTargetBaseDir / objTarget;
  const std::string testTarget =
      (testTargetBaseDir / sourceFilePath.filename()).string() + ".test";

  // Test binary target.
  std::unordered_set<std::string> testTargetDeps = { testObjTarget };
  collectBinDepObjs(
      testTargetDeps, sourceFilePath.stem().string(), objTargetDeps,
      buildObjTargets
  );

  if (mtx) {
    mtx->lock();
  }
  // Test object target.
  defineCompileTarget(
      testObjTarget, sourceFilePath, objTargetDeps, /*isTest=*/true
  );

  // Test binary target.
  const std::vector<std::string> commands = { LINK_BIN_COMMAND };
  defineTarget(testTarget, commands, testTargetDeps);

  testTargets.insert(testTarget);
  if (mtx) {
    mtx->unlock();
  }
  return Ok();
}

static std::vector<fs::path>
listSourceFilePaths(const fs::path& dir) {
  std::vector<fs::path> sourceFilePaths;
  for (const auto& entry : fs::recursive_directory_iterator(dir)) {
    if (!SOURCE_FILE_EXTS.contains(entry.path().extension())) {
      continue;
    }
    sourceFilePaths.emplace_back(entry.path());
  }
  return sourceFilePaths;
}

Result<void>
BuildConfig::configureBuild() {
  const fs::path srcDir = manifest.path.parent_path() / "src";
  if (!fs::exists(srcDir)) {
    Bail("{} is required but not found", srcDir);
  }

  // find main source file
  const auto isMainSource = [](const fs::path& file) {
    return file.filename().stem() == "main";
  };
  const auto isLibSource = [](const fs::path& file) {
    return file.filename().stem() == "lib";
  };
  fs::path mainSource;
  for (const auto& entry : fs::directory_iterator(srcDir)) {
    const fs::path& path = entry.path();
    if (!SOURCE_FILE_EXTS.contains(path.extension())) {
      continue;
    }
    if (!isMainSource(path)) {
      continue;
    }
    if (!mainSource.empty()) {
      Bail("multiple main sources were found");
    }
    mainSource = path;
    hasBinaryTarget = true;
  }

  fs::path libSource;
  for (const auto& entry : fs::directory_iterator(srcDir)) {
    const fs::path& path = entry.path();
    if (!SOURCE_FILE_EXTS.contains(path.extension())) {
      continue;
    }
    if (!isLibSource(path)) {
      continue;
    }
    if (!libSource.empty()) {
      Bail("multiple lib sources were found");
    }
    libSource = path;
    hasLibraryTarget = true;
  }

  if (!hasBinaryTarget && !hasLibraryTarget) {
    Bail("src/(main|lib){} was not found", SOURCE_FILE_EXTS);
  }

  if (!fs::exists(outBasePath)) {
    fs::create_directories(outBasePath);
  }

  setVariables();

  std::unordered_set<std::string> all = {};
  if (hasBinaryTarget) {
    all.insert(manifest.package.name);
  }
  if (hasLibraryTarget) {
    all.insert(libName);
  }

  // Build rules
  setAll(all);
  addPhony("all");

  std::vector<fs::path> sourceFilePaths = listSourceFilePaths(srcDir);
  std::string srcs;
  for (const fs::path& sourceFilePath : sourceFilePaths) {
    if (sourceFilePath != mainSource && isMainSource(sourceFilePath)) {
      Diag::warn(
          "source file `{}` is named `main` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as the program's entry point. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string()
      );
    } else if (sourceFilePath != libSource && isLibSource(sourceFilePath)) {
      Diag::warn(
          "source file `{}` is named `lib` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as a hasLibraryTarget. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string()
      );
    }

    srcs += ' ' + sourceFilePath.string();
  }

  defineSimpleVar("SRCS", srcs);

  // Source Pass
  const std::unordered_set<std::string> buildObjTargets =
      Try(processSources(sourceFilePaths));

  if (hasBinaryTarget) {
    const std::vector<std::string> commands = { LINK_BIN_COMMAND };
    defineOutputTarget(
        buildObjTargets, project.buildOutPath / "main.o", commands,
        outBasePath / manifest.package.name
    );
  }

  if (hasLibraryTarget) {
    const std::vector<std::string> commands = { ARCHIVE_LIB_COMMAND };
    defineOutputTarget(
        buildObjTargets, project.buildOutPath / "lib.o", commands,
        outBasePath / libName
    );
  }

  // Test Pass
  std::unordered_set<std::string> testTargets;
  if (isParallel()) {
    tbb::concurrent_vector<std::string> results;
    tbb::spin_mutex mtx;
    tbb::parallel_for(
        tbb::blocked_range<std::size_t>(0, sourceFilePaths.size()),
        [&](const tbb::blocked_range<std::size_t>& rng) {
          for (std::size_t i = rng.begin(); i != rng.end(); ++i) {
            std::ignore =
                processUnittestSrc(
                    sourceFilePaths[i], buildObjTargets, testTargets, &mtx
                )
                    .map_err([&results](const auto& err) {
                      results.push_back(err->what());
                    });
          }
        }
    );
    if (!results.empty()) {
      Bail("{}", fmt::join(results, "\n"));
    }
  } else {
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      Try(processUnittestSrc(sourceFilePath, buildObjTargets, testTargets));
    }
  }

  // Tidy Pass
  defineCondVar("CABIN_TIDY", "clang-tidy");
  defineSimpleVar("TIDY_TARGETS", "$(patsubst %,tidy_%,$(SRCS))", { "SRCS" });
  defineTarget("tidy", {}, { "$(TIDY_TARGETS)" });
  defineTarget(
      "$(TIDY_TARGETS)",
      { "$(CABIN_TIDY) $(CABIN_TIDY_FLAGS) $< -- $(CXXFLAGS) "
        "$(DEFINES) -DCABIN_TEST $(INCLUDES)" },
      { "tidy_%: %" }
  );
  addPhony("tidy");
  addPhony("$(TIDY_TARGETS)");
  return Ok();
}

Result<BuildConfig>
emitMakefile(
    const Manifest& manifest, const BuildProfile& buildProfile,
    const bool includeDevDeps
) {
  const Profile& profile = manifest.profiles.at(buildProfile);
  auto config = Try(BuildConfig::init(manifest, buildProfile));

  // When emitting Makefile, we also build the project.  So, we need to
  // make sure the dependencies are installed.
  Try(config.installDeps(includeDevDeps));

  bool buildProj = false;
  bool buildCompDb = false;
  if (config.makefileIsUpToDate()) {
    spdlog::debug("Makefile is up to date");
  } else {
    spdlog::debug("Makefile is NOT up to date");
    buildProj = true;
  }
  if (profile.compDb) {
    if (config.compdbIsUpToDate()) {
      spdlog::debug("compile_commands.json is up to date");
    } else {
      spdlog::debug("compile_commands.json is NOT up to date");
      buildCompDb = true;
    }
  }
  if (!buildProj && !buildCompDb) {
    return Ok(config);
  }

  Try(config.configureBuild());

  if (buildProj) {
    std::ofstream makefile(config.outBasePath / "Makefile");
    Try(config.emitMakefile(makefile));
  }
  if (buildCompDb) {
    std::ofstream compDb(config.outBasePath / "compile_commands.json");
    config.emitCompdb(compDb);
  }

  return Ok(config);
}

/// @returns the directory where the compilation database is generated.
Result<std::string>
emitCompdb(
    const Manifest& manifest, const BuildProfile& buildProfile,
    const bool includeDevDeps
) {
  auto config = Try(BuildConfig::init(manifest, buildProfile));

  // compile_commands.json also needs INCLUDES, but not LIBS.
  Try(config.installDeps(includeDevDeps));

  if (config.compdbIsUpToDate()) {
    spdlog::debug("compile_commands.json is up to date");
    return Ok(config.outBasePath);
  }
  spdlog::debug("compile_commands.json is NOT up to date");

  Try(config.configureBuild());
  std::ofstream ofs(config.outBasePath / "compile_commands.json");
  config.emitCompdb(ofs);
  return Ok(config.outBasePath);
}

Command
getMakeCommand() {
  Command makeCommand("make");
  if (!isVerbose()) {
    makeCommand.addArg("-s").addArg("--no-print-directory").addArg("Q=@");
  }
  if (isQuiet()) {
    makeCommand.addArg("QUIET=1");
  }

  const std::size_t numThreads = getParallelism();
  if (numThreads > 1) {
    makeCommand.addArg("-j" + std::to_string(numThreads));
  }

  return makeCommand;
}

}  // namespace cabin

#ifdef CABIN_TEST

#  include "Rustify/Tests.hpp"

namespace tests {

using namespace cabin;  // NOLINT(build/namespaces,google-build-using-namespace)

// NOTE: These tests are commented out, BuildConfig won't be used in the
// future.  We can remove these tests then.
//
// static void
// testCycleVars() {
//   BuildConfig config("test");
//   config.defineSimpleVar("a", "b", { "b" });
//   config.defineSimpleVar("b", "c", { "c" });
//   config.defineSimpleVar("c", "a", { "a" });
//
//   assertException<CabinError>(
//       [&config]() {
//         std::ostringstream oss;
//         config.emitMakefile(oss);
//       },
//       "too complex build graph"
//   );
//
//   pass();
// }
//
// static void
// testSimpleVars() {
//   BuildConfig config("test");
//   config.defineSimpleVar("c", "3", { "b" });
//   config.defineSimpleVar("b", "2", { "a" });
//   config.defineSimpleVar("a", "1");
//
//   std::ostringstream oss;
//   config.emitMakefile(oss);
//
//   assertTrue(
//       oss.str().starts_with("a := 1\n"
//                             "b := 2\n"
//                             "c := 3\n")
//   );
//
//   pass();
// }
//
// static void
// testDependOnUnregisteredVar() {
//   BuildConfig config("test");
//   config.defineSimpleVar("a", "1", { "b" });
//
//   std::ostringstream oss;
//   config.emitMakefile(oss);
//
//   assertTrue(oss.str().starts_with("a := 1\n"));
//
//   pass();
// }
//
// static void
// testCycleTargets() {
//   BuildConfig config("test");
//   config.defineTarget("a", { "echo a" }, { "b" });
//   config.defineTarget("b", { "echo b" }, { "c" });
//   config.defineTarget("c", { "echo c" }, { "a" });
//
//   assertException<CabinError>(
//       [&config]() {
//         std::ostringstream oss;
//         config.emitMakefile(oss);
//       },
//       "too complex build graph"
//   );
//
//   pass();
// }
//
// static void
// testSimpleTargets() {
//   BuildConfig config("test");
//   config.defineTarget("a", { "echo a" });
//   config.defineTarget("b", { "echo b" }, { "a" });
//   config.defineTarget("c", { "echo c" }, { "b" });
//
//   std::ostringstream oss;
//   config.emitMakefile(oss);
//
//   assertTrue(
//       oss.str().ends_with("c: b\n"
//                           "\t$(Q)echo c\n"
//                           "\n"
//                           "b: a\n"
//                           "\t$(Q)echo b\n"
//                           "\n"
//                           "a:\n"
//                           "\t$(Q)echo a\n"
//                           "\n")
//   );
//
//   pass();
// }
//
// static void
// testDependOnUnregisteredTarget() {
//   BuildConfig config("test");
//   config.defineTarget("a", { "echo a" }, { "b" });
//
//   std::ostringstream oss;
//   config.emitMakefile(oss);
//
//   assertTrue(
//       oss.str().ends_with("a: b\n"
//                           "\t$(Q)echo a\n"
//                           "\n")
//   );
//
//   pass();
// }

}  // namespace tests

int
main() {
  // tests::testCycleVars();
  // tests::testSimpleVars();
  // tests::testDependOnUnregisteredVar();
  // tests::testCycleTargets();
  // tests::testSimpleTargets();
  // tests::testDependOnUnregisteredTarget();
}
#endif
