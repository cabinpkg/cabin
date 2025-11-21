#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Builder/Compiler.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Manifest.hpp"
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
#include <ostream>
#include <queue>
#include <ranges>
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

std::string BuildConfig::mapHeaderToObj(const fs::path& headerPath) const {
  const fs::path objBase = fs::relative(project.buildOutPath, outBasePath);

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
  ninjaPlan.addEdge(std::move(edge));
}

void BuildConfig::writeBuildFiles() const {
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
BuildConfig::processSrc(const fs::path& sourceFilePath, const SourceRoot& root,
                        std::unordered_set<std::string>& buildObjTargets,
                        tbb::spin_mutex* mtx) {
  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath)), objTarget);

  std::error_code ec;
  const fs::path targetBaseDir =
      fs::relative(sourceFilePath.parent_path(), root.directory, ec);
  Ensure(!ec, "failed to compute relative path for {}", sourceFilePath);
  if (!targetBaseDir.empty()) {
    const auto first = targetBaseDir.begin();
    Ensure(first == targetBaseDir.end() || *first != "..",
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
BuildConfig::processSources(const std::vector<fs::path>& sourceFilePaths,
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
      Bail("{}", fmt::join(results, "\n"));
    }
  } else {
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      Try(processSrc(sourceFilePath, root, buildObjTargets));
    }
  }
  return Ok(buildObjTargets);
}

Result<std::optional<BuildConfig::TestTarget>>
BuildConfig::processUnittestSrc(const fs::path& sourceFilePath,
                                tbb::spin_mutex* mtx) {
  if (!Try(containsTestCode(sourceFilePath))) {
    return Ok(std::optional<TestTarget>());
  }

  std::string objTarget;
  const std::unordered_set<std::string> objTargetDeps =
      parseMMOutput(Try(runMM(sourceFilePath, /*isTest=*/true)), objTarget);

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
    Ensure(!relRootEc, "failed to compute relative path for {}",
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

  if (hasLibraryTarget) {
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

  return Ok(std::optional<TestTarget>(std::move(testTarget)));
}

Result<std::optional<BuildConfig::TestTarget>>
BuildConfig::processIntegrationTestSrc(const fs::path& sourceFilePath,
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

  std::vector<std::string> linkInputs{ testObjTarget };
  if (hasLibraryTarget) {
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
  const bool hasSrcDir = fs::exists(srcDir);
  const fs::path libDir = project.rootPath / "lib";

  const Profile& profile = project.manifest.profiles.at(buildProfile);
  archiver = compiler.detectArchiver(profile.lto);

  hasBinaryTarget = false;
  hasLibraryTarget = false;

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
        Bail("multiple main sources were found");
      }
      mainSource = path;
      hasBinaryTarget = true;
    }
  }

  if (!fs::exists(outBasePath)) {
    fs::create_directories(outBasePath);
  }

  compileUnits.clear();
  ninjaPlan.reset();
  testTargets.clear();

  cxxFlags = joinFlags(project.compilerOpts.cFlags.others);
  defines = joinFlags(project.compilerOpts.cFlags.macros);
  includes = joinFlags(project.compilerOpts.cFlags.includeDirs);
  const std::string ldOthers = joinFlags(project.compilerOpts.ldFlags.others);
  const std::string libDirs = joinFlags(project.compilerOpts.ldFlags.libDirs);
  ldFlags = combineFlags({ ldOthers, libDirs });
  libs = joinFlags(project.compilerOpts.ldFlags.libs);

  const std::vector<fs::path> sourceFilePaths =
      hasSrcDir ? listSourceFilePaths(srcDir) : std::vector<fs::path>{};
  for (const fs::path& sourceFilePath : sourceFilePaths) {
    if (sourceFilePath != mainSource && isMainSource(sourceFilePath)) {
      Diag::warn(
          "source file `{}` is named `main` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as the program's entry point. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string());
    }
  }

  std::vector<fs::path> publicSourceFilePaths;
  if (fs::exists(libDir)) {
    publicSourceFilePaths = listSourceFilePaths(libDir);
  }
  hasLibraryTarget = !publicSourceFilePaths.empty();

  if (!hasBinaryTarget && !hasLibraryTarget) {
    Bail("expected either `src/main{}` or at least one source file under "
         "`lib/` matching {}",
         SOURCE_FILE_EXTS, SOURCE_FILE_EXTS);
  }

  const SourceRoot srcRoot(srcDir);
  const SourceRoot libRoot(libDir, fs::path("lib"));

  const std::unordered_set<std::string> srcObjTargets =
      Try(processSources(sourceFilePaths, srcRoot));
  srcObjectTargets = srcObjTargets;
  std::erase_if(srcObjectTargets, [](const std::string& obj) {
    return obj == "main.o" || obj.ends_with("/main.o");
  });

  std::unordered_set<std::string> libObjTargets;
  if (!publicSourceFilePaths.empty()) {
    libObjTargets = Try(processSources(publicSourceFilePaths, libRoot));
  }

  std::unordered_set<std::string> buildObjTargets = srcObjTargets;
  buildObjTargets.insert(libObjTargets.begin(), libObjTargets.end());

  if (hasBinaryTarget) {
    const fs::path mainObjPath = project.buildOutPath / "main.o";
    const std::string mainObj =
        fs::relative(mainObjPath, outBasePath).generic_string();
    Ensure(compileUnits.contains(mainObj),
           "internal error: missing compile unit for {}", mainObj);

    std::unordered_set<std::string> deps = { mainObj };
    collectBinDepObjs(deps, "", compileUnits.at(mainObj).dependencies,
                      buildObjTargets);

    std::vector<std::string> inputs;
    if (hasLibraryTarget) {
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

  if (hasLibraryTarget) {
    std::vector<std::string> libraryInputs;
    libraryInputs.reserve(libObjTargets.size());
    for (const std::string& obj : libObjTargets) {
      libraryInputs.push_back(obj);
    }

    Ensure(!libraryInputs.empty(),
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

  if (buildProfile == BuildProfile::Test) {
    std::vector<TestTarget> discoveredTests;
    discoveredTests.reserve(sourceFilePaths.size());
    for (const fs::path& sourceFilePath : sourceFilePaths) {
      if (auto maybeTarget = Try(processUnittestSrc(sourceFilePath));
          maybeTarget.has_value()) {
        discoveredTests.push_back(std::move(maybeTarget.value()));
      }
    }

    for (const fs::path& sourceFilePath : publicSourceFilePaths) {
      if (auto maybeTarget = Try(processUnittestSrc(sourceFilePath));
          maybeTarget.has_value()) {
        discoveredTests.push_back(std::move(maybeTarget.value()));
      }
    }

    const fs::path integrationTestDir = project.rootPath / "tests";
    if (fs::exists(integrationTestDir)) {
      const std::vector<fs::path> integrationSources =
          listSourceFilePaths(integrationTestDir);
      for (const fs::path& sourceFilePath : integrationSources) {
        if (auto maybeTarget = Try(processIntegrationTestSrc(sourceFilePath));
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

    std::vector<std::string> testTargetNames;
    testTargetNames.reserve(testTargets.size());
    for (const TestTarget& target : testTargets) {
      testTargetNames.push_back(target.ninjaTarget);
    }
    ninjaPlan.setTestTargets(std::move(testTargetNames));
  } else {
    testTargets.clear();
    ninjaPlan.setTestTargets({});
  }

  return Ok();
}

static Result<void> generateCompdb(const fs::path& outDir) {
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
    const CommandOutput output = Try(compdbCmd.output());
    Ensure(output.exitStatus.success(), "ninja -t compdb {}",
           output.exitStatus);

    nlohmann::json json;
    try {
      json = nlohmann::json::parse(output.stdOut);
    } catch (const nlohmann::json::parse_error& e) {
      Bail("failed to parse ninja -t compdb output: {}", e.what());
    }
    Ensure(json.is_array(), "invalid compdb output");
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

  return Ok();
}

Result<BuildConfig> emitNinja(const Manifest& manifest,
                              const BuildProfile& buildProfile,
                              const bool includeDevDeps,
                              const bool enableCoverage) {
  Diag::info("Analyzing", "project dependencies...");

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
  return Ok(config.outBasePath.parent_path().string());
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
