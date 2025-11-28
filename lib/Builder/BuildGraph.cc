#include "Builder/BuildGraph.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Parallelism.hpp"

#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fmt/core.h>
#include <fmt/format.h>
#include <fmt/ranges.h>
#include <fstream>
#include <iomanip>
#include <map>
#include <nlohmann/json.hpp>
#include <optional>
#include <spdlog/spdlog.h>
#include <sstream>
#include <string>
#include <string_view>
#include <system_error>
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

std::unordered_set<std::string> parseMMOutput(const std::string& mmOutput,
                                              std::string& target) {
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

std::vector<fs::path> listSourceFilePaths(const fs::path& dir) {
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

BuildGraph::BuildGraph(BuildProfile buildProfileIn, std::string libNameIn,
                       Project projectIn, Compiler compilerIn)
    : outBasePath_(projectIn.outBasePath), project(std::move(projectIn)),
      compiler(std::move(compilerIn)), buildProfile_(std::move(buildProfileIn)),
      libName(std::move(libNameIn)), ninjaPlan(outBasePath_) {}

rs::Result<BuildGraph> BuildGraph::create(const Manifest& manifest,
                                          const BuildProfile& buildProfile) {
  std::string libName;
  if (manifest.package.name.starts_with("lib")) {
    libName = fmt::format("{}.a", manifest.package.name);
  } else {
    libName = fmt::format("lib{}.a", manifest.package.name);
  }

  Project project = rs_try(Project::init(buildProfile, manifest));
  return rs::Ok(BuildGraph(buildProfile, std::move(libName), std::move(project),
                           rs_try(Compiler::init())));
}

bool BuildGraph::isUpToDate(const std::string_view fileName) const {
  const fs::path filePath = outBasePath_ / fileName;

  if (!fs::exists(filePath)) {
    return false;
  }

  const fs::file_time_type configTime = fs::last_write_time(filePath);
  const std::array<fs::path, 3> watchedDirs{
    project.rootPath / "src",
    project.rootPath / "lib",
    project.rootPath / "include",
  };
  for (const fs::path& dir : watchedDirs) {
    if (!fs::exists(dir)) {
      continue;
    }
    for (const auto& entry : fs::recursive_directory_iterator(dir)) {
      if (fs::last_write_time(entry.path()) > configTime) {
        return false;
      }
    }
  }
  return fs::last_write_time(project.manifest.path) <= configTime;
}

std::string BuildGraph::mapHeaderToObj(const fs::path& headerPath) const {
  const fs::path objBase = fs::relative(project.buildOutPath, outBasePath_);

  const auto makeObjPath = [&](const fs::path& relDir, const fs::path& prefix) {
    fs::path objPath = objBase;
    if (!prefix.empty()) {
      objPath /= prefix;
    }
    if (!relDir.empty() && relDir != ".") {
      objPath /= relDir;
    }
    objPath /= headerPath.stem();
    objPath += ".o";
    return objPath;
  };

  const auto tryMap =
      [&](const fs::path& rootDir,
          const fs::path& prefix) -> std::optional<std::string> {
    std::error_code ec;
    const fs::path rel = fs::relative(headerPath.parent_path(), rootDir, ec);
    if (ec) {
      return std::nullopt;
    }
    if (!rel.empty()) {
      const auto first = rel.begin();
      if (first != rel.end() && *first == "..") {
        return std::nullopt;
      }
    }
    return makeObjPath(rel, prefix).generic_string();
  };

  if (auto mapped = tryMap(project.rootPath / "src", fs::path());
      mapped.has_value()) {
    return *mapped;
  }
  if (auto mapped = tryMap(project.rootPath / "include", fs::path("lib"));
      mapped.has_value()) {
    return *mapped;
  }
  if (auto mapped = tryMap(project.rootPath / "lib", fs::path("lib"));
      mapped.has_value()) {
    return *mapped;
  }

  fs::path fallback = objBase / headerPath.stem();
  fallback += ".o";
  return fallback.generic_string();
}

void BuildGraph::registerCompileUnit(
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
  ninjaPlan.addEdge(std::move(edge));
}

void BuildGraph::writeBuildFiles() const {
  const NinjaToolchain toolchain{
    .cxx = compiler.cxx,
    .cxxFlags = cxxFlags,
    .defines = defines,
    .includes = includes,
    .ldFlags = ldFlags,
    .libs = libs,
    .archiver = archiver,
  };

  ninjaPlan.writeFiles(toolchain);
}

rs::Result<std::string> BuildGraph::runMM(const std::string& sourceFile,
                                          const bool isTest) const {
  Command command = compiler.makeMMCmd(project.compilerOpts, sourceFile);
  if (isTest) {
    command.addArg("-DCABIN_TEST");
  }
  command.setWorkingDirectory(outBasePath_);
  return getCmdOutput(command);
}

rs::Result<bool>
BuildGraph::containsTestCode(const std::string& sourceFile) const {
  std::ifstream ifs(sourceFile);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.contains("CABIN_TEST")) {
      continue;
    }

    Command command =
        compiler.makePreprocessCmd(project.compilerOpts, sourceFile);
    const std::string src = rs_try(getCmdOutput(command));

    command.addArg("-DCABIN_TEST");
    const std::string testSrc = rs_try(getCmdOutput(command));

    const bool containsTest = src != testSrc;
    if (containsTest) {
      spdlog::trace("Found test code: {}", sourceFile);
    }
    return rs::Ok(containsTest);
  }
  return rs::Ok(false);
}

rs::Result<void>
BuildGraph::processSrc(const fs::path& sourceFilePath, const SourceRoot& root,
                       std::unordered_set<std::string>& buildObjTargets,
                       tbb::spin_mutex* mtx) {
  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(rs_try(runMM(sourceFilePath)), objTarget);

  std::error_code ec;
  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), root.directory, ec);
  rs_ensure(!ec, "failed to compute relative path for {}", sourceFilePath);
  if (!targetBaseDir.empty()) {
    const auto first = targetBaseDir.begin();
    rs_ensure(first == targetBaseDir.end() || *first != "..",
              "source file `{}` must reside under `{}`", sourceFilePath,
              root.directory);
  }
  fs::path buildTargetBaseDir = project.buildOutPath;
  if (!root.objectSubdir.empty()) {
    buildTargetBaseDir /= root.objectSubdir;
  }
  if (targetBaseDir != ".") {
    buildTargetBaseDir /= targetBaseDir;
  }

  const fs::path objOutput = buildTargetBaseDir / objTarget;
  const std::string buildObjTarget =
      fs::relative(objOutput, outBasePath_).generic_string();

  if (mtx) {
    mtx->lock();
  }
  buildObjTargets.insert(buildObjTarget);
  registerCompileUnit(buildObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/false);
  if (mtx) {
    mtx->unlock();
  }
  return rs::Ok();
}

rs::Result<std::unordered_set<std::string>>
BuildGraph::processSources(const std::vector<fs::path>& sourceFilePaths,
                           const SourceRoot& root) {
  std::unordered_set<std::string> buildObjTargets;

  if (isParallel()) {
    tbb::concurrent_vector<std::string> results;
    tbb::spin_mutex mtx;
    tbb::parallel_for(
        tbb::blocked_range<std::size_t>(0, sourceFilePaths.size()),
        [&](const tbb::blocked_range<std::size_t>& rng) {
          for (std::size_t i = rng.begin(); i != rng.end(); ++i) {
            std::ignore =
                processSrc(sourceFilePaths[i], root, buildObjTargets, &mtx)
                    .map_err([&results](const auto& err) {
                      results.push_back(err->what());
                    });
          }
        });
    if (!results.empty()) {
      rs_bail("{}", fmt::join(results, "\n"));
    }
  } else {
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      rs_try(processSrc(sourceFilePath, root, buildObjTargets));
    }
  }
  return rs::Ok(buildObjTargets);
}

rs::Result<std::optional<BuildGraph::TestTarget>>
BuildGraph::processUnittestSrc(const fs::path& sourceFilePath,
                               tbb::spin_mutex* mtx) {
  if (!rs_try(containsTestCode(sourceFilePath))) {
    return rs::Ok(std::optional<TestTarget>());
  }

  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(rs_try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

  fs::path relBase = fs::path("unit");

  const auto canonicalOrGeneric = [](const fs::path& path) {
    std::error_code ec;
    const fs::path canonical = fs::weakly_canonical(path, ec);
    if (ec) {
      return path.lexically_normal().generic_string();
    }
    return canonical.generic_string();
  };

  const std::string canonicalSource = canonicalOrGeneric(sourceFilePath);
  const std::string canonicalSrcRoot =
      canonicalOrGeneric(project.rootPath / "src");
  const std::string canonicalLibRoot =
      canonicalOrGeneric(project.rootPath / "lib");

  const auto setRelBaseFrom = [&](const std::string& baseCanonical,
                                  std::string_view subdir) -> bool {
    if (baseCanonical.empty()) {
      return false;
    }
    if (canonicalSource.size() <= baseCanonical.size()) {
      return false;
    }
    if (!canonicalSource.starts_with(baseCanonical)) {
      return false;
    }
    const char divider = canonicalSource[baseCanonical.size()];
    if (divider != '/') {
      return false;
    }
    const std::string remainder =
        canonicalSource.substr(baseCanonical.size() + 1);
    if (remainder.empty()) {
      return false;
    }

    relBase /= fs::path(subdir);
    const fs::path remainderPath(remainder);
    const fs::path parent = remainderPath.parent_path();
    if (!parent.empty()) {
      relBase /= parent;
    }
    return true;
  };

  bool handled = false;
  bool isSrcUnit = false;
  if (setRelBaseFrom(canonicalSrcRoot, "src")) {
    handled = true;
    isSrcUnit = true;
  } else if (setRelBaseFrom(canonicalLibRoot, "lib")) {
    handled = true;
  }

  if (!handled) {
    std::error_code relRootEc;
    const fs::path relRootParent =
        fs::relative(sourceFilePath.parent_path(), project.rootPath, relRootEc);
    rs_ensure(!relRootEc, "failed to compute relative path for {}",
              sourceFilePath);
    if (relRootParent != "." && !relRootParent.empty()) {
      relBase /= relRootParent;
    }
  }

  const fs::path testObjRel = relBase / objTarget;
  const std::string testObjTarget = testObjRel.generic_string();

  fs::path testBinaryRel = relBase / sourceFilePath.filename();
  testBinaryRel += ".test";
  const std::string testBinary = testBinaryRel.generic_string();

  if (mtx) {
    mtx->lock();
  }
  registerCompileUnit(testObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/true);
  if (mtx) {
    mtx->unlock();
  }

  std::vector<std::string> linkInputs;
  linkInputs.push_back(testObjTarget);

  if (isSrcUnit) {
    std::unordered_set<std::string> deps;
    collectBinDepObjs(deps, sourceFilePath.stem().string(), objTargetDeps,
                      srcObjectTargets);

    std::vector<std::string> srcDeps(deps.begin(), deps.end());
    std::ranges::sort(srcDeps);
    linkInputs.insert(linkInputs.end(), srcDeps.begin(), srcDeps.end());
  }

  if (hasLibraryTarget_) {
    linkInputs.push_back(libName);
  }

  NinjaEdge linkEdge;
  linkEdge.outputs = { testBinary };
  linkEdge.rule = "cxx_link_exe";
  linkEdge.inputs = std::move(linkInputs);
  linkEdge.bindings.emplace_back("out_dir", parentDirOrDot(testBinary));

  ninjaPlan.addEdge(std::move(linkEdge));

  TestTarget testTarget;
  testTarget.ninjaTarget = testBinary;
  testTarget.sourcePath =
      fs::relative(sourceFilePath, project.rootPath).generic_string();
  testTarget.kind = TestKind::Unit;

  return rs::Ok(std::optional<TestTarget>(std::move(testTarget)));
}

rs::Result<std::optional<BuildGraph::TestTarget>>
BuildGraph::processIntegrationTestSrc(const fs::path& sourceFilePath,
                                      tbb::spin_mutex* mtx) {
  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(rs_try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), project.rootPath / "tests");
  fs::path testTargetBaseDir = project.integrationTestOutPath;
  if (targetBaseDir != ".") {
    testTargetBaseDir /= targetBaseDir;
  }

  const fs::path testObjOutput = testTargetBaseDir / objTarget;
  const std::string testObjTarget =
      fs::relative(testObjOutput, outBasePath_).generic_string();
  const fs::path testBinaryPath = testTargetBaseDir / sourceFilePath.stem();
  const std::string testBinary =
      fs::relative(testBinaryPath, outBasePath_).generic_string();

  std::vector<std::string> linkInputs{ testObjTarget };
  if (hasLibraryTarget_) {
    linkInputs.push_back(libName);
  }
  std::ranges::sort(linkInputs);

  NinjaEdge linkEdge;
  linkEdge.outputs = { testBinary };
  linkEdge.rule = "cxx_link_exe";
  linkEdge.inputs = std::move(linkInputs);
  linkEdge.bindings.emplace_back("out_dir", parentDirOrDot(testBinary));

  if (mtx) {
    mtx->lock();
  }
  registerCompileUnit(testObjTarget, sourceFilePath.string(), objTargetDeps,
                      /*isTest=*/true);
  ninjaPlan.addEdge(std::move(linkEdge));
  if (mtx) {
    mtx->unlock();
  }

  TestTarget testTarget;
  testTarget.ninjaTarget = testBinary;
  testTarget.sourcePath =
      fs::relative(sourceFilePath, project.rootPath).generic_string();
  testTarget.kind = TestKind::Integration;

  return rs::Ok(std::optional<TestTarget>(std::move(testTarget)));
}

void BuildGraph::collectBinDepObjs(
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

rs::Result<void> BuildGraph::installDeps(const bool includeDevDeps) {
  const std::vector<CompilerOpts> depsCompOpts =
      rs_try(project.manifest.installDeps(includeDevDeps));

  for (const CompilerOpts& depOpts : depsCompOpts) {
    project.compilerOpts.merge(depOpts);
  }
  return rs::Ok();
}

void BuildGraph::enableCoverage() {
  project.compilerOpts.cFlags.others.emplace_back("--coverage");
  project.compilerOpts.ldFlags.others.emplace_back("--coverage");
}

rs::Result<void> BuildGraph::configure() {
  const fs::path srcDir = project.rootPath / "src";
  const bool hasSrcDir = fs::exists(srcDir);
  const fs::path libDir = project.rootPath / "lib";

  const Profile& profile = project.manifest.profiles.at(buildProfile_);
  archiver = compiler.detectArchiver(profile.lto);

  hasBinaryTarget_ = false;
  hasLibraryTarget_ = false;

  const auto isMainSource = [](const fs::path& file) {
    return file.filename().stem() == "main";
  };

  fs::path mainSource;
  if (hasSrcDir) {
    for (const auto& entry : fs::directory_iterator(srcDir)) {
      const fs::path& path = entry.path();
      if (!SOURCE_FILE_EXTS.contains(path.extension().string())) {
        continue;
      }
      if (!isMainSource(path)) {
        continue;
      }
      if (!mainSource.empty()) {
        rs_bail("multiple main sources were found");
      }
      mainSource = path;
      hasBinaryTarget_ = true;
    }
  }

  if (!fs::exists(outBasePath_)) {
    fs::create_directories(outBasePath_);
  }

  compileUnits.clear();
  ninjaPlan.reset();
  testTargets_.clear();

  cxxFlags = joinFlags(project.compilerOpts.cFlags.others);
  defines = joinFlags(project.compilerOpts.cFlags.macros);
  includes = joinFlags(project.compilerOpts.cFlags.includeDirs);
  const std::string ldOthers = joinFlags(project.compilerOpts.ldFlags.others);
  const std::string libDirs = joinFlags(project.compilerOpts.ldFlags.libDirs);
  ldFlags = combineFlags({ ldOthers, libDirs });
  libs = joinFlags(project.compilerOpts.ldFlags.libs);

  std::vector<fs::path> sourceFilePaths;
  if (hasSrcDir) {
    sourceFilePaths = listSourceFilePaths(srcDir);
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      if (sourceFilePath != mainSource && isMainSource(sourceFilePath)) {
        Diag::warn(
            "source file `{}` is named `main` but is not located directly in "
            "the `src/` directory. "
            "This file will not be treated as the program's entry point. "
            "Move it directly to 'src/' if intended as such.",
            sourceFilePath.string());
      }
    }
  }

  std::vector<fs::path> publicSourceFilePaths;
  if (fs::exists(libDir)) {
    publicSourceFilePaths = listSourceFilePaths(libDir);
  }
  hasLibraryTarget_ = !publicSourceFilePaths.empty();

  if (!hasBinaryTarget_ && !hasLibraryTarget_) {
    rs_bail("expected either `src/main{}` or at least one source file under "
            "`lib/` matching {}",
            SOURCE_FILE_EXTS, SOURCE_FILE_EXTS);
  }

  const SourceRoot srcRoot(srcDir);
  const SourceRoot libRoot(libDir, fs::path("lib"));

  const std::unordered_set<std::string> srcObjTargets =
      rs_try(processSources(sourceFilePaths, srcRoot));
  srcObjectTargets = srcObjTargets;
  std::erase_if(srcObjectTargets, [](const std::string& obj) {
    return obj == "main.o" || obj.ends_with("/main.o");
  });

  std::unordered_set<std::string> libObjTargets;
  if (!publicSourceFilePaths.empty()) {
    libObjTargets = rs_try(processSources(publicSourceFilePaths, libRoot));
  }

  std::unordered_set<std::string> buildObjTargets = srcObjTargets;
  buildObjTargets.insert(libObjTargets.begin(), libObjTargets.end());

  if (hasBinaryTarget_) {
    const fs::path mainObjPath = project.buildOutPath / "main.o";
    const std::string mainObj =
        fs::relative(mainObjPath, outBasePath_).generic_string();
    rs_ensure(compileUnits.contains(mainObj),
              "internal error: missing compile unit for {}", mainObj);

    std::unordered_set<std::string> deps = { mainObj };
    collectBinDepObjs(deps, "", compileUnits.at(mainObj).dependencies,
                      buildObjTargets);

    std::vector<std::string> inputs;
    if (hasLibraryTarget_) {
      deps.erase(mainObj);
      std::vector<std::string> srcInputs;
      srcInputs.reserve(deps.size());
      for (const std::string& dep : deps) {
        if (libObjTargets.contains(dep)) {
          continue;
        }
        srcInputs.push_back(dep);
      }
      std::ranges::sort(srcInputs);

      inputs.push_back(mainObj);
      inputs.insert(inputs.end(), srcInputs.begin(), srcInputs.end());
      inputs.push_back(libName);
    } else {
      inputs.assign(deps.begin(), deps.end());
      std::ranges::sort(inputs);
    }

    NinjaEdge linkEdge;
    linkEdge.outputs = { project.manifest.package.name };
    linkEdge.rule = "cxx_link_exe";
    linkEdge.inputs = std::move(inputs);
    linkEdge.bindings.emplace_back(
        "out_dir", parentDirOrDot(project.manifest.package.name));
    ninjaPlan.addEdge(std::move(linkEdge));
    ninjaPlan.addDefaultTarget(project.manifest.package.name);
  }

  if (hasLibraryTarget_) {
    std::vector<std::string> libraryInputs;
    libraryInputs.reserve(libObjTargets.size());
    for (const std::string& obj : libObjTargets) {
      libraryInputs.push_back(obj);
    }

    rs_ensure(!libraryInputs.empty(),
              "internal error: expected objects for library target");
    std::ranges::sort(libraryInputs);

    NinjaEdge archiveEdge;
    archiveEdge.outputs = { libName };
    archiveEdge.rule = "cxx_link_static_lib";
    archiveEdge.inputs = std::move(libraryInputs);
    archiveEdge.bindings.emplace_back("out_dir", parentDirOrDot(libName));
    ninjaPlan.addEdge(std::move(archiveEdge));
    ninjaPlan.addDefaultTarget(libName);
  }

  if (buildProfile_ == BuildProfile::Test) {
    std::vector<TestTarget> discoveredTests;
    discoveredTests.reserve(sourceFilePaths.size());
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      if (auto maybeTarget = rs_try(processUnittestSrc(sourceFilePath));
          maybeTarget.has_value()) {
        discoveredTests.push_back(std::move(maybeTarget.value()));
      }
    }

    for (const fs::path& sourceFilePath : publicSourceFilePaths) {
      if (auto maybeTarget = rs_try(processUnittestSrc(sourceFilePath));
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
                rs_try(processIntegrationTestSrc(sourceFilePath));
            maybeTarget.has_value()) {
          discoveredTests.push_back(std::move(maybeTarget.value()));
        }
      }
    }

    std::ranges::sort(discoveredTests,
                      [](const TestTarget& lhs, const TestTarget& rhs) {
                        return lhs.ninjaTarget < rhs.ninjaTarget;
                      });
    testTargets_ = std::move(discoveredTests);

    std::vector<std::string> testTargetNames;
    testTargetNames.reserve(testTargets_.size());
    for (const TestTarget& target : testTargets_) {
      testTargetNames.push_back(target.ninjaTarget);
    }
    ninjaPlan.setTestTargets(std::move(testTargetNames));
  } else {
    testTargets_.clear();
    ninjaPlan.setTestTargets({});
  }

  return rs::Ok();
}

rs::Result<void> BuildGraph::writeBuildFilesIfNeeded() const {
  if (isUpToDate("build.ninja")) {
    return rs::Ok();
  }
  writeBuildFiles();
  return rs::Ok();
}

rs::Result<void> BuildGraph::generateCompdb() const {
  const fs::path& outDir = outBasePath_;
  const fs::path cabinOutRoot = outDir.parent_path();

  std::vector<fs::path> buildDirs{ outDir };
  if (fs::exists(cabinOutRoot) && fs::is_directory(cabinOutRoot)) {
    for (const auto& entry : fs::directory_iterator(cabinOutRoot)) {
      if (!entry.is_directory()) {
        continue;
      }
      const fs::path& path = entry.path();
      if (fs::exists(path / "build.ninja")) {
        buildDirs.push_back(path);
      }
    }
  }

  std::ranges::sort(buildDirs);
  const auto uniqueResult = std::ranges::unique(buildDirs);
  buildDirs.erase(uniqueResult.begin(), uniqueResult.end());

  std::map<std::pair<std::string, std::string>, nlohmann::json> entries;

  for (const fs::path& buildDir : buildDirs) {
    if (!fs::exists(buildDir / "build.ninja")) {
      continue;
    }

    Command compdbCmd("ninja");
    compdbCmd.addArg("-C").addArg(buildDir.string());
    compdbCmd.addArg("-t").addArg("compdb");
    compdbCmd.addArg("cxx_compile");
    const CommandOutput output = rs_try(compdbCmd.output());
    rs_ensure(output.exitStatus.success(), "ninja -t compdb {}",
              output.exitStatus);

    nlohmann::json json;
    try {
      json = nlohmann::json::parse(output.stdOut);
    } catch (const nlohmann::json::parse_error& e) {
      rs_bail("failed to parse ninja -t compdb output: {}", e.what());
    }
    rs_ensure(json.is_array(), "invalid compdb output");
    for (auto& entry : json) {
      const auto directory = entry.value("directory", std::string_view{});
      const auto file = entry.value("file", std::string_view{});
      if (!directory.empty() && !file.empty()) {
        entries[std::make_pair(std::string(directory), std::string(file))] =
            entry;
      }
    }
  }

  nlohmann::json combined = nlohmann::json::array();
  for (auto& [_, entry] : entries) {
    combined.push_back(std::move(entry));
  }

  fs::create_directories(cabinOutRoot);
  std::ofstream aggregateFile(cabinOutRoot / "compile_commands.json");
  aggregateFile << combined.dump(2) << '\n';

  return rs::Ok();
}

rs::Result<void> BuildGraph::plan(const bool logAnalysis) {
  if (logAnalysis) {
    Diag::info("Analyzing", "project dependencies...");
  }

  const bool buildProj = !isUpToDate("build.ninja");
  spdlog::debug("build.ninja is {}up to date", buildProj ? "NOT " : "");

  rs_try(configure());
  if (buildProj) {
    writeBuildFiles();
  }
  rs_try(generateCompdb());

  return rs::Ok();
}

// NOLINTNEXTLINE(readability-convert-member-functions-to-static)
Command BuildGraph::ninjaCommand(const bool forDryRun) const {
  Command ninja("ninja");
  if (!isVerbose() && !forDryRun) {
    ninja.addArg("--quiet");
  } else if (isVeryVerbose()) {
    ninja.addArg("--verbose");
  }

  const std::size_t numThreads = getParallelism();
  ninja.addArg(fmt::format("-j{}", numThreads));

  return ninja;
}

rs::Result<bool>
BuildGraph::needsBuild(const std::vector<std::string>& targets) const {
  Command dryRunCmd = ninjaCommand(true);
  dryRunCmd.addArg("-C").addArg(outBasePath_.string()).addArg("-n");
  for (const std::string& target : targets) {
    dryRunCmd.addArg(target);
  }

  const CommandOutput dryRun = rs_try(dryRunCmd.output());
  static constexpr std::string_view noWorkMsg = "ninja: no work to do.";
  const bool hasNoWork = dryRun.stdOut.contains(noWorkMsg);
  return rs::Ok(!hasNoWork || !dryRun.exitStatus.success());
}

rs::Result<ExitStatus>
BuildGraph::buildTargets(const std::vector<std::string>& targets,
                         const std::string_view displayName) const {
  Command buildCmd = ninjaCommand(false);
  buildCmd.addArg("-C").addArg(outBasePath_.string());
  for (const std::string& target : targets) {
    buildCmd.addArg(target);
  }

  if (rs_try(needsBuild(targets))) {
    Diag::info("Compiling", "{} v{} ({})", displayName,
               project.manifest.package.version.toString(),
               project.manifest.path.parent_path().string());
  }

  return execCmd(buildCmd);
}

} // namespace cabin
