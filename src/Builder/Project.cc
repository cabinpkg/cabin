#include "Project.hpp"

#include "Algos.hpp"
#include "Builder/BuildProfile.hpp"
#include "Git2.hpp"
#include "TermColor.hpp"

#include <filesystem>
#include <rs/result.hpp>
#include <spdlog/spdlog.h>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

namespace cabin {

// Generally split the string by space character, but it will properly interpret
// the quotes and some escape sequences. (More specifically it will ignore
// whatever character that goes after a backslash and preserve all characters,
// usually used to pass an argument containing spaces, between quotes.)
static std::vector<std::string> parseEnvFlags(std::string_view env) {
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

static std::vector<std::string> getEnvFlags(const char* name) {
  if (const char* env = std::getenv(name)) {
    return parseEnvFlags(env);
  }
  return {};
}

Project::Project(const BuildProfile& buildProfile, Manifest m,
                 CompilerOpts opts)
    : rootPath(m.path.parent_path()),
      outBasePath(rootPath / "cabin-out" / fmt::format("{}", buildProfile)),
      buildOutPath(outBasePath / (m.package.name + ".d")),
      unittestOutPath(outBasePath / "unit"),
      integrationTestOutPath(outBasePath / "intg"), manifest(std::move(m)),
      compilerOpts(std::move(opts)) //
{
  includeIfExist(rootPath / "src", /*isSystem=*/false);
  includeIfExist(rootPath / "include", /*isSystem=*/false);
  if (buildProfile == BuildProfile::Test) {
    includeIfExist(rootPath / "tests", /*isSystem=*/false);
  }

  compilerOpts.cFlags.others.emplace_back("-std=c++"
                                          + manifest.package.edition.str);
  if (shouldColorStderr()) {
    compilerOpts.cFlags.others.emplace_back("-fdiagnostics-color");
  }

  const Profile& profile = manifest.profiles.at(buildProfile);
  if (profile.debug) {
    compilerOpts.cFlags.others.emplace_back("-g");
    compilerOpts.cFlags.macros.emplace_back("DEBUG", "");
  } else {
    compilerOpts.cFlags.macros.emplace_back("NDEBUG", "");
  }
  compilerOpts.cFlags.others.emplace_back(
      fmt::format("-O{}", profile.optLevel));
  if (profile.lto) {
    compilerOpts.cFlags.others.emplace_back("-flto");
    compilerOpts.ldFlags.others.emplace_back("-flto");
  }
  for (const std::string& flag : profile.cxxflags) {
    compilerOpts.cFlags.others.emplace_back(flag);
  }
  // Environment variables takes the highest precedence and will be appended at
  // last.
  for (const std::string& flag : getEnvFlags("CXXFLAGS")) {
    compilerOpts.cFlags.others.emplace_back(flag);
  }

  const std::string pkgName = toMacroName(manifest.package.name);
  const Version& pkgVersion = manifest.package.version;
  std::string commitHash;
  std::string commitShortHash;
  std::string commitDate;
  try {
    git2::Repository repo{};
    repo.open(rootPath.string());

    const git2::Oid oid = repo.refNameToId("HEAD");
    commitHash = oid.toString();
    commitShortHash = commitHash.substr(0, git2::SHORT_HASH_LEN);
    commitDate = git2::Commit().lookup(repo, oid).time().toString();
  } catch (const git2::Exception& e) {
    spdlog::trace("No git repository found");
  }

  // Variables Cabin sets for the user.
  using DefVal = std::variant<std::string, std::uint64_t>;
  const auto defValU64 = [](std::uint64_t x) {
    return DefVal(std::in_place_type<std::uint64_t>, x);
  };

  std::unordered_map<std::string_view, DefVal> defines;
  defines.emplace("PKG_NAME", manifest.package.name);
  defines.emplace("PKG_VERSION", pkgVersion.toString());
  defines.emplace("PKG_VERSION_MAJOR", defValU64(pkgVersion.major));
  defines.emplace("PKG_VERSION_MINOR", defValU64(pkgVersion.minor));
  defines.emplace("PKG_VERSION_PATCH", defValU64(pkgVersion.patch));
  defines.emplace("PKG_VERSION_PRE", pkgVersion.pre.toString());
  defines.emplace("PKG_VERSION_NUM", defValU64(pkgVersion.toNum()));
  defines.emplace("COMMIT_HASH", commitHash);
  defines.emplace("COMMIT_SHORT_HASH", commitShortHash);
  defines.emplace("COMMIT_DATE", commitDate);
  defines.emplace("PROFILE", fmt::format("{}", buildProfile));

  const auto quote = [](auto&& val) {
    if constexpr (std::is_same_v<std::decay_t<decltype(val)>, std::string>) {
      return fmt::format("'\"{}\"'", std::forward<decltype(val)>(val));
    } else {
      return fmt::format("{}", std::forward<decltype(val)>(val));
    }
  };
  for (auto&& [key, val] : defines) {
    std::string quoted = std::visit(quote, std::move(val));
    compilerOpts.cFlags.macros.emplace_back(
        fmt::format("CABIN_{}_{}", pkgName, key), std::move(quoted));
  }

  // LDFLAGS from manifest
  for (const std::string& flag : profile.ldflags) {
    compilerOpts.ldFlags.others.emplace_back(flag);
  }
  // Environment variables takes the highest precedence and will be appended at
  // last.
  for (const std::string& flag : getEnvFlags("LDFLAGS")) {
    compilerOpts.ldFlags.others.emplace_back(flag);
  }
};

void Project::includeIfExist(const fs::path& path, bool isSystem) {
  if (fs::exists(path)) {
    compilerOpts.cFlags.includeDirs.emplace_back(path, isSystem);
  }
}

Result<Project> Project::init(const BuildProfile& buildProfile,
                              const fs::path& rootDir) {
  Manifest manifest = Try(Manifest::tryParse(rootDir / Manifest::FILE_NAME));
  return Ok(Project(buildProfile, std::move(manifest), CompilerOpts()));
}

Result<Project> Project::init(const BuildProfile& buildProfile,
                              const Manifest& manifest) {
  return Ok(Project(buildProfile, manifest, CompilerOpts()));
}

} // namespace cabin

#ifdef CABIN_TEST

#  include <rs/tests.hpp>

namespace tests {

using namespace cabin; // NOLINT(build/namespaces,google-build-using-namespace)

static void testParseEnvFlags() {
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
      " \"-I/path/contains space\"  '-Lanother/path with/space' normal  ");
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsEscapeQuotes.size(), static_cast<std::size_t>(3));
  assertEq(argsEscapeQuotes[0], "-I/path/contains space");
  assertEq(argsEscapeQuotes[1], "-Lanother/path with/space");
  assertEq(argsEscapeQuotes[2], "normal");

  std::vector<std::string> argsEscapeMixed = parseEnvFlags(
      R"-( "-IMy \"Headers\"\\v1" '\?pattern' normal path/contain/\"quote\"
mixEverything" abc "\?\#   )-");
  // NOLINTNEXTLINE(*-magic-numbers)
  assertEq(argsEscapeMixed.size(), static_cast<std::size_t>(5));
  assertEq(argsEscapeMixed[0], R"(-IMy "Headers"\v1)");
  assertEq(argsEscapeMixed[1], "?pattern");
  assertEq(argsEscapeMixed[2], "normal");
  assertEq(argsEscapeMixed[3], "path/contain/\"quote\"");
  assertEq(argsEscapeMixed[4], "mixEverything abc ?#");

  pass();
}

} // namespace tests

int main() { tests::testParseEnvFlags(); }

#endif
