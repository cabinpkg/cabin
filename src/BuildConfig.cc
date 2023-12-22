#include "BuildConfig.hpp"

#include "Algos.hpp"
#include "Logger.hpp"
#include "Manifest.hpp"
#include "TermColor.hpp"

#include <array>
#include <filesystem>
#include <fstream>
#include <memory>
#include <sstream>
#include <stdexcept>

static String OUT_DIR = "poac-out/debug";
static String CXX = "clang++";
static String INCLUDES;

struct Target {
  Vec<String> commands;
  Vec<String> dependsOn;
};

struct BuildConfig {
  HashMap<String, String> variables;
  HashMap<String, Vec<String>> varDeps;
  HashMap<String, Target> targets;
  HashMap<String, Vec<String>> targetDeps;

  void
  defineVariable(String name, String value, const Vec<String>& dependsOn = {});
  void defineTarget(
      String name, const Vec<String>& commands,
      const Vec<String>& dependsOn = {}
  );
  void emitMakefile(std::ostream& os = std::cout) const;
};

void BuildConfig::defineVariable(
    String name, String value, const Vec<String>& dependsOn
) {
  variables[name] = value;
  for (const String& dep : dependsOn) {
    // reverse dependency
    varDeps[dep].push_back(name);
  }
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
  const Vec<String> sortedVars = topoSort(variables, varDeps);
  for (const String& var : sortedVars) {
    if (var == "CXX") {
      os << var << " ?= " << variables.at(var) << '\n';
    } else {
      os << var << " = " << variables.at(var) << '\n';
    }
  }
  if (!sortedVars.empty() && !targets.empty()) {
    os << '\n';
  }

  if (targets.contains(".PHONY")) {
    emitTarget(os, ".PHONY", targets.at(".PHONY").dependsOn);
  }
  if (targets.contains("all")) {
    emitTarget(os, "all", targets.at("all").dependsOn);
  }
  const Vec<String> sortedTargets = topoSort(targets, targetDeps);
  for (auto itr = sortedTargets.rbegin(); itr != sortedTargets.rend(); itr++) {
    if (*itr == ".PHONY" || *itr == "all") {
      continue;
    }
    emitTarget(os, *itr, targets.at(*itr).dependsOn, targets.at(*itr).commands);
  }
}

static Vec<String> listSourceFiles(StringRef directory) {
  Vec<String> sourceFiles;
  for (const auto& entry : fs::recursive_directory_iterator(directory)) {
    if (!SOURCE_FILE_EXTS.contains(entry.path().extension())) {
      continue;
    }
    sourceFiles.push_back(entry.path().string());
  }
  return sourceFiles;
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

static String runMM(const String& sourceFile) {
  const String command =
      "cd " + OUT_DIR + " && " + CXX + INCLUDES + " -MM " + sourceFile;
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

static bool isMakefileUpToDate(StringRef makefilePath) {
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

static void defineDirTarget(BuildConfig& config, const String& directory) {
  config.defineTarget(directory, {buildCmd("mkdir -p $@")});
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

  const String compileCmd = "$(CXX) $(CFLAGS) $(INCLUDES)";
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
  commands[1] = buildCmd("$(CXX) $(CFLAGS) $^ -o $@");
  config.defineTarget(binTarget, commands, deps);
}

struct ObjTargetInfo {
  String name;
  String baseDir;
  Vec<String> deps;
};

// Returns the directory where the Makefile is generated.
String emitMakefile(const bool debug) {
  if (!fs::exists("src")) {
    throw std::runtime_error("src directory not found");
  }
  if (!fs::exists("src/main.cc")) {
    throw std::runtime_error("src/main.cc not found");
  }
  if (!fs::exists(OUT_DIR)) {
    fs::create_directories(OUT_DIR);
  }
  if (const char* cxx = std::getenv("CXX")) {
    CXX = cxx;
  }

  const String makefilePath = OUT_DIR + "/Makefile";
  if (isMakefileUpToDate(makefilePath)) {
    Logger::debug("Makefile is up to date");
    return OUT_DIR;
  }

  const String projectName = getPackageName();
  const String pathFromOutDir = "../../";

  BuildConfig config;

  // Compiler settings
  config.defineVariable("CXX", CXX);
  String cflags =
      "-Wall -Wextra -pedantic-errors -std=c++" + getPackageEdition();
  if (shouldColor()) {
    cflags += " -fdiagnostics-color";
  }
  if (debug) {
    cflags += " -g -O0 -DDEBUG";
  } else {
    cflags += " -O3 -DNDEBUG";
  }
  config.defineVariable("CFLAGS", cflags);

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
  config.defineVariable("INCLUDES", INCLUDES);

  // Build rules
  const String buildOutDir = projectName + ".d";
  defineDirTarget(config, buildOutDir);

  Vec<String> phonies = {"all"};
  config.defineTarget("all", {}, {projectName});

  const Vec<String> sourceFiles = listSourceFiles("src");
  Vec<String> buildObjTargets;

  // sourceFile.cc -> ObjTargetInfo
  HashMap<String, ObjTargetInfo> objTargetInfos;
  for (const String& sourceFileName : sourceFiles) {
    const String sourceFile = pathFromOutDir + sourceFileName;
    const String mmOutput = runMM(sourceFile);

    String objTarget; // sourceFile.o
    Vec<String> objTargetDeps;
    parseMMOutput(mmOutput, objTarget, objTargetDeps);

    const String targetBaseDir =
        fs::relative(Path(sourceFile).parent_path(), pathFromOutDir + "src")
            .string();
    objTargetInfos[sourceFileName] = {objTarget, targetBaseDir, objTargetDeps};

    // Add a target to create the buildOutDir and buildTargetBaseDir.
    Vec<String> buildTargetDeps = objTargetDeps;
    buildTargetDeps.push_back("|"); // order-only dependency
    buildTargetDeps.push_back(buildOutDir);
    String buildTargetBaseDir = buildOutDir;
    if (targetBaseDir != ".") {
      buildTargetBaseDir += "/" + targetBaseDir;
      defineDirTarget(config, buildTargetBaseDir);
      buildTargetDeps.push_back(buildTargetBaseDir);
    }

    const String buildObjTarget = buildTargetBaseDir + "/" + objTarget;
    buildObjTargets.push_back(buildObjTarget);
    defineCompileTarget(config, buildObjTarget, buildTargetDeps);
  }
  defineLinkTarget(config, projectName, buildObjTargets);

  // Targets for testing.
  bool enableTesting = false;
  Vec<String> testCommands;
  Vec<String> testTargets;
  const HashSet<String> buildObjTargetSet(
      buildObjTargets.begin(), buildObjTargets.end()
  );
  const String testOutDir = "tests";
  for (auto& [sourceFile, objTargetInfo] : objTargetInfos) {
    if (containsTestCode(sourceFile)) {
      enableTesting = true;

      // NOTE: Since we know that we don't use objTargetInfos for other
      // targets, we can just update it here instead of creating a copy.
      objTargetInfo.deps.push_back("|"); // order-only dependency
      objTargetInfo.deps.push_back(testOutDir);

      // Add a target to create the testTargetBaseDir.
      String testTargetBaseDir = testOutDir;
      if (objTargetInfo.baseDir != ".") {
        testTargetBaseDir += "/" + objTargetInfo.baseDir;
        defineDirTarget(config, testTargetBaseDir);
        objTargetInfo.deps.push_back(testTargetBaseDir);
      }

      const String testObjTarget =
          testTargetBaseDir + "/test_" + objTargetInfo.name;
      const String testTargetName = Path(sourceFile).stem().string();
      const String testTarget = testTargetBaseDir + "/test_" + testTargetName;

      // Test object target.
      defineCompileTarget(config, testObjTarget, objTargetInfo.deps, true);

      // Test binary target.
      Vec<String> testTargetDeps = {testObjTarget};
      // This test target depends on the object file corresponding to
      // the header file included in this source file.
      for (const String& header : objTargetInfo.deps) {
        // We shouldn't depend on the original object file (e.g.,
        // poac.d/path/to/file.o). We should depend on the test object
        // file (e.g., tests/path/to/test_file.o).
        const Path headerPath(header);
        if (Path(sourceFile).stem().string() == headerPath.stem().string()) {
          continue;
        }
        if (!HEADER_FILE_EXTS.contains(headerPath.extension())) {
          continue;
        }

        // headerPath: src/path/to/header.h ->
        // object target: poac.d/path/to/header.o
        String headerObjTargetBaseDir =
            fs::relative(headerPath.parent_path(), pathFromOutDir + "src")
                .string();
        if (headerObjTargetBaseDir != ".") {
          headerObjTargetBaseDir = buildOutDir + "/" + headerObjTargetBaseDir;
        } else {
          headerObjTargetBaseDir = buildOutDir;
        }
        const String headerObjTarget =
            headerObjTargetBaseDir + "/" + headerPath.stem().string() + ".o";
        Logger::debug("headerObjTarget: ", headerObjTarget);

        auto itr = buildObjTargetSet.find(headerObjTarget);
        if (itr == buildObjTargetSet.end()) {
          continue;
        }
        testTargetDeps.push_back(*itr);
      }

      defineLinkTarget(config, testTarget, testTargetDeps);
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
    defineDirTarget(config, testOutDir);
    config.defineTarget("test", testCommands, testTargets);
    phonies.push_back("test");
  }

  config.defineTarget(".PHONY", {}, phonies);

  std::ofstream ofs(makefilePath);
  config.emitMakefile(ofs);
  return OUT_DIR;
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

#  include <cassert>
#  include <sstream>
#  include <stdexcept>

void test_cycle_vars() {
  BuildConfig config;
  config.defineVariable("a", "b", {"b"});
  config.defineVariable("b", "c", {"c"});
  config.defineVariable("c", "a", {"a"});

  try {
    std::stringstream ss;
    config.emitMakefile(ss);
  } catch (const std::runtime_error& e) {
    assert(std::string(e.what()) == "too complex build graph");
    return;
  }

  assert(false && "should not reach here");
}

void test_simple_vars() {
  BuildConfig config;
  config.defineVariable("c", "3", {"b"});
  config.defineVariable("b", "2", {"a"});
  config.defineVariable("a", "1");

  std::stringstream ss;
  config.emitMakefile(ss);

  assert(ss.str() == "a = 1\n"
                      "b = 2\n"
                      "c = 3\n");
}

void test_depend_on_unregistered_var() {
  BuildConfig config;
  config.defineVariable("a", "1", {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  assert(ss.str() == "a = 1\n");
}

void test_cycle_targets() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"}, {"b"});
  config.defineTarget("b", {"echo b"}, {"c"});
  config.defineTarget("c", {"echo c"}, {"a"});

  try {
    std::stringstream ss;
    config.emitMakefile(ss);
  } catch (const std::runtime_error& e) {
    assert(std::string(e.what()) == "too complex build graph");
    return;
  }

  assert(false && "should not reach here");
}

void test_simple_targets() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"});
  config.defineTarget("b", {"echo b"}, {"a"});
  config.defineTarget("c", {"echo c"}, {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  assert(ss.str() == "c: b\n"
                      "\techo c\n"
                      "\n"
                      "b: a\n"
                      "\techo b\n"
                      "\n"
                      "a:\n"
                      "\techo a\n"
                      "\n");
}

void test_depend_on_unregistered_target() {
  BuildConfig config;
  config.defineTarget("a", {"echo a"}, {"b"});

  std::stringstream ss;
  config.emitMakefile(ss);

  assert(ss.str() == "a: b\n"
                      "\techo a\n"
                      "\n");
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
