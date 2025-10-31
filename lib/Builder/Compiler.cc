#include "Builder/Compiler.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Rustify/Result.hpp"

#include <array>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

static std::optional<std::string> getEnvVar(const char* name) {
  if (const char* value = std::getenv(name);
      value != nullptr && *value != '\0') {
    return std::string(value);
  }
  return std::nullopt;
}

static std::optional<std::string>
findSiblingTool(const fs::path& base, const std::string& candidate) {
  if (base.has_parent_path()) {
    const fs::path sibling = base.parent_path() / candidate;
    if (fs::exists(sibling)) {
      return sibling.string();
    }
  }
  return std::nullopt;
}

static std::optional<std::string>
makeToolNameForCompiler(const std::string& compilerName,
                        std::string_view suffix, std::string_view tool) {
  const std::size_t pos = compilerName.rfind(suffix);
  if (pos == std::string::npos) {
    return std::nullopt;
  }
  if (pos + suffix.size() > compilerName.size()) {
    return std::nullopt;
  }
  if (pos != 0) {
    const auto prev = static_cast<unsigned char>(compilerName[pos - 1]);
    if (std::isalnum(prev)) {
      return std::nullopt;
    }
  }

  const std::string prefix = compilerName.substr(0, pos);
  const std::string postfix =
      compilerName.substr(pos + static_cast<std::size_t>(suffix.size()));
  return fmt::format("{}{}{}", prefix, tool, postfix);
}

static std::optional<std::string> resolveToolWithSuffix(const fs::path& cxxPath,
                                                        std::string_view suffix,
                                                        std::string_view tool) {
  const std::string filename = cxxPath.filename().string();
  const auto candidateName = makeToolNameForCompiler(filename, suffix, tool);
  if (!candidateName.has_value()) {
    return std::nullopt;
  }
  const std::string& candidate = candidateName.value();

  if (auto sibling = findSiblingTool(cxxPath, candidate); sibling.has_value()) {
    return sibling;
  }
  if (commandExists(candidate)) {
    return candidate;
  }
  return std::nullopt;
}

static std::optional<std::string> resolveLlvmAr(const fs::path& cxxPath) {
  if (auto resolved = resolveToolWithSuffix(cxxPath, "clang++", "llvm-ar");
      resolved.has_value()) {
    return resolved;
  }
  if (auto resolved = resolveToolWithSuffix(cxxPath, "clang", "llvm-ar");
      resolved.has_value()) {
    return resolved;
  }
  if (commandExists("llvm-ar")) {
    return std::string("llvm-ar");
  }
  return std::nullopt;
}

static std::optional<std::string> resolveGccAr(const fs::path& cxxPath) {
  if (auto resolved = resolveToolWithSuffix(cxxPath, "g++", "gcc-ar");
      resolved.has_value()) {
    return resolved;
  }
  if (auto resolved = resolveToolWithSuffix(cxxPath, "gcc", "gcc-ar");
      resolved.has_value()) {
    return resolved;
  }
  if (commandExists("gcc-ar")) {
    return std::string("gcc-ar");
  }
  return std::nullopt;
}

enum class CompilerFlavor : std::uint8_t { Clang, Gcc, Other };

static CompilerFlavor detectCompilerFlavor(const fs::path& cxxPath) {
  const std::string name = cxxPath.filename().string();
  if (name.contains("clang")) {
    return CompilerFlavor::Clang;
  }
  if (name.contains("g++") || name.contains("gcc")) {
    return CompilerFlavor::Gcc;
  }
  return CompilerFlavor::Other;
}

static std::optional<std::string> envArchiverOverride() {
  if (auto ar = getEnvVar("CABIN_AR"); ar.has_value()) {
    return ar;
  }
  if (auto ar = getEnvVar("AR"); ar.has_value()) {
    return ar;
  }
  if (auto ar = getEnvVar("LLVM_AR"); ar.has_value()) {
    return ar;
  }
  if (auto ar = getEnvVar("GCC_AR"); ar.has_value()) {
    return ar;
  }
  return std::nullopt;
}

// TODO: The parsing of pkg-config output might not be robust.  It assumes
// that there wouldn't be backquotes or double quotes in the output, (should
// be treated as a single flag).  The current code just splits the output by
// space.

Result<CFlags>
CFlags::parsePkgConfig(const std::string_view pkgConfigVer) noexcept {
  const Command pkgConfigCmd =
      Command("pkg-config").addArg("--cflags").addArg(pkgConfigVer);
  std::string output = Try(getCmdOutput(pkgConfigCmd));
  output.pop_back(); // remove '\n'

  std::vector<Macro> macros;           // -D<name>=<val>
  std::vector<IncludeDir> includeDirs; // -I<dir>
  std::vector<std::string> others;     // e.g., -pthread, -fPIC

  const auto parseCFlag = [&](const std::string& flag) {
    if (flag.starts_with("-D")) {
      const std::string macro = flag.substr(2);
      const std::size_t eqPos = macro.find('=');
      if (eqPos == std::string::npos) {
        macros.emplace_back(macro, "");
      } else {
        macros.emplace_back(macro.substr(0, eqPos), macro.substr(eqPos + 1));
      }
    } else if (flag.starts_with("-I")) {
      includeDirs.emplace_back(flag.substr(2));
    } else {
      others.emplace_back(flag);
    }
  };

  std::string flag;
  for (const char i : output) {
    if (i != ' ') {
      flag += i;
    } else {
      if (flag.empty()) {
        continue;
      }

      parseCFlag(flag);
      flag.clear();
    }
  }
  if (!flag.empty()) {
    parseCFlag(flag);
  }

  return Ok(CFlags( //
      std::move(macros), std::move(includeDirs), std::move(others)));
}

void CFlags::merge(const CFlags& other) noexcept {
  macros.insert(macros.end(), other.macros.begin(), other.macros.end());
  includeDirs.insert(includeDirs.end(), other.includeDirs.begin(),
                     other.includeDirs.end());
  others.insert(others.end(), other.others.begin(), other.others.end());
}

Result<LdFlags>
LdFlags::parsePkgConfig(const std::string_view pkgConfigVer) noexcept {
  const Command pkgConfigCmd =
      Command("pkg-config").addArg("--libs").addArg(pkgConfigVer);
  std::string output = Try(getCmdOutput(pkgConfigCmd));
  output.pop_back(); // remove '\n'

  std::vector<LibDir> libDirs;     // -L<dir>
  std::vector<Lib> libs;           // -l<lib>
  std::vector<std::string> others; // e.g., -Wl,...

  const auto parseLdFlag = [&](const std::string& flag) {
    if (flag.starts_with("-L")) {
      libDirs.emplace_back(flag.substr(2));
    } else if (flag.starts_with("-l")) {
      libs.emplace_back(flag.substr(2));
    } else {
      others.emplace_back(flag);
    }
  };

  std::string flag;
  for (const char i : output) {
    if (i != ' ') {
      flag += i;
    } else {
      if (flag.empty()) {
        continue;
      }

      parseLdFlag(flag);
      flag.clear();
    }
  }
  if (!flag.empty()) {
    parseLdFlag(flag);
  }

  return Ok(LdFlags(std::move(libDirs), std::move(libs), std::move(others)));
}

LdFlags::LdFlags(std::vector<LibDir> libDirs, std::vector<Lib> libs,
                 std::vector<std::string> others) noexcept
    : libDirs(std::move(libDirs)), others(std::move(others)) {
  // Remove duplicates of libs.
  std::unordered_set<std::string> libSet;
  std::vector<Lib> dedupLibs;
  for (Lib& lib : libs) {
    if (libSet.insert(lib.name).second) {
      dedupLibs.emplace_back(std::move(lib));
    }
  }
  this->libs = std::move(dedupLibs);
}

void LdFlags::merge(const LdFlags& other) noexcept {
  libDirs.insert(libDirs.end(), other.libDirs.begin(), other.libDirs.end());
  others.insert(others.end(), other.others.begin(), other.others.end());

  // Remove duplicates of libs & other.libs.
  std::unordered_set<std::string> libSet;
  for (const Lib& lib : libs) {
    libSet.insert(lib.name);
  }
  std::vector<Lib> dedupLibs;
  for (const Lib& lib : other.libs) {
    if (libSet.insert(lib.name).second) {
      dedupLibs.emplace_back(lib);
    }
  }
  libs.insert(libs.end(), dedupLibs.begin(), dedupLibs.end());
}

Result<CompilerOpts>
CompilerOpts::parsePkgConfig(const VersionReq& pkgVerReq,
                             const std::string_view pkgName) noexcept {
  const std::string pkgConfigVer = pkgVerReq.toPkgConfigString(pkgName);
  CFlags cFlags = Try(CFlags::parsePkgConfig(pkgConfigVer));
  LdFlags ldFlags = Try(LdFlags::parsePkgConfig(pkgConfigVer));
  return Ok(CompilerOpts(std::move(cFlags), std::move(ldFlags)));
}

void CompilerOpts::merge(const CompilerOpts& other) noexcept {
  cFlags.merge(other.cFlags);
  ldFlags.merge(other.ldFlags);
}

Compiler Compiler::init(std::string cxx) noexcept {
  return Compiler(std::move(cxx));
}

Result<Compiler> Compiler::init() noexcept {
  if (const char* cxxP = std::getenv("CXX")) {
    return Ok(Compiler::init(std::string(cxxP)));
  }

  static constexpr std::array<std::string_view, 3> candidates{ "c++", "g++",
                                                               "clang++" };
  for (const std::string_view candidate : candidates) {
    if (commandExists(candidate)) {
      return Ok(Compiler::init(std::string(candidate)));
    }
  }

  return Err(anyhow::anyhow("failed to locate a C++ compiler, set $CXX"));
}

Command Compiler::makeCompileCmd(const CompilerOpts& opts,
                                 const std::string& sourceFile,
                                 const std::string& objFile) const {
  return Command(cxx)
      .addArgs(opts.cFlags.others)
      .addArgs(opts.cFlags.macros)
      .addArgs(opts.cFlags.includeDirs)
      .addArg("-c")
      .addArg(sourceFile)
      .addArg("-o")
      .addArg(objFile);
}

Command Compiler::makeMMCmd(const CompilerOpts& opts,
                            const std::string& sourceFile) const {
  return Command(cxx)
      .addArgs(opts.cFlags.others)
      .addArgs(opts.cFlags.macros)
      .addArgs(opts.cFlags.includeDirs)
      .addArg("-MM")
      .addArg(sourceFile);
}

Command Compiler::makePreprocessCmd(const CompilerOpts& opts,
                                    const std::string& sourceFile) const {
  return Command(cxx)
      .addArg("-E")
      .addArgs(opts.cFlags.others)
      .addArgs(opts.cFlags.macros)
      .addArgs(opts.cFlags.includeDirs)
      .addArg(sourceFile);
}

std::string Compiler::detectArchiver(const bool useLTO) const {
  if (auto override = envArchiverOverride(); override.has_value()) {
    return override.value();
  }
  if (!useLTO) {
    return "ar";
  }

  const fs::path cxxPath(cxx);
  switch (detectCompilerFlavor(cxxPath)) {
  case CompilerFlavor::Clang:
    if (auto llvmAr = resolveLlvmAr(cxxPath); llvmAr.has_value()) {
      return llvmAr.value();
    }
    break;
  case CompilerFlavor::Gcc:
    if (auto gccAr = resolveGccAr(cxxPath); gccAr.has_value()) {
      return gccAr.value();
    }
    break;
  case CompilerFlavor::Other:
    break;
  }

  return "ar";
}

} // namespace cabin

#ifdef CABIN_TEST

#  include "Rustify/Tests.hpp"

namespace tests {

using namespace cabin; // NOLINT(build/namespaces,google-build-using-namespace)

static void testMakeToolNameForCompiler() {
  auto expectValue = [](const std::optional<std::string>& value,
                        const std::string& expected) {
    assertTrue(value.has_value());
    assertEq(*value, expected);
  };

  expectValue(makeToolNameForCompiler("clang++", "clang++", "llvm-ar"),
              "llvm-ar");
  expectValue(makeToolNameForCompiler("clang++-19", "clang++", "llvm-ar"),
              "llvm-ar-19");
  expectValue(makeToolNameForCompiler("aarch64-linux-gnu-clang++", "clang++",
                                      "llvm-ar"),
              "aarch64-linux-gnu-llvm-ar");
  expectValue(
      makeToolNameForCompiler("x86_64-w64-mingw32-g++-13", "g++", "gcc-ar"),
      "x86_64-w64-mingw32-gcc-ar-13");

  assertFalse(makeToolNameForCompiler("clang++", "g++", "gcc-ar").has_value());
  assertFalse(makeToolNameForCompiler("foo", "clang++", "llvm-ar").has_value());

  pass();
}

} // namespace tests

int main() { tests::testMakeToolNameForCompiler(); }

#endif
