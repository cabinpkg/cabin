#include "Compiler.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Rustify/Result.hpp"

#include <cstdlib>
#include <sstream>
#include <string>
#include <string_view>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

// TODO: The parsing of pkg-config output might not be robust.  It assumes
// that there wouldn't be backquotes or double quotes in the output, (should
// be treated as a single flag).  The current code just splits the output by
// space.

Result<CFlags>
CFlags::parsePkgConfig(const std::string_view pkgConfigVer) noexcept {
  const Command pkgConfigCmd =
      Command("pkg-config").addArg("--cflags").addArg(pkgConfigVer);
  std::string output = Try(getCmdOutput(pkgConfigCmd));
  output.pop_back();  // remove '\n'

  std::vector<Macro> macros;            // -D<name>=<val>
  std::vector<IncludeDir> includeDirs;  // -I<dir>
  std::vector<std::string> others;      // e.g., -pthread, -fPIC

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

  return Ok(CFlags(  //
      std::move(macros), std::move(includeDirs), std::move(others)));
}

void
CFlags::merge(const CFlags& other) noexcept {
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
  output.pop_back();  // remove '\n'

  std::vector<LibDir> libDirs;      // -L<dir>
  std::vector<Lib> libs;            // -l<lib>
  std::vector<std::string> others;  // e.g., -Wl,...

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

void
LdFlags::merge(const LdFlags& other) noexcept {
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

void
CompilerOpts::merge(const CompilerOpts& other) noexcept {
  cFlags.merge(other.cFlags);
  ldFlags.merge(other.ldFlags);
}

Compiler
Compiler::init(std::string cxx) noexcept {
  return Compiler(std::move(cxx));
}

Result<Compiler>
Compiler::init() noexcept {
  using std::string_view_literals::operator""sv;

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

  return Ok(Compiler::init(std::move(cxx)));
}

Command
Compiler::makeCompileCmd(const CompilerOpts& opts,
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

Command
Compiler::makeMMCmd(const CompilerOpts& opts,
                    const std::string& sourceFile) const {
  return Command(cxx)
      .addArgs(opts.cFlags.others)
      .addArgs(opts.cFlags.macros)
      .addArgs(opts.cFlags.includeDirs)
      .addArg("-MM")
      .addArg(sourceFile);
}

Command
Compiler::makePreprocessCmd(const CompilerOpts& opts,
                            const std::string& sourceFile) const {
  return Command(cxx)
      .addArg("-E")
      .addArgs(opts.cFlags.others)
      .addArgs(opts.cFlags.macros)
      .addArgs(opts.cFlags.includeDirs)
      .addArg(sourceFile);
}

}  // namespace cabin
