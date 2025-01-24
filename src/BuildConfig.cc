#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Git2.hpp"
#include "Logger.hpp"
#include "Manifest.hpp"
#include "Parallelism.hpp"
#include "Semver.hpp"
#include "TermColor.hpp"

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
BuildConfig::init(const Manifest& manifest, const bool isDebug) {
  std::string libName;
  if (manifest.package.name.starts_with("lib")) {
    libName = fmt::format("{}.a", manifest.package.name);
  } else {
    libName = fmt::format("lib{}.a", manifest.package.name);
  }

  fs::path outBasePath;
  const fs::path projectBasePath = manifest.path.parent_path();
  if (isDebug) {
    outBasePath = projectBasePath / "cabin-out" / "debug";
  } else {
    outBasePath = projectBasePath / "cabin-out" / "release";
  }
  fs::path buildOutPath = outBasePath / (manifest.package.name + ".d");
  fs::path unittestOutPath = outBasePath / "unittests";

  std::vector<std::string> includes;
  fs::path projectIncludePath = projectBasePath / "include";
  if (fs::exists(projectIncludePath)) {
    includes.push_back("-I" + projectIncludePath.string());
  }

  std::string cxx;
  if (const char* cxxP = std::getenv("CXX")) {
    cxx = cxxP;
  } else {
    const std::string output = Try(Command("make")
                                       .addArg("--print-data-base")
                                       .addArg("--question")
                                       .addArg("-f")
                                       .addArg("/dev/null")
                                       .setStdErrConfig(Command::IOConfig::Null)
                                       .output())
                                   .stdOut;
    std::istringstream iss(output);
    std::string line;

    bool cxxFound = false;
    while (std::getline(iss, line)) {
      if (line.starts_with("CXX = ")) {
        cxxFound = true;
        cxx = line.substr("CXX = "sv.size());
        break;
      }
    }
    Ensure(cxxFound, "failed to get CXX from make");
  }

  return Ok(BuildConfig(
      manifest, isDebug, std::move(libName), std::move(outBasePath),
      std::move(buildOutPath), std::move(unittestOutPath), std::move(cxx),
      std::move(includes)
  ));
}

// Generally split the string by space character, but it will properly interpret
// the quotes and some escape sequences. (More specifically it will ignore
// whatever character that goes after a backslash and preserve all characters,
// usually used to pass an argument containing spaces, between quotes.)
static std::vector<std::string>
parseEnvFlags(std::string_view env) {
  std::vector<std::string> result;
  std::string buffer;

  bool foundBackslash = false;
  bool isInQuote = false;
  char quoteChar = ' ';

  for (const char c : env) {
    if (foundBackslash) {
      buffer += c;
      foundBackslash = false;
    } else if (isInQuote) {
      // Backslashes in quotes should still be processed.
      if (c == '\\') {
        foundBackslash = true;
      } else if (c == quoteChar) {
        isInQuote = false;
      } else {
        buffer += c;
      }
    } else if (c == '\'' || c == '"') {
      isInQuote = true;
      quoteChar = c;
    } else if (c == '\\') {
      foundBackslash = true;
    } else if (std::isspace(c)) {
      // Add argument only if necessary (i.e. buffer is not empty)
      if (!buffer.empty()) {
        result.push_back(buffer);
        buffer.clear();
      }
      // Otherwise just ignore the character. Notice that the two conditions
      // cannot be combined into just one else-if branch because that will cause
      // extra spaces to appear in the result.
    } else {
      buffer += c;
    }
  }

  // Append the buffer if it's not empty (happens when no spaces at the end of
  // string).
  if (!buffer.empty()) {
    result.push_back(buffer);
  }

  return result;
}

static std::vector<std::string>
getEnvFlags(const char* name) {
  if (const char* env = std::getenv(name)) {
    return parseEnvFlags(env);
  }
  return {};
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
    const std::string file =
        fs::relative(targetInfo.sourceFile.value(), directory);
    // The output is the target.
    const std::string output = fs::relative(target, directory);
    const Command cmd = Command(cxx)
                            .addArgs(cxxflags)
                            .addArgs(defines)
                            .addArg("-DCABIN_TEST")
                            .addArgs(includes)
                            .addArg("-c")
                            .addArg(file)
                            .addArg("-o")
                            .addArg(output);

    oss << indent1 << "{\n";
    oss << indent2 << "\"directory\": " << directory << ",\n";
    oss << indent2 << "\"file\": " << std::quoted(file) << ",\n";
    oss << indent2 << "\"output\": " << std::quoted(output) << ",\n";
    oss << indent2 << "\"command\": " << std::quoted(cmd.toString()) << "\n";
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
  Command command =
      Command(cxx).addArgs(cxxflags).addArgs(defines).addArgs(includes);
  if (isTest) {
    command.addArg("-DCABIN_TEST");
  }
  command.addArg("-MM");
  command.addArg(sourceFile);
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
      // TODO: Can't we somehow elegantly make the compiler command sharable?
      Command command(cxx);
      command.addArg("-E");
      command.addArgs(cxxflags);
      command.addArgs(defines);
      command.addArgs(includes);
      command.addArg(sourceFile);

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
        logger::trace("Found test code: {}", sourceFile);
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

    const std::string objTarget = mapHeaderToObj(headerPath, buildOutPath);
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
  const std::vector<DepMetadata> deps =
      Try(manifest.installDeps(includeDevDeps));
  for (const DepMetadata& dep : deps) {
    if (!dep.includes.empty()) {
      includes.push_back(replaceAll(dep.includes, "-I", "-isystem"));
    }
    if (!dep.libs.empty()) {
      libs.push_back(dep.libs);
    }
  }
  logger::trace("INCLUDES: {}", includes);
  logger::trace("LIBS: {}", libs);
  return Ok();
}

void
BuildConfig::addDefine(
    const std::string_view name, const std::string_view value
) {
  defines.push_back(fmt::format("-D{}='\"{}\"'", name, value));
}

void
BuildConfig::setVariables() {
  this->defineSimpleVar("CXX", cxx);

  cxxflags.push_back("-std=c++" + manifest.package.edition.str);
  if (shouldColorStderr()) {
    cxxflags.emplace_back("-fdiagnostics-color");
  }

  const Profile& profile =
      isDebug ? manifest.profiles.at("dev") : manifest.profiles.at("release");
  // TODO: profile.debug and optLevel never become std::nullopt via
  // getDevProfile and getReleaseProfile.  This is not intuitive.  We should
  // fix the implementation and struct design.
  if (profile.debug) {
    cxxflags.emplace_back("-g");
    cxxflags.emplace_back("-DDEBUG");
  } else {
    cxxflags.emplace_back("-DNDEBUG");
  }
  cxxflags.emplace_back(fmt::format("-O{}", profile.optLevel));
  if (profile.lto) {
    cxxflags.emplace_back("-flto");
  }
  for (const std::string& flag : profile.cxxflags) {
    cxxflags.emplace_back(flag);
  }

  // Environment variables takes the highest precedence and will be appended at
  // last.
  for (const std::string& flag : getEnvFlags("CXXFLAGS")) {
    cxxflags.emplace_back(flag);
  }
  defineSimpleVar("CXXFLAGS", fmt::format("{:s}", fmt::join(cxxflags, " ")));

  const std::string pkgName = toMacroName(manifest.package.name);
  const Version& pkgVersion = manifest.package.version;
  std::string commitHash;
  std::string commitShortHash;
  std::string commitDate;
  try {
    git2::Repository repo{};
    repo.open(".");

    const git2::Oid oid = repo.refNameToId("HEAD");
    commitHash = oid.toString();
    commitShortHash = commitHash.substr(0, git2::SHORT_HASH_LEN);
    commitDate = git2::Commit().lookup(repo, oid).time().toString();
  } catch (const git2::Exception& e) {
    logger::trace("No git repository found");
  }

  // Variables Cabin sets for the user.
  const std::vector<std::pair<std::string, std::string>> defines{
    { fmt::format("CABIN_{}_PKG_NAME", pkgName), manifest.package.name },
    { fmt::format("CABIN_{}_PKG_VERSION", pkgName), pkgVersion.toString() },
    { fmt::format("CABIN_{}_PKG_VERSION_MAJOR", pkgName),
      std::to_string(pkgVersion.major) },
    { fmt::format("CABIN_{}_PKG_VERSION_MINOR", pkgName),
      std::to_string(pkgVersion.minor) },
    { fmt::format("CABIN_{}_PKG_VERSION_PATCH", pkgName),
      std::to_string(pkgVersion.patch) },
    { fmt::format("CABIN_{}_PKG_VERSION_PRE", pkgName),
      pkgVersion.pre.toString() },
    { fmt::format("CABIN_{}_COMMIT_HASH", pkgName), commitHash },
    { fmt::format("CABIN_{}_COMMIT_SHORT_HASH", pkgName), commitShortHash },
    { fmt::format("CABIN_{}_COMMIT_DATE", pkgName), commitDate },
    { fmt::format("CABIN_{}_PROFILE", pkgName),
      std::string(modeToString(isDebug)) },
  };
  for (const auto& [key, val] : defines) {
    addDefine(key, val);
  }

  defineSimpleVar(
      "DEFINES", fmt::format("{:s}", fmt::join(this->defines, " "))
  );
  defineSimpleVar("INCLUDES", fmt::format("{:s}", fmt::join(includes, " ")));

  // LDFLAGS from manifest
  for (const std::string& flag : profile.ldflags) {
    ldflags.emplace_back(flag);
  }
  // Environment variables takes the highest precedence and will be appended at
  // last.
  for (const std::string& flag : getEnvFlags("LDFLAGS")) {
    ldflags.push_back(flag);
  }
  defineSimpleVar("LDFLAGS", fmt::format("{:s}", fmt::join(ldflags, " ")));

  defineSimpleVar("LIBS", fmt::format("{:s}", fmt::join(libs, " ")));
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
  fs::path buildTargetBaseDir = buildOutPath;
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
  fs::path testTargetBaseDir = unittestOutPath;
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
      logger::warn(
          "source file `{}` is named `main` but is not located directly in the "
          "`src/` directory. "
          "This file will not be treated as the program's entry point. "
          "Move it directly to 'src/' if intended as such.",
          sourceFilePath.string()
      );
    } else if (sourceFilePath != libSource && isLibSource(sourceFilePath)) {
      logger::warn(
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
        buildObjTargets, buildOutPath / "main.o", commands,
        outBasePath / manifest.package.name
    );
  }

  if (hasLibraryTarget) {
    const std::vector<std::string> commands = { ARCHIVE_LIB_COMMAND };
    defineOutputTarget(
        buildObjTargets, buildOutPath / "lib.o", commands, outBasePath / libName
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
    const Manifest& manifest, const bool isDebug, const bool includeDevDeps
) {
  const Profile& profile =
      isDebug ? manifest.profiles.at("dev") : manifest.profiles.at("release");
  auto config = Try(BuildConfig::init(manifest, isDebug));

  // When emitting Makefile, we also build the project.  So, we need to
  // make sure the dependencies are installed.
  Try(config.installDeps(includeDevDeps));

  bool buildProj = false;
  bool buildCompDb = false;
  if (config.makefileIsUpToDate()) {
    logger::debug("Makefile is up to date");
  } else {
    logger::debug("Makefile is NOT up to date");
    buildProj = true;
  }
  if (profile.compDb) {
    if (config.compdbIsUpToDate()) {
      logger::debug("compile_commands.json is up to date");
    } else {
      logger::debug("compile_commands.json is NOT up to date");
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
    const Manifest& manifest, const bool isDebug, const bool includeDevDeps
) {
  auto config = Try(BuildConfig::init(manifest, isDebug));

  // compile_commands.json also needs INCLUDES, but not LIBS.
  Try(config.installDeps(includeDevDeps));

  if (config.compdbIsUpToDate()) {
    logger::debug("compile_commands.json is up to date");
    return Ok(config.outBasePath);
  }
  logger::debug("compile_commands.json is NOT up to date");

  Try(config.configureBuild());
  std::ofstream ofs(config.outBasePath / "compile_commands.json");
  config.emitCompdb(ofs);
  return Ok(config.outBasePath);
}

std::string_view
modeToString(const bool isDebug) {
  return isDebug ? "debug" : "release";
}

std::string_view
modeToProfile(const bool isDebug) {
  return isDebug ? "dev" : "release";
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

static void
testParseEnvFlags() {
  std::vector<std::string> argsNoEscape = parseEnvFlags(" a   b c ");
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsNoEscape.size(), static_cast<std::size_t>(3));
  assertEq(argsNoEscape[0], "a");
  assertEq(argsNoEscape[1], "b");
  assertEq(argsNoEscape[2], "c");

  std::vector<std::string> argsEscapeBackslash =
      parseEnvFlags(R"(  a\ bc   cd\$fg  hi windows\\path\\here  )");
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsEscapeBackslash.size(), static_cast<std::size_t>(4));
  assertEq(argsEscapeBackslash[0], "a bc");
  assertEq(argsEscapeBackslash[1], "cd$fg");
  assertEq(argsEscapeBackslash[2], "hi");
  assertEq(argsEscapeBackslash[3], R"(windows\path\here)");

  std::vector<std::string> argsEscapeQuotes = parseEnvFlags(
      " \"-I/path/contains space\"  '-Lanother/path with/space' normal  "
  );
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsEscapeQuotes.size(), static_cast<std::size_t>(3));
  assertEq(argsEscapeQuotes[0], "-I/path/contains space");
  assertEq(argsEscapeQuotes[1], "-Lanother/path with/space");
  assertEq(argsEscapeQuotes[2], "normal");

  std::vector<std::string> argsEscapeMixed = parseEnvFlags(
      R"-( "-IMy \"Headers\"\\v1" '\?pattern' normal path/contain/\"quote\" mixEverything" abc "\?\#   )-"
  );
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsEscapeMixed.size(), static_cast<std::size_t>(5));
  assertEq(argsEscapeMixed[0], R"(-IMy "Headers"\v1)");
  assertEq(argsEscapeMixed[1], "?pattern");
  assertEq(argsEscapeMixed[2], "normal");
  assertEq(argsEscapeMixed[3], "path/contain/\"quote\"");
  assertEq(argsEscapeMixed[4], "mixEverything abc ?#");

  pass();
}

}  // namespace tests

int
main() {
  // tests::testCycleVars();
  // tests::testSimpleVars();
  // tests::testDependOnUnregisteredVar();
  // tests::testCycleTargets();
  // tests::testSimpleTargets();
  // tests::testDependOnUnregisteredTarget();
  tests::testParseEnvFlags();
}
#endif
