#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Logger.hpp"
#include "Manifest.hpp"
#include "TermColor.hpp"

#include <algorithm>
#include <array>
#include <cstdlib>
#include <ctype.h>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <memory>
#include <ostream>
#include <sstream>
#include <stdexcept>
#include <stdio.h>
#include <string>

static String OUT_DIR;
static constexpr StringRef TEST_OUT_DIR = "tests";
static const String PATH_FROM_OUT_DIR = "../../";
static String CXX = "clang++";
static String INCLUDES;
static String DEFINES;

static void setOutDir(const bool debug) {
  if (debug) {
    OUT_DIR = "poac-out/debug";
  } else {
    OUT_DIR = "poac-out/release";
  }
}

static String getOutDir() {
  if (OUT_DIR.empty()) {
    throw std::runtime_error("OUT_DIR is not set");
  }
  return OUT_DIR;
}

enum class VarType {
  Recursive, // =
  Simple, // :=
  Cond, // ?=
  Append, // +=
  Shell, // !=
};

std::ostream& operator<<(std::ostream& os, VarType type) {
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

struct Variable {
  String value;
  VarType type;
};

std::ostream& operator<<(std::ostream& os, const Variable& var) {
  os << var.type << ' ' << var.value;
  return os;
}

struct Target {
  Vec<String> commands;
  Vec<String> dependsOn;
};

struct BuildConfig {
  HashMap<String, Variable> variables;
  HashMap<String, Vec<String>> varDeps;
  HashMap<String, Target> targets;
  HashMap<String, Vec<String>> targetDeps;
  Option<Target> phony;
  Option<Target> all;

  void defineVariable(String, Variable, const Vec<String>& = {});
  void defineSimpleVariable(String, String, const Vec<String>& = {});
  void defineCondVariable(String, String, const Vec<String>& = {});

  void defineTarget(String, const Vec<String>&, const Vec<String>& = {});
  void setPhony(const Vec<String>&);
  void setAll(const Vec<String>&);
  void emitMakefile(std::ostream& = std::cout) const;
  void emitCompdb(StringRef, std::ostream& = std::cout) const;
};

void BuildConfig::defineVariable(
    String name, Variable value, const Vec<String>& dependsOn
) {
  variables[name] = value;
  for (const String& dep : dependsOn) {
    // reverse dependency
    varDeps[dep].push_back(name);
  }
}

void BuildConfig::defineSimpleVariable(
    String name, String value, const Vec<String>& dependsOn
) {
  defineVariable(name, {value, VarType::Simple}, dependsOn);
}

void BuildConfig::defineCondVariable(
    String name, String value, const Vec<String>& dependsOn
) {
  defineVariable(name, {value, VarType::Cond}, dependsOn);
}

void BuildConfig::defineTarget(
    String name, const Vec<String>& commands, const Vec<String>& dependsOn
) {
  targets[name] = {commands, dependsOn};
  for (const String& dep : dependsOn) {
    // reverse dependency
    targetDeps[dep].push_back(name);
  }
}

void BuildConfig::setPhony(const Vec<String>& dependsOn) {
  phony = {{}, dependsOn};
}

void BuildConfig::setAll(const Vec<String>& dependsOn) {
  all = {{}, dependsOn};
}

static void emitTarget(
    std::ostream& os, StringRef target, const Vec<String>& dependsOn,
    const Vec<String>& commands = {}
) {
  usize offset = 0;

  os << target << ":";
  offset += target.size() + 2; // : and space

  for (const String& dep : dependsOn) {
    if (offset + dep.size() + 2 > 80) { // 2 for space and \.
      // \ for line continuation. \ is the 80th character.
      os << std::setw(83 - offset) << " \\\n ";
      offset = 2;
    }
    os << " " << dep;
    offset += dep.size() + 1; // space
  }
  os << '\n';

  for (const String& cmd : commands) {
    os << '\t' << cmd << '\n';
  }
  os << '\n';
}

void BuildConfig::emitMakefile(std::ostream& os) const {
  // TODO: I guess we can topo-sort when calling defineVariable and
  // defineTarget.  Then we don't need to sort here.  This way we can
  // avoid the extra memory usage and possibly improve time complexity.
  // The current way is simple and bug-free though.

  const Vec<String> sortedVars = topoSort(variables, varDeps);
  for (const String& varName : sortedVars) {
    os << varName << ' ' << variables.at(varName) << '\n';
  }
  if (!sortedVars.empty() && !targets.empty()) {
    os << '\n';
  }

  if (phony.has_value()) {
    emitTarget(os, ".PHONY", phony->dependsOn);
  }
  if (all.has_value()) {
    emitTarget(os, "all", all->dependsOn);
  }
  const Vec<String> sortedTargets = topoSort(targets, targetDeps);
  for (auto itr = sortedTargets.rbegin(); itr != sortedTargets.rend(); itr++) {
    emitTarget(os, *itr, targets.at(*itr).dependsOn, targets.at(*itr).commands);
  }
}

void BuildConfig::emitCompdb(StringRef baseDir, std::ostream& os) const {
  const Path baseDirPath = fs::canonical(baseDir);
  const HashSet<String> phonyDeps(
      phony->dependsOn.begin(), phony->dependsOn.end()
  );
  const String firstIdent = String(2, ' ');
  const String secondIdent = String(4, ' ');

  std::stringstream ss;
  for (const auto& [target, targetInfo] : targets) {
    if (phonyDeps.contains(target)) {
      // Ignore phony dependencies.
      continue;
    }

    bool isCompileTarget = false;
    for (StringRef cmd : targetInfo.commands) {
      if (!cmd.starts_with("$(CXX)") && !cmd.starts_with("@$(CXX)")) {
        continue;
      }
      if (cmd.find("-c") == String::npos) {
        // Ignore linking commands.
        continue;
      }
      isCompileTarget = true;
    }
    if (!isCompileTarget) {
      continue;
    }

    // The first dependency is the source file.
    const String file = targetInfo.dependsOn[0];
    // The output is the target.
    const String output = target;
    const String cmd = CXX + ' ' + variables.at("CXXFLAGS").value + DEFINES
                       + INCLUDES + " -c " + file + " -o " + output;

    ss << firstIdent << "{\n";
    ss << secondIdent << "\"directory\": " << baseDirPath << ",\n";
    ss << secondIdent << "\"file\": " << std::quoted(file) << ",\n";
    ss << secondIdent << "\"output\": " << std::quoted(output) << ",\n";
    ss << secondIdent << "\"command\": " << std::quoted(cmd) << "\n";
    ss << firstIdent << "},\n";
  }

  String output = ss.str();
  if (!output.empty()) {
    // Remove the last comma.
    output.pop_back(); // \n
    output.pop_back(); // ,
  }

  os << "[\n";
  os << output << '\n';
  os << "]\n";
}

static Vec<String> listSourceFilePaths(StringRef directory) {
  Vec<String> sourceFilePaths;
  for (const auto& entry : fs::recursive_directory_iterator(directory)) {
    if (!SOURCE_FILE_EXTS.contains(entry.path().extension())) {
      continue;
    }
    sourceFilePaths.push_back(entry.path().string());
  }
  return sourceFilePaths;
}

static String exec(const char* cmd) {
  std::array<char, 128> buffer;
  String result;
  std::unique_ptr<FILE, decltype(&pclose)> pipe(popen(cmd, "r"), pclose);
  if (!pipe) {
    throw std::runtime_error("popen() failed!");
  }
  while (fgets(buffer.data(), buffer.size(), pipe.get()) != nullptr) {
    result += buffer.data();
  }
  return result;
}

static String runMM(const String& sourceFile, const bool isTest = false) {
  String command = "cd " + getOutDir() + " && " + CXX + DEFINES + INCLUDES;
  if (isTest) {
    command += " -DPOAC_TEST -MM " + sourceFile;
  } else {
    command += " -MM " + sourceFile;
  }
  return exec(command.c_str());
}

static void
parseMMOutput(const String& mmOutput, String& target, Vec<String>& deps) {
  std::istringstream iss(mmOutput);
  std::getline(iss, target, ':');
  Logger::debug(target, ':');

  String dependency;
  while (std::getline(iss, dependency, ' ')) {
    if (!dependency.empty() && dependency.front() != '\\') {
      // Remove trailing newline if it exists
      if (dependency.back() == '\n') {
        dependency.pop_back();
      }
      deps.push_back(dependency);
      Logger::debug(" '", dependency, "'");
    }
  }
  Logger::debug("");
}

static bool isUpToDate(StringRef makefilePath) {
  if (!fs::exists(makefilePath)) {
    return false;
  }

  const fs::file_time_type makefileTime = fs::last_write_time(makefilePath);
  // Makefile depends on all files in ./src and poac.toml.
  for (const auto& entry : fs::recursive_directory_iterator("src")) {
    if (fs::last_write_time(entry.path()) > makefileTime) {
      return false;
    }
  }
  if (fs::last_write_time("poac.toml") > makefileTime) {
    return false;
  }

  return true;
}

static bool containsTestCode(const String& sourceFile) {
  std::ifstream ifs(sourceFile);
  String line;
  while (std::getline(ifs, line)) {
    if (line.find("POAC_TEST") != String::npos) {
      Logger::debug("contains test code: ", sourceFile);
      return true;
    }
  }
  Logger::debug("does not contain test code: ", sourceFile);
  return false;
}

static String buildCmd(const String& cmd) noexcept {
  if (isVerbose()) {
    return cmd;
  } else {
    return "@" + cmd;
  }
}

static void defineDirTarget(BuildConfig& config, StringRef directory) {
  config.defineTarget(String(directory), {buildCmd("mkdir -p $@")});
}

static void defineCompileTarget(
    BuildConfig& config, const String& objTarget, const Vec<String>& deps,
    const bool isTest = false
) {
  std::ostringstream oss;
  Logger::log(
      oss, LogLevel::info, "Compiling", deps[0].substr(6) // remove "../../"
  );

  Vec<String> commands(2);
  commands[0] = "@echo '" + oss.str() + "'";

  const String compileCmd = "$(CXX) $(CXXFLAGS) $(DEFINES) $(INCLUDES)";
  if (isTest) {
    commands[1] = buildCmd(compileCmd + " -DPOAC_TEST -c $< -o $@");
  } else {
    commands[1] = buildCmd(compileCmd + " -c $< -o $@");
  }
  config.defineTarget(objTarget, commands, deps);
}

static void defineLinkTarget(
    BuildConfig& config, const String& binTarget, const Vec<String>& deps
) {
  std::ostringstream oss;
  Logger::log(oss, LogLevel::info, "Linking", binTarget);

  Vec<String> commands(2);
  commands[0] = "@echo '" + oss.str() + "'";
  commands[1] = buildCmd("$(CXX) $(CXXFLAGS) $^ -o $@");
  config.defineTarget(binTarget, commands, deps);
}

struct ObjTargetInfo {
  String name;
  String baseDir;
};

static void recursiveFindObjDeps(
    HashSet<String>& deps, const Vec<String>& objTargetDeps,
    const String& sourceFile, const String& buildOutDir,
    const HashSet<String>& buildObjTargetSet, BuildConfig& config
) {
  // This test target depends on the object file corresponding to
  // the header file included in this source file.
  for (const String& header : objTargetDeps) {
    const Path headerPath(header);
    if (Path(sourceFile).stem().string() == headerPath.stem().string()) {
      // We shouldn't depend on the original object file (e.g.,
      // poac.d/path/to/file.o). We should depend on the test object
      // file (e.g., tests/path/to/test_file.o).
      continue;
    }
    if (!HEADER_FILE_EXTS.contains(headerPath.extension())) {
      continue;
    }

    // Map headerPath to the corresponding object target.
    // headerPath: src/path/to/header.h ->
    // object target: poac.d/path/to/header.o
    String headerObjTargetBaseDir =
        fs::relative(headerPath.parent_path(), PATH_FROM_OUT_DIR + "src")
            .string();
    if (headerObjTargetBaseDir != ".") {
      headerObjTargetBaseDir = buildOutDir + "/" + headerObjTargetBaseDir;
    } else {
      headerObjTargetBaseDir = buildOutDir;
    }
    const String headerObjTarget =
        headerObjTargetBaseDir + "/" + headerPath.stem().string() + ".o";
    Logger::debug("headerObjTarget: ", headerObjTarget);

    if (deps.contains(headerObjTarget)) {
      continue;
    }

    auto itr = buildObjTargetSet.find(headerObjTarget);
    if (itr == buildObjTargetSet.end()) {
      continue;
    }
    deps.insert(*itr);
    Logger::debug("headerObjTarget: added ", *itr);
    recursiveFindObjDeps(
        deps, config.targets.at(*itr).dependsOn, sourceFile, buildOutDir,
        buildObjTargetSet, config
    );
  }
}

static BuildConfig configureBuild(const bool debug) {
  if (!fs::exists("src")) {
    throw std::runtime_error("src directory not found");
  }
  if (!fs::exists("src/main.cc")) {
    throw std::runtime_error("src/main.cc not found");
  }

  const String outDir = getOutDir();
  if (!fs::exists(outDir)) {
    fs::create_directories(outDir);
  }
  if (const char* cxx = std::getenv("CXX")) {
    CXX = cxx;
  }

  const String packageName = getPackageName();

  BuildConfig config;

  // Variables
  config.defineCondVariable("CXX", CXX);
  String cxxflags =
      "-Wall -Wextra -pedantic-errors -std=c++" + getPackageEdition();
  if (shouldColor()) {
    cxxflags += " -fdiagnostics-color";
  }
  if (debug) {
    cxxflags += " -g -O0 -DDEBUG";
  } else {
    cxxflags += " -O3 -DNDEBUG";
  }
  config.defineSimpleVariable("CXXFLAGS", cxxflags);

  String packageNameUpper = packageName;
  std::transform(
      packageNameUpper.begin(), packageNameUpper.end(),
      packageNameUpper.begin(), ::toupper
  );
  DEFINES =
      " -D" + packageNameUpper + "_VERSION='\"" + getPackageVersion() + "\"'";
  config.defineSimpleVariable("DEFINES", DEFINES);

  const Vec<Path> deps = installGitDependencies();
  for (const Path& dep : deps) {
    const Path includeDir = dep / "include";
    if (fs::exists(includeDir) && fs::is_directory(includeDir)
        && !fs::is_empty(includeDir)) {
      INCLUDES += " -I" + includeDir.string();
    } else {
      INCLUDES += " -I" + dep.string();
    }
  }
  Logger::debug("INCLUDES: ", INCLUDES);
  config.defineSimpleVariable("INCLUDES", INCLUDES);

  // Build rules
  const String buildOutDir = packageName + ".d";
  defineDirTarget(config, buildOutDir);

  Vec<String> phonies = {"all"};
  config.setAll({packageName});

  const Vec<String> sourceFilePaths = listSourceFilePaths("src");
  Vec<String> buildObjTargets;

  // sourceFile.cc -> ObjTargetInfo
  HashMap<String, ObjTargetInfo> objTargetInfos;
  for (const String& sourceFilePath : sourceFilePaths) {
    const String sourceFile = PATH_FROM_OUT_DIR + sourceFilePath;
    const String mmOutput = runMM(sourceFile);

    String objTarget; // sourceFile.o
    Vec<String> objTargetDeps;
    parseMMOutput(mmOutput, objTarget, objTargetDeps);

    const String targetBaseDir =
        fs::relative(Path(sourceFile).parent_path(), PATH_FROM_OUT_DIR + "src")
            .string();
    objTargetInfos[sourceFilePath] = {objTarget, targetBaseDir};

    // Add a target to create the buildOutDir and buildTargetBaseDir.
    objTargetDeps.emplace_back("|"); // order-only dependency
    objTargetDeps.push_back(buildOutDir);
    String buildTargetBaseDir = buildOutDir;
    if (targetBaseDir != ".") {
      buildTargetBaseDir += "/" + targetBaseDir;
      defineDirTarget(config, buildTargetBaseDir);
      objTargetDeps.push_back(buildTargetBaseDir);
    }

    const String buildObjTarget = buildTargetBaseDir + "/" + objTarget;
    buildObjTargets.push_back(buildObjTarget);
    defineCompileTarget(config, buildObjTarget, objTargetDeps);
  }
  defineLinkTarget(config, packageName, buildObjTargets);

  // Targets for testing.
  bool enableTesting = false;
  Vec<String> testCommands;
  Vec<String> testTargets;
  const HashSet<String> buildObjTargetSet(
      buildObjTargets.begin(), buildObjTargets.end()
  );
  for (auto& [sourceFilePath, objTargetInfo] : objTargetInfos) {
    if (containsTestCode(sourceFilePath)) {
      enableTesting = true;

      const String sourceFile = PATH_FROM_OUT_DIR + sourceFilePath;
      const String mmOutput = runMM(sourceFile, true /* isTest */);

      String objTarget; // sourceFile.o
      Vec<String> objTargetDeps;
      parseMMOutput(mmOutput, objTarget, objTargetDeps);

      // Add a target to create the testTargetBaseDir.
      objTargetDeps.emplace_back("|"); // order-only dependency
      objTargetDeps.emplace_back(TEST_OUT_DIR);
      String testTargetBaseDir(TEST_OUT_DIR);
      if (objTargetInfo.baseDir != ".") {
        testTargetBaseDir += "/" + objTargetInfo.baseDir;
        defineDirTarget(config, testTargetBaseDir);
        objTargetDeps.push_back(testTargetBaseDir);
      }

      const String testObjTarget =
          testTargetBaseDir + "/test_" + objTargetInfo.name;
      const String testTargetName = Path(sourceFile).stem().string();
      const String testTarget = testTargetBaseDir + "/test_" + testTargetName;
      Logger::debug("testTarget: ", testTarget);

      // Test object target.
      defineCompileTarget(
          config, testObjTarget, objTargetDeps, true /* isTest */
      );

      // Test binary target.
      HashSet<String> testTargetDeps = {testObjTarget};
      recursiveFindObjDeps(
          testTargetDeps, objTargetDeps, sourceFile, buildOutDir,
          buildObjTargetSet, config
      );
      Vec<String> testTargetDepsVec(
          testTargetDeps.begin(), testTargetDeps.end()
      );
      defineLinkTarget(config, testTarget, testTargetDepsVec);
      Logger::debug(testTarget, ':');
      for (const String& dep : testTargetDeps) {
        Logger::debug(" '", dep, "'");
      }

      std::ostringstream oss;
      Logger::log(oss, LogLevel::info, "Testing", testTargetName);
      testCommands.push_back("@echo '" + oss.str() + "'");
      testCommands.push_back(buildCmd(testTarget));
      testTargets.push_back(testTarget);
    }
  }
  if (enableTesting) {
    // Target to create the tests directory.
    defineDirTarget(config, TEST_OUT_DIR);
    config.defineTarget("test", testCommands, testTargets);
    phonies.emplace_back("test");
  }

  config.setPhony(phonies);
  return config;
}

/// @returns the directory where the Makefile is generated.
String emitMakefile(const bool debug) {
  setOutDir(debug);

  const String outDir = getOutDir();
  const String makefilePath = outDir + "/Makefile";
  if (isUpToDate(makefilePath)) {
    Logger::debug("Makefile is up to date");
    return outDir;
  }

  const BuildConfig config = configureBuild(debug);
  std::ofstream ofs(makefilePath);
  config.emitMakefile(ofs);
  return outDir;
}

/// @returns the directory where the compilation database is generated.
String emitCompdb(const bool debug) {
  setOutDir(debug);

  const String outDir = getOutDir();
  const String compdbPath = outDir + "/compile_commands.json";
  if (isUpToDate(compdbPath)) {
    Logger::debug("compile_commands.json is up to date");
    return outDir;
  }

  const BuildConfig config = configureBuild(debug);
  std::ofstream ofs(compdbPath);
  config.emitCompdb(outDir, ofs);
  return outDir;
}

String modeString(const bool debug) {
  return debug ? "debug" : "release";
}

String getMakeCommand() {
  if (isVerbose()) {
    return "make";
  } else {
    return "make -s --no-print-directory";
  }
}

#ifdef POAC_TEST

#  include "TestUtils.hpp"

void test_cycle_vars() {
  BuildConfig config;
  config.defineSimpleVariable("a", "b", {"b"});
  config.defineSimpleVariable("b", "c", {"c"});
  config.defineSimpleVariable("c", "a", {"a"});

  ASSERT_EXCEPTION(std::stringstream ss; config.emitMakefile(ss),
                                         std::runtime_error,
                                         "too complex build graph");
}

void test_simple_vars() {
  BuildConfig config;
  config.defineSimpleVariable("c", "3", {"b"});
  config.defineSimpleVariable("b", "2", {"a"});
  config.defineSimpleVariable("a", "1");

  std::stringstream ss;
  config.emitMakefile(ss);

  ASSERT_EQ(
      ss.str(),
      "a := 1\n"
      "b := 2\n"
      "c := 3\n"
  );
}

void test_depend_on_unregistered_var() {
  BuildConfig config;
  config.defineSimpleVariable("a", "1", {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  ASSERT_EQ(ss.str(), "a := 1\n");
}

void test_cycle_targets() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"}, {"b"});
  config.defineTarget("b", {"echo b"}, {"c"});
  config.defineTarget("c", {"echo c"}, {"a"});

  ASSERT_EXCEPTION(std::stringstream ss; config.emitMakefile(ss),
                                         std::runtime_error,
                                         "too complex build graph");
}

void test_simple_targets() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"});
  config.defineTarget("b", {"echo b"}, {"a"});
  config.defineTarget("c", {"echo c"}, {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  ASSERT_EQ(
      ss.str(),
      "c: b\n"
      "\techo c\n"
      "\n"
      "b: a\n"
      "\techo b\n"
      "\n"
      "a:\n"
      "\techo a\n"
      "\n"
  );
}

void test_depend_on_unregistered_target() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"}, {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  ASSERT_EQ(
      ss.str(),
      "a: b\n"
      "\techo a\n"
      "\n"
  );
}

int main() {
  test_cycle_vars();
  test_simple_vars();
  test_depend_on_unregistered_var();
  test_cycle_targets();
  test_simple_targets();
  test_depend_on_unregistered_target();
}
#endif
