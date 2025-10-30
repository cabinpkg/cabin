#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Builder/Compiler.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"

#include <algorithm>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/core.h>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <fstream>
#include <iomanip>
#include <nlohmann/json.hpp>
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

static std::string parentDirOrDot(const std::string& path) {
  const fs::path parent = fs::path(path).parent_path();
  if (parent.empty()) {
    return ".";
  }
  return parent.generic_string();
}

template <typename Range>
static std::string joinFlags(const Range& range) {
  if (range.empty()) {
    return "";
  }
  return fmt::format("{}", fmt::join(range, " "));
}

static std::string combineFlags(std::initializer_list<std::string_view> parts) {
  std::string result;
  for (const std::string_view part : parts) {
    if (part.empty()) {
      continue;
    }
    if (!result.empty()) {
      result.push_back(' ');
    }
    result.append(part);
  }
  return result;
}

static std::unordered_set<std::string>
parseMMOutput(const std::string& mmOutput, std::string& target) {
  std::istringstream iss(mmOutput);
  std::getline(iss, target, ':');

  std::string dependency;
  std::unordered_set<std::string> deps;
  bool isFirst = true;
  while (std::getline(iss, dependency, ' ')) {
    if (dependency.empty() || dependency.front() == '\\') {
      continue;
    }
    if (dependency.back() == '\n') {
      dependency.pop_back();
    }
    if (isFirst) {
      isFirst = false;
      continue;
    }
    deps.insert(dependency);
  }
  return deps;
}

static std::vector<fs::path> listSourceFilePaths(const fs::path& dir) {
  std::vector<fs::path> sourceFilePaths;
  for (const auto& entry : fs::recursive_directory_iterator(dir)) {
    if (!SOURCE_FILE_EXTS.contains(entry.path().extension().string())) {
      continue;
    }
    sourceFilePaths.emplace_back(entry.path());
  }
  std::ranges::sort(sourceFilePaths);
  return sourceFilePaths;
}

Result<BuildConfig> BuildConfig::init(const Manifest& manifest,
                                      const BuildProfile& buildProfile) {
  using std::string_view_literals::operator""sv;

  std::string libName;
  if (manifest.package.name.starts_with("lib")) {
    libName = fmt::format("{}.a", manifest.package.name);
  } else {
    libName = fmt::format("lib{}.a", manifest.package.name);
  }

  Project project = Try(Project::init(buildProfile, manifest));
  return Ok(BuildConfig(buildProfile, std::move(libName), std::move(project),
                        Try(Compiler::init())));
}

bool BuildConfig::isUpToDate(const std::string_view fileName) const {
  const fs::path filePath = outBasePath / fileName;

  if (!fs::exists(filePath)) {
    return false;
  }

  const fs::file_time_type configTime = fs::last_write_time(filePath);
  const fs::path srcDir = project.rootPath / "src";
  for (const auto& entry : fs::recursive_directory_iterator(srcDir)) {
    if (fs::last_write_time(entry.path()) > configTime) {
      return false;
    }
  }
  return fs::last_write_time(project.manifest.path) <= configTime;
}

std::string BuildConfig::mapHeaderToObj(const fs::path& headerPath) const {
  fs::path objBase = fs::relative(project.buildOutPath, outBasePath);
  const fs::path relHeader =
      fs::relative(headerPath.parent_path(), project.rootPath / "src");
  if (relHeader != ".") {
    objBase /= relHeader;
  }
  objBase /= headerPath.stem();
  objBase += ".o";
  return objBase.generic_string();
}

void BuildConfig::addEdge(NinjaEdge edge) {
  ninjaEdges.push_back(std::move(edge));
}

void BuildConfig::registerCompileUnit(
    const std::string& objTarget, const std::string& sourceFile,
    const std::unordered_set<std::string>& dependencies, const bool isTest) {
  compileUnits[objTarget] = CompileUnit{ .source = sourceFile,
                                         .dependencies = dependencies,
                                         .isTest = isTest };

  NinjaEdge edge;
  edge.outputs = { objTarget };
  edge.rule = "cxx_compile";
  edge.inputs = { sourceFile };
  edge.implicitInputs.assign(dependencies.begin(), dependencies.end());
  std::ranges::sort(edge.implicitInputs);
  edge.bindings.emplace_back("out_dir", parentDirOrDot(objTarget));
  edge.bindings.emplace_back("extra_flags", isTest ? "-DCABIN_TEST" : "");
  addEdge(std::move(edge));
}

void BuildConfig::writeBuildFiles() const {
  writeBuildNinja();
  writeConfigNinja();
  writeRulesNinja();
  writeTargetsNinja();
}

void BuildConfig::writeBuildNinja() const {
  std::ofstream buildFile(outBasePath / "build.ninja");
  buildFile << "# Generated by Cabin\n";
  buildFile << "ninja_required_version = 1.11\n\n";
  buildFile << "include config.ninja\n";
  buildFile << "include rules.ninja\n";
  buildFile << "include targets.ninja\n\n";
  if (!defaultTargets.empty()) {
    buildFile << "default " << joinFlags(defaultTargets) << '\n';
  }
}

void BuildConfig::writeConfigNinja() const {
  std::ofstream cfg(outBasePath / "config.ninja");
  cfg << "# Build variables\n";
  cfg << "CXX = " << compiler.cxx << '\n';
  cfg << "CXXFLAGS = " << cxxFlags << '\n';
  cfg << "DEFINES = " << defines << '\n';
  cfg << "INCLUDES = " << includes << '\n';
  cfg << "LDFLAGS = " << ldFlags << '\n';
  cfg << "LIBS = " << libs << '\n';
}

void BuildConfig::writeRulesNinja() const {
  std::ofstream rules(outBasePath / "rules.ninja");

  rules << "rule cxx_compile\n";
  rules << "  command = $CXX $DEFINES $INCLUDES $CXXFLAGS $extra_flags -c $in "
           "-o $out\n";
  rules << "  description = CXX $out\n\n";

  rules << "rule cxx_link\n";
  rules << "  command = $CXX $in $LDFLAGS $LIBS -o $out\n";
  rules << "  description = LINK $out\n\n";

  rules << "rule ar_archive\n";
  rules << "  command = ar rcs $out $in\n";
  rules << "  description = AR $out\n\n";
}

void BuildConfig::writeTargetsNinja() const {
  std::ofstream targetsFile(outBasePath / "targets.ninja");

  for (const NinjaEdge& edge : ninjaEdges) {
    targetsFile << "build " << joinFlags(edge.outputs);
    targetsFile << ": " << edge.rule;
    if (!edge.inputs.empty()) {
      targetsFile << ' ' << joinFlags(edge.inputs);
    }
    if (!edge.implicitInputs.empty()) {
      targetsFile << " | " << joinFlags(edge.implicitInputs);
    }
    if (!edge.orderOnlyInputs.empty()) {
      targetsFile << " || " << joinFlags(edge.orderOnlyInputs);
    }
    targetsFile << '\n';
    for (const auto& [key, value] : edge.bindings) {
      targetsFile << "  " << key << " = " << value << '\n';
    }
    targetsFile << '\n';
  }

  if (!defaultTargets.empty()) {
    targetsFile << "build all: phony " << joinFlags(defaultTargets) << '\n'
                << '\n';
  }
  if (!testTargets.empty()) {
    std::vector<std::string> testTargetNames;
    testTargetNames.reserve(testTargets.size());
    for (const TestTarget& target : testTargets) {
      testTargetNames.push_back(target.ninjaTarget);
    }
    targetsFile << "build tests: phony " << joinFlags(testTargetNames) << '\n'
                << '\n';
  }
}

Result<std::string> BuildConfig::runMM(const std::string& sourceFile,
                                       const bool isTest) const {
  Command command = compiler.makeMMCmd(project.compilerOpts, sourceFile);
  if (isTest) {
    command.addArg("-DCABIN_TEST");
  }
  command.setWorkingDirectory(outBasePath);
  return getCmdOutput(command);
}

Result<bool>
BuildConfig::containsTestCode(const std::string& sourceFile) const {
  std::ifstream ifs(sourceFile);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.contains("CABIN_TEST")) {
      continue;
    }

    Command command =
        compiler.makePreprocessCmd(project.compilerOpts, sourceFile);
    const std::string src = Try(getCmdOutput(command));

    command.addArg("-DCABIN_TEST");
    const std::string testSrc = Try(getCmdOutput(command));

    const bool containsTest = src != testSrc;
    if (containsTest) {
      spdlog::trace("Found test code: {}", sourceFile);
    }
    return Ok(containsTest);
  }
  return Ok(false);
}

Result<void>
BuildConfig::processSrc(const fs::path& sourceFilePath,
                        std::unordered_set<std::string>& buildObjTargets,
                        tbb::spin_mutex* mtx) {
  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath)), objTarget);

  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), project.rootPath / "src");
  fs::path buildTargetBaseDir = project.buildOutPath;
  if (targetBaseDir != ".") {
    buildTargetBaseDir /= targetBaseDir;
  }

  const fs::path objOutput = buildTargetBaseDir / objTarget;
  const std::string buildObjTarget =
      fs::relative(objOutput, outBasePath).generic_string();

  if (mtx) {
    mtx->lock();
  }
  buildObjTargets.insert(buildObjTarget);
  registerCompileUnit(buildObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/false);
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
        });
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

Result<std::optional<BuildConfig::TestTarget>> BuildConfig::processUnittestSrc(
    const fs::path& sourceFilePath,
    const std::unordered_set<std::string>& buildObjTargets,
    tbb::spin_mutex* mtx) {
  if (!Try(containsTestCode(sourceFilePath))) {
    return Ok(std::optional<TestTarget>());
  }

  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), project.rootPath / "src");
  fs::path testTargetBaseDir = project.unittestOutPath;
  if (targetBaseDir != ".") {
    testTargetBaseDir /= targetBaseDir;
  }

  const fs::path testObjOutput = testTargetBaseDir / objTarget;
  const std::string testObjTarget =
      fs::relative(testObjOutput, outBasePath).generic_string();
  const fs::path testBinaryPath =
      (testTargetBaseDir / sourceFilePath.filename()).concat(".test");
  const std::string testBinary =
      fs::relative(testBinaryPath, outBasePath).generic_string();

  std::unordered_set<std::string> deps = { testObjTarget };
  collectBinDepObjs(deps, sourceFilePath.stem().string(), objTargetDeps,
                    buildObjTargets);

  std::vector<std::string> linkInputs(deps.begin(), deps.end());
  std::ranges::sort(linkInputs);

  NinjaEdge linkEdge;
  linkEdge.outputs = { testBinary };
  linkEdge.rule = "cxx_link";
  linkEdge.inputs = std::move(linkInputs);
  linkEdge.bindings.emplace_back("out_dir", parentDirOrDot(testBinary));

  if (mtx) {
    mtx->lock();
  }
  registerCompileUnit(testObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/true);
  addEdge(std::move(linkEdge));
  if (mtx) {
    mtx->unlock();
  }

  TestTarget testTarget;
  testTarget.ninjaTarget = testBinary;
  testTarget.sourcePath =
      fs::relative(sourceFilePath, project.rootPath).generic_string();
  testTarget.kind = TestKind::Unit;

  return Ok(std::optional<TestTarget>(std::move(testTarget)));
}

Result<std::optional<BuildConfig::TestTarget>>
BuildConfig::processIntegrationTestSrc(
    const fs::path& sourceFilePath,
    const std::unordered_set<std::string>& buildObjTargets,
    tbb::spin_mutex* mtx) {
  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), project.rootPath / "tests");
  fs::path testTargetBaseDir = project.integrationTestOutPath;
  if (targetBaseDir != ".") {
    testTargetBaseDir /= targetBaseDir;
  }

  const fs::path testObjOutput = testTargetBaseDir / objTarget;
  const std::string testObjTarget =
      fs::relative(testObjOutput, outBasePath).generic_string();
  const fs::path testBinaryPath = testTargetBaseDir / sourceFilePath.stem();
  const std::string testBinary =
      fs::relative(testBinaryPath, outBasePath).generic_string();

  std::unordered_set<std::string> deps = { testObjTarget };
  collectBinDepObjs(deps, sourceFilePath.stem().string(), objTargetDeps,
                    buildObjTargets);
  for (const std::string& obj : buildObjTargets) {
    if (obj.ends_with("/main.o") || obj == "main.o") {
      continue;
    }
    deps.insert(obj);
  }
  if (hasLibraryTarget) {
    deps.insert(libName);
  }

  std::vector<std::string> linkInputs(deps.begin(), deps.end());
  std::ranges::sort(linkInputs);

  NinjaEdge linkEdge;
  linkEdge.outputs = { testBinary };
  linkEdge.rule = "cxx_link";
  linkEdge.inputs = std::move(linkInputs);
  linkEdge.bindings.emplace_back("out_dir", parentDirOrDot(testBinary));

  if (mtx) {
    mtx->lock();
  }
  registerCompileUnit(testObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/true);
  addEdge(std::move(linkEdge));
  if (mtx) {
    mtx->unlock();
  }

  TestTarget testTarget;
  testTarget.ninjaTarget = testBinary;
  testTarget.sourcePath =
      fs::relative(sourceFilePath, project.rootPath).generic_string();
  testTarget.kind = TestKind::Integration;

  return Ok(std::optional<TestTarget>(std::move(testTarget)));
}

void BuildConfig::collectBinDepObjs(
    std::unordered_set<std::string>& deps,
    const std::string_view sourceFileName,
    const std::unordered_set<std::string>& objTargetDeps,
    const std::unordered_set<std::string>& buildObjTargets) const {
  for (const std::string& dep : objTargetDeps) {
    const fs::path headerPath = dep;
    if (sourceFileName == headerPath.stem().string()) {
      continue;
    }
    if (!HEADER_FILE_EXTS.contains(headerPath.extension().string())) {
      continue;
    }

    const std::string objTarget = mapHeaderToObj(headerPath);
    if (!buildObjTargets.contains(objTarget)) {
      continue;
    }
    if (!deps.insert(objTarget).second) {
      continue;
    }

    const auto it = compileUnits.find(objTarget);
    if (it == compileUnits.end()) {
      continue;
    }
    collectBinDepObjs(deps, sourceFileName, it->second.dependencies,
                      buildObjTargets);
  }
}

Result<void> BuildConfig::installDeps(const bool includeDevDeps) {
  const std::vector<CompilerOpts> depsCompOpts =
      Try(project.manifest.installDeps(includeDevDeps));

  for (const CompilerOpts& depOpts : depsCompOpts) {
    project.compilerOpts.merge(depOpts);
  }
  return Ok();
}

void BuildConfig::enableCoverage() {
  project.compilerOpts.cFlags.others.emplace_back("--coverage");
  project.compilerOpts.ldFlags.others.emplace_back("--coverage");
}

Result<void> BuildConfig::configureBuild() {
  const fs::path srcDir = project.rootPath / "src";
  if (!fs::exists(srcDir)) {
    Bail("{} is required but not found", srcDir);
  }

  const auto isMainSource = [](const fs::path& file) {
    return file.filename().stem() == "main";
  };
  const auto isLibSource = [](const fs::path& file) {
    return file.filename().stem() == "lib";
  };

  fs::path mainSource;
  for (const auto& entry : fs::directory_iterator(srcDir)) {
    const fs::path& path = entry.path();
    if (!SOURCE_FILE_EXTS.contains(path.extension().string())) {
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
    if (!SOURCE_FILE_EXTS.contains(path.extension().string())) {
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

  compileUnits.clear();
  ninjaEdges.clear();
  defaultTargets.clear();
  testTargets.clear();

  cxxFlags = joinFlags(project.compilerOpts.cFlags.others);
  defines = joinFlags(project.compilerOpts.cFlags.macros);
  includes = joinFlags(project.compilerOpts.cFlags.includeDirs);
  const std::string ldOthers = joinFlags(project.compilerOpts.ldFlags.others);
  const std::string libDirs = joinFlags(project.compilerOpts.ldFlags.libDirs);
  ldFlags = combineFlags({ ldOthers, libDirs });
  libs = joinFlags(project.compilerOpts.ldFlags.libs);

  const std::vector<fs::path> sourceFilePaths = listSourceFilePaths(srcDir);
  for (const fs::path& sourceFilePath : sourceFilePaths) {
    if (sourceFilePath != mainSource && isMainSource(sourceFilePath)) {
      Diag::warn(
          "source file `{}` is named `main` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as the program's entry point. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string());
    } else if (sourceFilePath != libSource && isLibSource(sourceFilePath)) {
      Diag::warn(
          "source file `{}` is named `lib` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as a hasLibraryTarget. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string());
    }
  }

  const std::unordered_set<std::string> buildObjTargets =
      Try(processSources(sourceFilePaths));

  if (hasBinaryTarget) {
    const fs::path mainObjPath = project.buildOutPath / "main.o";
    const std::string mainObj =
        fs::relative(mainObjPath, outBasePath).generic_string();
    Ensure(compileUnits.contains(mainObj),
           "internal error: missing compile unit for {}", mainObj);

    std::unordered_set<std::string> deps = { mainObj };
    collectBinDepObjs(deps, "", compileUnits.at(mainObj).dependencies,
                      buildObjTargets);

    std::vector<std::string> inputs(deps.begin(), deps.end());
    std::ranges::sort(inputs);

    NinjaEdge linkEdge;
    linkEdge.outputs = { project.manifest.package.name };
    linkEdge.rule = "cxx_link";
    linkEdge.inputs = std::move(inputs);
    linkEdge.bindings.emplace_back(
        "out_dir", parentDirOrDot(project.manifest.package.name));
    addEdge(std::move(linkEdge));
    defaultTargets.push_back(project.manifest.package.name);
  }

  if (hasLibraryTarget) {
    const fs::path libObjPath = project.buildOutPath / "lib.o";
    const std::string libObj =
        fs::relative(libObjPath, outBasePath).generic_string();
    Ensure(compileUnits.contains(libObj),
           "internal error: missing compile unit for {}", libObj);

    std::unordered_set<std::string> deps = { libObj };
    collectBinDepObjs(deps, "", compileUnits.at(libObj).dependencies,
                      buildObjTargets);

    std::vector<std::string> inputs(deps.begin(), deps.end());
    std::ranges::sort(inputs);

    NinjaEdge archiveEdge;
    archiveEdge.outputs = { libName };
    archiveEdge.rule = "ar_archive";
    archiveEdge.inputs = std::move(inputs);
    archiveEdge.bindings.emplace_back("out_dir", parentDirOrDot(libName));
    addEdge(std::move(archiveEdge));
    defaultTargets.push_back(libName);
  }

  if (buildProfile == BuildProfile::Test) {
    std::vector<TestTarget> discoveredTests;
    discoveredTests.reserve(sourceFilePaths.size());
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      if (auto maybeTarget =
              Try(processUnittestSrc(sourceFilePath, buildObjTargets));
          maybeTarget.has_value()) {
        discoveredTests.push_back(std::move(maybeTarget.value()));
      }
    }

    const fs::path integrationTestDir = project.rootPath / "tests";
    if (fs::exists(integrationTestDir)) {
      const std::vector<fs::path> integrationSources =
          listSourceFilePaths(integrationTestDir);
      for (const fs::path& sourceFilePath : integrationSources) {
        if (auto maybeTarget =
                Try(processIntegrationTestSrc(sourceFilePath, buildObjTargets));
            maybeTarget.has_value()) {
          discoveredTests.push_back(std::move(maybeTarget.value()));
        }
      }
    }

    std::ranges::sort(discoveredTests,
                      [](const TestTarget& lhs, const TestTarget& rhs) {
                        return lhs.ninjaTarget < rhs.ninjaTarget;
                      });
    testTargets = std::move(discoveredTests);
  } else {
    testTargets.clear();
  }

  return Ok();
}

static Result<void> generateCompdb(const fs::path& outDir) {
  Command compdbCmd("ninja");
  compdbCmd.addArg("-C").addArg(outDir.string());
  compdbCmd.addArg("-t").addArg("compdb");
  compdbCmd.addArg("cxx_compile");
  const CommandOutput output = Try(compdbCmd.output());
  Ensure(output.exitStatus.success(), "ninja -t compdb {}", output.exitStatus);

  std::ofstream compdbFile(outDir / "compile_commands.json");
  compdbFile << output.stdOut;
  return Ok();
}

Result<BuildConfig> emitNinja(const Manifest& manifest,
                              const BuildProfile& buildProfile,
                              const bool includeDevDeps,
                              const bool enableCoverage) {
  auto config = Try(BuildConfig::init(manifest, buildProfile));

  Try(config.installDeps(includeDevDeps));
  if (enableCoverage) {
    config.enableCoverage();
  }
  const bool buildProj = !config.ninjaIsUpToDate();
  spdlog::debug("build.ninja is {}up to date", buildProj ? "NOT " : "");

  Try(config.configureBuild());
  if (buildProj) {
    config.writeBuildFiles();
  }
  Try(generateCompdb(config.outBasePath));

  return Ok(config);
}

Result<std::string> emitCompdb(const Manifest& manifest,
                               const BuildProfile& buildProfile,
                               const bool includeDevDeps) {
  auto config = Try(emitNinja(manifest, buildProfile, includeDevDeps,
                              /*enableCoverage=*/false));
  return Ok(config.outBasePath.string());
}

static Command makeNinjaCommand(const bool forDryRun) {
  Command ninjaCommand("ninja");
  if (!isVerbose() && !forDryRun) {
    ninjaCommand.addArg("--quiet");
  } else if (isVeryVerbose()) {
    ninjaCommand.addArg("--verbose");
  }

  const std::size_t numThreads = getParallelism();
  ninjaCommand.addArg(fmt::format("-j{}", numThreads));

  return ninjaCommand;
}

Command getNinjaCommand() { return makeNinjaCommand(false); }

Result<bool> ninjaNeedsWork(const fs::path& outDir,
                            const std::vector<std::string>& targets) {
  Command dryRunCmd = makeNinjaCommand(true);
  dryRunCmd.addArg("-C").addArg(outDir.string()).addArg("-n");
  for (const std::string& target : targets) {
    dryRunCmd.addArg(target);
  }

  const CommandOutput dryRun = Try(dryRunCmd.output());
  static constexpr std::string_view noWorkMsg = "ninja: no work to do.";
  const bool hasNoWork = dryRun.stdOut.contains(noWorkMsg);
  return Ok(!hasNoWork || !dryRun.exitStatus.success());
}

} // namespace cabin

#ifdef CABIN_TEST

#  include "Rustify/Tests.hpp"

namespace tests {

using namespace cabin; // NOLINT(build/namespaces,google-build-using-namespace)

static void testJoinFlags() {
  const std::vector<std::string> flags{ "-Ifoo", "-Ibar" };
  assertEq(joinFlags(flags), "-Ifoo -Ibar");

  const std::vector<std::string> empty;
  assertEq(joinFlags(empty), "");

  pass();
}

static void testCombineFlags() {
  const std::string combined = combineFlags({ "-O2", "", "-fno-rtti", "-g" });
  assertEq(combined, "-O2 -fno-rtti -g");

  pass();
}

static void testParentDirOrDot() {
  assertEq(parentDirOrDot("objs/main.o"), "objs");
  assertEq(parentDirOrDot("main.o"), ".");

  pass();
}

static void testParseMMOutput() {
  const std::string input =
      "main.o: src/main.cc include/foo.hpp include/bar.hpp \\\n"
      " include/baz.hh\n";
  std::string target;
  const auto deps = parseMMOutput(input, target);

  assertEq(target, "main.o");
  assertTrue(deps.contains("include/foo.hpp"));
  assertTrue(deps.contains("include/bar.hpp"));
  assertTrue(deps.contains("include/baz.hh"));

  pass();
}

} // namespace tests

int main() {
  tests::testJoinFlags();
  tests::testCombineFlags();
  tests::testParentDirOrDot();
  tests::testParseMMOutput();
}

#endif
