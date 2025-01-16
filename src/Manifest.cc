#include "Manifest.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Git2.hpp"
#include "Logger.hpp"
#include "Rustify/Result.hpp"
#include "Semver.hpp"
#include "VersionReq.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <fmt/core.h>
#include <optional>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace cabin {

static fs::path
getXdgCacheHome() noexcept {
  if (const char* envP = std::getenv("XDG_CACHE_HOME")) {
    return envP;
  }
  const fs::path userDir = std::getenv("HOME");
  return userDir / ".cache";
}

static const fs::path CACHE_DIR(getXdgCacheHome() / "cabin");
static const fs::path GIT_DIR(CACHE_DIR / "git");
static const fs::path GIT_SRC_DIR(GIT_DIR / "src");

static const std::unordered_set<char> ALLOWED_CHARS = {
  '-', '_', '/', '.', '+'  // allowed in the dependency name
};

Result<Edition>
Edition::tryFromString(std::string str) noexcept {
  if (str == "98") {
    return Ok(Edition(Edition::Cpp98, std::move(str)));
  } else if (str == "03") {
    return Ok(Edition(Edition::Cpp03, std::move(str)));
  } else if (str == "0x" || str == "11") {
    return Ok(Edition(Edition::Cpp11, std::move(str)));
  } else if (str == "1y" || str == "14") {
    return Ok(Edition(Edition::Cpp14, std::move(str)));
  } else if (str == "1z" || str == "17") {
    return Ok(Edition(Edition::Cpp17, std::move(str)));
  } else if (str == "2a" || str == "20") {
    return Ok(Edition(Edition::Cpp20, std::move(str)));
  } else if (str == "2b" || str == "23") {
    return Ok(Edition(Edition::Cpp23, std::move(str)));
  } else if (str == "2c") {
    return Ok(Edition(Edition::Cpp26, std::move(str)));
  }
  Bail("invalid edition");
}

Result<Package>
Package::tryFromToml(const toml::value& val) noexcept {
  auto name = Try(toml::try_find<std::string>(val, "package", "name"));
  auto edition = Try(Edition::tryFromString(
      Try(toml::try_find<std::string>(val, "package", "edition"))
  ));
  auto version = Try(Version::parse(
      Try(toml::try_find<std::string>(val, "package", "version"))
  ));
  return Ok(Package(std::move(name), std::move(edition), std::move(version)));
}

static Result<std::size_t>
validateOptLevel(const std::size_t optLevel) noexcept {
  // TODO: use toml::format_error for better diagnostics.
  Ensure(optLevel <= 3, "opt_level must be between 0 and 3");
  return Ok(optLevel);
}

static Result<void>
validateCxxflag(const std::string_view cxxflag) noexcept {
  // cxxflag must start with `-`
  if (cxxflag.empty() || cxxflag[0] != '-') {
    Bail("cxxflag must start with `-`");
  }

  // cxxflag only contains alphanumeric characters, `-`, `_`, `=`, `+`, `:`,
  // or `.`.
  for (const char c : cxxflag) {
    if (!std::isalnum(c) && c != '-' && c != '_' && c != '=' && c != '+'
        && c != ':' && c != '.') {
      Bail(
          "cxxflag must only contain alphanumeric characters, `-`, `_`, `=`, "
          "`+`, `:`, or `.`"
      );
    }
  }

  return Ok();
}

static Result<std::vector<std::string>>
validateCxxflags(const std::vector<std::string>& cxxflags) noexcept {
  for (const std::string& cxxflag : cxxflags) {
    Try(validateCxxflag(cxxflag));
  }
  return Ok(cxxflags);
}

static Result<std::unordered_map<std::string, Profile>>
parseProfiles(const toml::value& val) noexcept {
  std::unordered_map<std::string, Profile> profiles;

  // Base profile to propagate to other profiles
  const auto cxxflags =
      Try(validateCxxflags(toml::find_or_default<std::vector<std::string>>(
          val, "profile", "cxxflags"
      )));
  const mitama::maybe lto = toml::try_find<bool>(val, "profile", "lto").ok();
  const mitama::maybe debug =
      toml::try_find<bool>(val, "profile", "debug").ok();
  const mitama::maybe optLevel =
      toml::try_find<std::size_t>(val, "profile", "opt_level").ok();

  // Dev
  auto devCxxflags =
      Try(validateCxxflags(toml::find_or<std::vector<std::string>>(
          val, "profile", "dev", "cxxflags", cxxflags
      )));
  const auto devLto =
      toml::find_or<bool>(val, "profile", "dev", "lto", lto.unwrap_or(false));
  const auto devDebug = toml::find_or<bool>(
      val, "profile", "dev", "debug", debug.unwrap_or(true)
  );
  const auto devOptLevel = Try(validateOptLevel(toml::find_or<std::size_t>(
      val, "profile", "dev", "opt_level", optLevel.unwrap_or(0)
  )));
  profiles.insert(
      { "dev", Profile(std::move(devCxxflags), devLto, devDebug, devOptLevel) }
  );

  // Release
  auto relCxxflags =
      Try(validateCxxflags(toml::find_or<std::vector<std::string>>(
          val, "profile", "release", "cxxflags", cxxflags
      )));
  const auto relLto = toml::find_or<bool>(
      val, "profile", "release", "lto", lto.unwrap_or(false)
  );
  const auto relDebug = toml::find_or<bool>(
      val, "profile", "release", "debug", debug.unwrap_or(false)
  );
  const auto relOptLevel = Try(validateOptLevel(toml::find_or<std::size_t>(
      val, "profile", "release", "opt_level", optLevel.unwrap_or(3)
  )));
  profiles.insert(
      { "release",
        Profile(std::move(relCxxflags), relLto, relDebug, relOptLevel) }
  );

  return Ok(profiles);
}

Result<Cpplint>
Cpplint::tryFromToml(const toml::value& val) noexcept {
  auto filters = toml::find_or_default<std::vector<std::string>>(
      val, "lint", "cpplint", "filters"
  );
  return Ok(Cpplint(std::move(filters)));
}

Result<Lint>
Lint::tryFromToml(const toml::value& val) noexcept {
  auto cpplint = Try(Cpplint::tryFromToml(val));
  return Ok(Lint(std::move(cpplint)));
}

static Result<void>
validateDepName(const std::string_view name) noexcept {
  Ensure(!name.empty(), "dependency name must not be empty");
  Ensure(
      std::isalnum(name.front()),
      "dependency name must start with an alphanumeric character"
  );
  Ensure(
      std::isalnum(name.back()) || name.back() == '+',
      "dependency name must end with an alphanumeric character or `+`"
  );

  for (const char c : name) {
    if (!std::isalnum(c) && !ALLOWED_CHARS.contains(c)) {
      Bail(
          "dependency name must be alphanumeric, `-`, `_`, `/`, "
          "`.`, or `+`"
      );
    }
  }

  for (std::size_t i = 1; i < name.size(); ++i) {
    if (name[i] == '+') {
      // Allow consecutive `+` characters.
      continue;
    }

    if (!std::isalnum(name[i]) && name[i] == name[i - 1]) {
      Bail(
          "dependency name must not contain consecutive non-alphanumeric "
          "characters"
      );
    }
  }
  for (std::size_t i = 1; i < name.size() - 1; ++i) {
    if (name[i] != '.') {
      continue;
    }

    if (!std::isdigit(name[i - 1]) || !std::isdigit(name[i + 1])) {
      Bail("dependency name must contain `.` wrapped by digits");
    }
  }

  std::unordered_map<char, int> charsFreq;
  for (const char c : name) {
    ++charsFreq[c];
  }

  Ensure(
      charsFreq['/'] <= 1, "dependency name must not contain more than one `/`"
  );
  Ensure(
      charsFreq['+'] == 0 || charsFreq['+'] == 2,
      "dependency name must contain zero or two `+`"
  );
  if (charsFreq['+'] == 2) {
    if (name.find('+') + 1 != name.rfind('+')) {
      Bail("`+` in the dependency name must be consecutive");
    }
  }

  return Ok();
}

static Result<GitDependency>
parseGitDep(const std::string& name, const toml::table& info) noexcept {
  Try(validateDepName(name));
  std::string gitUrlStr;
  std::optional<std::string> target = std::nullopt;

  const auto& gitUrl = info.at("git");
  if (gitUrl.is_string()) {
    gitUrlStr = gitUrl.as_string();

    // rev, tag, or branch
    for (const char* key : { "rev", "tag", "branch" }) {
      if (info.contains(key)) {
        const auto& value = info.at(key);
        if (value.is_string()) {
          target = value.as_string();
          break;
        }
      }
    }
  }
  return Ok(GitDependency(name, gitUrlStr, std::move(target)));
}

static Result<PathDependency>
parsePathDep(const std::string& name, const toml::table& info) noexcept {
  Try(validateDepName(name));
  const auto& path = info.at("path");
  Ensure(path.is_string(), "path dependency must be a string");
  return Ok(PathDependency(name, path.as_string()));
}

static Result<SystemDependency>
parseSystemDep(const std::string& name, const toml::table& info) noexcept {
  Try(validateDepName(name));
  const auto& version = info.at("version");
  Ensure(version.is_string(), "system dependency version must be a string");

  const std::string versionReq = version.as_string();
  return Ok(SystemDependency(name, Try(VersionReq::parse(versionReq))));
}

static Result<std::vector<Dependency>>
parseDependencies(const toml::value& val, const char* key) noexcept {
  const auto tomlDeps = toml::try_find<toml::table>(val, key);
  if (tomlDeps.is_err()) {
    logger::debug("[{}] not found or not a table", key);
    return Ok(std::vector<Dependency>{});
  }

  std::vector<Dependency> deps;
  for (const auto& dep : tomlDeps.unwrap()) {
    if (dep.second.is_table()) {
      const auto& info = dep.second.as_table();
      if (info.contains("git")) {
        deps.emplace_back(Try(parseGitDep(dep.first, info)));
        continue;
      } else if (info.contains("system") && info.at("system").as_boolean()) {
        deps.emplace_back(Try(parseSystemDep(dep.first, info)));
        continue;
      } else if (info.contains("path")) {
        deps.emplace_back(Try(parsePathDep(dep.first, info)));
        continue;
      }
    }

    Bail(
        "Only Git dependency, path dependency, and system dependency are "
        "supported for now: {}",
        dep.first
    );
  }
  return Ok(deps);
}

Result<DepMetadata>
GitDependency::install() const {
  fs::path installDir = GIT_SRC_DIR / name;
  if (target.has_value()) {
    installDir += '-' + target.value();
  }

  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    logger::debug("{} is already installed", name);
  } else {
    git2::Repository repo;
    repo.clone(url, installDir.string());

    if (target.has_value()) {
      // Checkout to target.
      const std::string target = this->target.value();
      const git2::Object obj = repo.revparseSingle(target);
      repo.setHeadDetached(obj.id());
      repo.checkoutHead(true);
    }

    logger::info(
        "Downloaded", "{} {}", name, target.has_value() ? target.value() : url
    );
  }

  const fs::path includeDir = installDir / "include";
  std::string includes = "-isystem";

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    includes += includeDir.string();
  } else {
    includes += installDir.string();
  }

  // Currently, no libs are supported.
  return Ok(DepMetadata{ .includes = includes, .libs = "" });
}

Result<DepMetadata>
PathDependency::install() const {
  const fs::path installDir = fs::weakly_canonical(path);
  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    logger::debug("{} is already installed", name);
  } else {
    Bail("{} can't be accessible as directory", installDir.string());
  }

  const fs::path includeDir = installDir / "include";
  std::string includes = "-isystem";

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    includes += includeDir.string();
  } else {
    includes += installDir.string();
  }

  // Currently, no libs are supported.
  return Ok(DepMetadata{ .includes = includes, .libs = "" });
}

Result<DepMetadata>
SystemDependency::install() const {
  const std::string pkgConfigVer = versionReq.toPkgConfigString(name);
  const Command cflagsCmd =
      Command("pkg-config").addArg("--cflags").addArg(pkgConfigVer);
  const Command libsCmd =
      Command("pkg-config").addArg("--libs").addArg(pkgConfigVer);

  std::string cflags = Try(getCmdOutput(cflagsCmd));
  cflags.pop_back();  // remove '\n'
  std::string libs = Try(getCmdOutput(libsCmd));
  libs.pop_back();  // remove '\n'

  return Ok(DepMetadata{ .includes = cflags, .libs = libs });
}

Result<fs::path>
findManifest(fs::path candidateDir) noexcept {
  const fs::path origCandDir = candidateDir;
  while (true) {
    const fs::path configPath = candidateDir / Manifest::NAME;
    logger::trace("Finding manifest: {}", configPath.string());
    if (fs::exists(configPath)) {
      return Ok(configPath);
    }

    const fs::path parentPath = candidateDir.parent_path();
    if (candidateDir.has_parent_path()
        && parentPath != candidateDir.root_directory()) {
      candidateDir = parentPath;
    } else {
      break;
    }
  }

  Bail(
      "{} not find in `{}` and its parents", Manifest::NAME,
      origCandDir.string()
  );
}

Result<Manifest>
Manifest::tryParse(fs::path path, const bool findParents) noexcept {
  if (findParents) {
    path = Try(findManifest(path.parent_path()));
  }
  return Manifest::tryFromToml(toml::parse(path), path);
}

Result<Manifest>
Manifest::tryFromToml(const toml::value& data, fs::path path) noexcept {
  auto package = Try(Package::tryFromToml(data));
  std::vector<Dependency> dependencies =
      Try(parseDependencies(data, "dependencies"));
  std::vector<Dependency> devDependencies =
      Try(parseDependencies(data, "dev-dependencies"));
  std::unordered_map<std::string, Profile> profiles = Try(parseProfiles(data));
  auto lint = Try(Lint::tryFromToml(data));

  return Ok(Manifest(
      std::move(path), std::move(package), std::move(dependencies),
      std::move(devDependencies), std::move(profiles), std::move(lint)
  ));
}

Result<std::vector<DepMetadata>>
Manifest::installDeps(const bool includeDevDeps) const {
  std::vector<DepMetadata> installed;
  const auto install = [&](const auto& arg) -> Result<void> {
    installed.emplace_back(Try(arg.install()));
    return Ok();
  };

  for (const auto& dep : dependencies) {
    Try(std::visit(install, dep));
  }
  if (includeDevDeps) {
    for (const auto& dep : devDependencies) {
      Try(std::visit(install, dep));
    }
  }
  return Ok(installed);
}

// Returns an error message if the package name is invalid.
Result<void>
validatePackageName(const std::string_view name) noexcept {
  Ensure(!name.empty(), "package name must not be empty");
  Ensure(name.size() > 1, "package name must be more than one character");

  for (const char c : name) {
    if (!std::islower(c) && !std::isdigit(c) && c != '-' && c != '_') {
      Bail(
          "package name must only contain lowercase letters, numbers, dashes, "
          "and underscores"
      );
    }
  }

  Ensure(std::isalpha(name[0]), "package name must start with a letter");
  Ensure(
      std::isalnum(name[name.size() - 1]),
      "package name must end with a letter or digit"
  );

  const std::unordered_set<std::string_view> keywords = {
#include "Keywords.def"
  };
  Ensure(!keywords.contains(name), "package name must not be a C++ keyword");

  return Ok();
}

}  // namespace cabin

#ifdef CABIN_TEST

#  include <climits>
#  include <fmt/ranges.h>
#  include <toml11/fwd/literal_fwd.hpp>

namespace tests {

// NOLINTBEGIN
using namespace cabin;
using namespace toml::literals::toml_literals;
// NOLINTEND

inline static void
assertEditionEq(
    const Edition::Year left, const Edition::Year right,
    const std::source_location& loc = std::source_location::current()
) {
  assertEq(static_cast<uint16_t>(left), static_cast<uint16_t>(right), "", loc);
}
inline static void
assertEditionEq(
    const Edition& left, const Edition::Year right,
    const std::source_location& loc = std::source_location::current()
) {
  assertEditionEq(left.edition, right, loc);
}

static void
testEditionTryFromString() {  // Valid editions
  assertEditionEq(Edition::tryFromString("98").unwrap(), Edition::Cpp98);
  assertEditionEq(Edition::tryFromString("03").unwrap(), Edition::Cpp03);
  assertEditionEq(Edition::tryFromString("0x").unwrap(), Edition::Cpp11);
  assertEditionEq(Edition::tryFromString("11").unwrap(), Edition::Cpp11);
  assertEditionEq(Edition::tryFromString("1y").unwrap(), Edition::Cpp14);
  assertEditionEq(Edition::tryFromString("14").unwrap(), Edition::Cpp14);
  assertEditionEq(Edition::tryFromString("1z").unwrap(), Edition::Cpp17);
  assertEditionEq(Edition::tryFromString("17").unwrap(), Edition::Cpp17);
  assertEditionEq(Edition::tryFromString("2a").unwrap(), Edition::Cpp20);
  assertEditionEq(Edition::tryFromString("20").unwrap(), Edition::Cpp20);
  assertEditionEq(Edition::tryFromString("2b").unwrap(), Edition::Cpp23);
  assertEditionEq(Edition::tryFromString("23").unwrap(), Edition::Cpp23);
  assertEditionEq(Edition::tryFromString("2c").unwrap(), Edition::Cpp26);

  // Invalid editions
  assertEq(Edition::tryFromString("").unwrap_err()->what(), "invalid edition");
  assertEq(
      Edition::tryFromString("abc").unwrap_err()->what(), "invalid edition"
  );
  assertEq(
      Edition::tryFromString("99").unwrap_err()->what(), "invalid edition"
  );
  assertEq(
      Edition::tryFromString("21").unwrap_err()->what(), "invalid edition"
  );

  pass();
}

static void
testEditionComparison() {
  assertTrue(
      Edition::tryFromString("98").unwrap()
      <= Edition::tryFromString("03").unwrap()
  );
  assertTrue(
      Edition::tryFromString("03").unwrap()
      <= Edition::tryFromString("11").unwrap()
  );
  assertTrue(
      Edition::tryFromString("11").unwrap()
      <= Edition::tryFromString("14").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      <= Edition::tryFromString("17").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      <= Edition::tryFromString("20").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      <= Edition::tryFromString("23").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      <= Edition::tryFromString("2c").unwrap()
  );

  assertTrue(
      Edition::tryFromString("98").unwrap()
      < Edition::tryFromString("03").unwrap()
  );
  assertTrue(
      Edition::tryFromString("03").unwrap()
      < Edition::tryFromString("11").unwrap()
  );
  assertTrue(
      Edition::tryFromString("11").unwrap()
      < Edition::tryFromString("14").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      < Edition::tryFromString("17").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      < Edition::tryFromString("20").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      < Edition::tryFromString("23").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      < Edition::tryFromString("2c").unwrap()
  );

  assertTrue(
      Edition::tryFromString("11").unwrap()
      == Edition::tryFromString("0x").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      == Edition::tryFromString("1y").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      == Edition::tryFromString("1z").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      == Edition::tryFromString("2a").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      == Edition::tryFromString("2b").unwrap()
  );

  assertTrue(
      Edition::tryFromString("11").unwrap()
      != Edition::tryFromString("03").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      != Edition::tryFromString("11").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      != Edition::tryFromString("14").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      != Edition::tryFromString("17").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      != Edition::tryFromString("20").unwrap()
  );

  assertTrue(
      Edition::tryFromString("2c").unwrap()
      > Edition::tryFromString("23").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      > Edition::tryFromString("20").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      > Edition::tryFromString("17").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      > Edition::tryFromString("14").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      > Edition::tryFromString("11").unwrap()
  );
  assertTrue(
      Edition::tryFromString("11").unwrap()
      > Edition::tryFromString("03").unwrap()
  );
  assertTrue(
      Edition::tryFromString("03").unwrap()
      > Edition::tryFromString("98").unwrap()
  );

  assertTrue(
      Edition::tryFromString("2c").unwrap()
      >= Edition::tryFromString("23").unwrap()
  );
  assertTrue(
      Edition::tryFromString("23").unwrap()
      >= Edition::tryFromString("20").unwrap()
  );
  assertTrue(
      Edition::tryFromString("20").unwrap()
      >= Edition::tryFromString("17").unwrap()
  );
  assertTrue(
      Edition::tryFromString("17").unwrap()
      >= Edition::tryFromString("14").unwrap()
  );
  assertTrue(
      Edition::tryFromString("14").unwrap()
      >= Edition::tryFromString("11").unwrap()
  );
  assertTrue(
      Edition::tryFromString("11").unwrap()
      >= Edition::tryFromString("03").unwrap()
  );
  assertTrue(
      Edition::tryFromString("03").unwrap()
      >= Edition::tryFromString("98").unwrap()
  );

  assertTrue(Edition::tryFromString("17").unwrap() <= Edition::Cpp17);
  assertTrue(Edition::tryFromString("17").unwrap() < Edition::Cpp20);
  assertTrue(Edition::tryFromString("20").unwrap() == Edition::Cpp20);
  assertTrue(Edition::tryFromString("20").unwrap() != Edition::Cpp23);
  assertTrue(Edition::tryFromString("23").unwrap() > Edition::Cpp20);
  assertTrue(Edition::tryFromString("20").unwrap() >= Edition::Cpp20);

  pass();
}

static void
testPackageTryFromToml() {
  // Valid package
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
      version = "1.2.3"
)"_toml;

    auto pkg = Package::tryFromToml(val).unwrap();
    assertEq(pkg.name, "test-pkg");
    assertEq(pkg.edition.str, "20");
    assertEq(pkg.version.toString(), "1.2.3");
  }

  // Missing fields
  {
    const toml::value val = R"(
      [package]
)"_toml;

    assertEq(
        Package::tryFromToml(val).unwrap_err()->what(),
        R"([error] toml::value::at: key "name" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table
)"
    );
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
)"_toml;

    assertEq(
        Package::tryFromToml(val).unwrap_err()->what(),
        R"([error] toml::value::at: key "edition" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table
)"
    );
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
)"_toml;

    assertEq(
        Package::tryFromToml(val).unwrap_err()->what(),
        R"([error] toml::value::at: key "version" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table
)"
    );
  }

  // Invalid fields
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "invalid"
      version = "1.2.3"
)"_toml;

    assertEq(Package::tryFromToml(val).unwrap_err()->what(), "invalid edition");
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
      version = "invalid"
)"_toml;

    assertEq(
        Package::tryFromToml(val).unwrap_err()->what(),
        R"(invalid semver:
invalid
^^^^^^^ expected number)"
    );
  }

  pass();
}

static void
testLintTryFromToml() {
  // Basic lint config
  {
    const toml::value val = R"(
      [lint.cpplint]
      filters = [
        "+filter1",
        "-filter2"
      ]
)"_toml;

    auto lint = Lint::tryFromToml(val).unwrap();
    assertEq(
        fmt::format("{}", fmt::join(lint.cpplint.filters, ",")),
        fmt::format(
            "{}",
            fmt::join(std::vector<std::string>{ "+filter1", "-filter2" }, ",")
        )
    );
  }

  // Empty lint config
  {
    const toml::value val{};
    auto lint = Lint::tryFromToml(val).unwrap();
    assertTrue(lint.cpplint.filters.empty());
  }

  pass();
}

static void
testValidateDepName() {
  assertEq(
      validateDepName("").unwrap_err()->what(),
      "dependency name must not be empty"
  );
  assertEq(
      validateDepName("-").unwrap_err()->what(),
      "dependency name must start with an alphanumeric character"
  );
  assertEq(
      validateDepName("1-").unwrap_err()->what(),
      "dependency name must end with an alphanumeric character or `+`"
  );

  for (char c = 0; c < CHAR_MAX; ++c) {
    if (std::isalnum(c) || ALLOWED_CHARS.contains(c)) {
      continue;
    }
    assertEq(
        validateDepName("1" + std::string(1, c) + "1").unwrap_err()->what(),
        "dependency name must be alphanumeric, `-`, `_`, `/`, `.`, or `+`"
    );
  }

  assertEq(
      validateDepName("1--1").unwrap_err()->what(),
      "dependency name must not contain consecutive non-alphanumeric characters"
  );
  assertTrue(validateDepName("1-1-1").is_ok());

  assertTrue(validateDepName("1.1").is_ok());
  assertTrue(validateDepName("1.1.1").is_ok());
  assertEq(
      validateDepName("a.a").unwrap_err()->what(),
      "dependency name must contain `.` wrapped by digits"
  );

  assertTrue(validateDepName("a/b").is_ok());
  assertEq(
      validateDepName("a/b/c").unwrap_err()->what(),
      "dependency name must not contain more than one `/`"
  );

  assertEq(
      validateDepName("a+").unwrap_err()->what(),
      "dependency name must contain zero or two `+`"
  );
  assertEq(
      validateDepName("a+++").unwrap_err()->what(),
      "dependency name must contain zero or two `+`"
  );

  assertEq(
      validateDepName("a+b+c").unwrap_err()->what(),
      "`+` in the dependency name must be consecutive"
  );

  // issue #921
  assertTrue(validateDepName("gtkmm-4.0").is_ok());
  assertTrue(validateDepName("ncurses++").is_ok());

  pass();
}

}  // namespace tests

int
main() {
  tests::testEditionTryFromString();
  tests::testEditionComparison();
  tests::testPackageTryFromToml();
  tests::testLintTryFromToml();
  tests::testValidateDepName();
}

#endif
