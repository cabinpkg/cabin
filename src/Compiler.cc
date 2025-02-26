#include "Compiler.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Rustify/Result.hpp"

#include <string>
#include <string_view>
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
      std::move(macros), std::move(includeDirs), std::move(others)
  ));
}

void
CFlags::merge(const CFlags& other) noexcept {
  macros.insert(macros.end(), other.macros.begin(), other.macros.end());
  includeDirs.insert(
      includeDirs.end(), other.includeDirs.begin(), other.includeDirs.end()
  );
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

void
LdFlags::merge(const LdFlags& other) noexcept {
  libDirs.insert(libDirs.end(), other.libDirs.begin(), other.libDirs.end());
  libs.insert(libs.end(), other.libs.begin(), other.libs.end());
  others.insert(others.end(), other.others.begin(), other.others.end());
}

Result<CompilerOptions>
CompilerOptions::parsePkgConfig(
    const VersionReq& pkgVerReq, const std::string_view pkgName
) noexcept {
  const std::string pkgConfigVer = pkgVerReq.toPkgConfigString(pkgName);
  CFlags cFlags = Try(CFlags::parsePkgConfig(pkgConfigVer));
  LdFlags ldFlags = Try(LdFlags::parsePkgConfig(pkgConfigVer));
  return Ok(CompilerOptions(std::move(cFlags), std::move(ldFlags)));
}

void
CompilerOptions::merge(const CompilerOptions& other) noexcept {
  cFlags.merge(other.cFlags);
  ldFlags.merge(other.ldFlags);
}

}  // namespace cabin
