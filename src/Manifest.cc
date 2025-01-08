#include "Manifest.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Exception.hpp"
#include "Git2.hpp"
#include "Logger.hpp"
#include "Rustify.hpp"
#include "Semver.hpp"
#include "TermColor.hpp"
#include "VersionReq.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <fmt/core.h>
#include <fmt/std.h>
#include <optional>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

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
  auto version = Try(Version::tryParse(
      Try(toml::try_find<std::string>(val, "package", "version"))
  ));
  return Ok(Package(std::move(name), std::move(edition), std::move(version)));
}

static Result<std::size_t>
validateOptLevel(const std::size_t optLevel) noexcept {
  if (optLevel > 3) {
    // TODO: use toml::format_error for better diagnostics.
    Bail("opt_level must be between 0 and 3");
  }
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
    auto res = validateCxxflag(cxxflag);
    if (res.is_err()) {
      return Err(res.unwrap_err());
    }
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
  const mitama::maybe<const bool> lto =
      toml::try_find<bool>(val, "profile", "lto").ok();
  const mitama::maybe<const bool> debug =
      toml::try_find<bool>(val, "profile", "debug").ok();
  const mitama::maybe<std::size_t> optLevel =
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

static void
validateDepName(const std::string_view name) {
  if (name.empty()) {
    throw CabinError("dependency name is empty");
  }

  if (!std::isalnum(name.front())) {
    throw CabinError("dependency name must start with an alphanumeric character"
    );
  }
  if (!std::isalnum(name.back()) && name.back() != '+') {
    throw CabinError(
        "dependency name must end with an alphanumeric character or `+`"
    );
  }

  for (const char c : name) {
    if (!std::isalnum(c) && !ALLOWED_CHARS.contains(c)) {
      throw CabinError(
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
      throw CabinError(
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
      throw CabinError("dependency name must contain `.` wrapped by digits");
    }
  }

  std::unordered_map<char, int> charsFreq;
  for (const char c : name) {
    ++charsFreq[c];
  }

  if (charsFreq['/'] > 1) {
    throw CabinError("dependency name must not contain more than one `/`");
  }
  if (charsFreq['+'] != 0 && charsFreq['+'] != 2) {
    throw CabinError("dependency name must contain zero or two `+`");
  }
  if (charsFreq['+'] == 2) {
    if (name.find('+') + 1 != name.rfind('+')) {
      throw CabinError("`+` in the dependency name must be consecutive");
    }
  }
}

static GitDependency
parseGitDep(const std::string& name, const toml::table& info) {
  validateDepName(name);
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
  return { .name = name, .url = gitUrlStr, .target = target };
}

static PathDependency
parsePathDep(const std::string& name, const toml::table& info) {
  validateDepName(name);
  const auto& path = info.at("path");
  if (!path.is_string()) {
    throw CabinError("path dependency must be a string");
  }
  return { .name = name, .path = path.as_string() };
}

static SystemDependency
parseSystemDep(const std::string& name, const toml::table& info) {
  validateDepName(name);
  const auto& version = info.at("version");
  if (!version.is_string()) {
    throw CabinError("system dependency version must be a string");
  }

  const std::string versionReq = version.as_string();
  return { .name = name, .versionReq = VersionReq::parse(versionReq) };
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
        deps.emplace_back(parseGitDep(dep.first, info));
        continue;
      } else if (info.contains("system") && info.at("system").as_boolean()) {
        deps.emplace_back(parseSystemDep(dep.first, info));
        continue;
      } else if (info.contains("path")) {
        deps.emplace_back(parsePathDep(dep.first, info));
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

DepMetadata
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
  return { .includes = includes, .libs = "" };
}

DepMetadata
PathDependency::install() const {
  const fs::path installDir = fs::weakly_canonical(path);
  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    logger::debug("{} is already installed", name);
  } else {
    throw CabinError(installDir.string() + " can't be accessible as directory");
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
  return { .includes = includes, .libs = "" };
}

DepMetadata
SystemDependency::install() const {
  const std::string pkgConfigVer = versionReq.toPkgConfigString(name);
  const Command cflagsCmd =
      Command("pkg-config").addArg("--cflags").addArg(pkgConfigVer);
  const Command libsCmd =
      Command("pkg-config").addArg("--libs").addArg(pkgConfigVer);

  std::string cflags = getCmdOutput(cflagsCmd);
  cflags.pop_back();  // remove '\n'
  std::string libs = getCmdOutput(libsCmd);
  libs.pop_back();  // remove '\n'

  return { .includes = cflags, .libs = libs };
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

  Bail("{} not find in {} and its parents", Manifest::NAME, origCandDir);
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

std::vector<DepMetadata>
Manifest::installDeps(const bool includeDevDeps) const {
  std::vector<DepMetadata> installed;
  for (const auto& dep : dependencies) {
    std::visit(
        [&](const auto& arg) { installed.emplace_back(arg.install()); }, dep
    );
  }
  if (includeDevDeps) {
    for (const auto& dep : devDependencies) {
      std::visit(
          [&](const auto& arg) { installed.emplace_back(arg.install()); }, dep
      );
    }
  }
  return installed;
}

// Returns an error message if the package name is invalid.
std::optional<std::string>  // TODO: result-like types make more sense.
validatePackageName(const std::string_view name) noexcept {
  // Empty
  if (name.empty()) {
    return "must not be empty";
  }

  // Only one character
  if (name.size() == 1) {
    return "must be more than one character";
  }

  // Only lowercase letters, numbers, dashes, and underscores
  for (const char c : name) {
    if (!std::islower(c) && !std::isdigit(c) && c != '-' && c != '_') {
      return "must only contain lowercase letters, numbers, dashes, and "
             "underscores";
    }
  }

  // Start with a letter
  if (!std::isalpha(name[0])) {
    return "must start with a letter";
  }

  // End with a letter or digit
  if (!std::isalnum(name[name.size() - 1])) {
    return "must end with a letter or digit";
  }

  // Using C++ keywords
  const std::unordered_set<std::string_view> keywords = {
#include "Keywords.def"
  };
  if (keywords.contains(name)) {
    return "must not be a C++ keyword";
  }

  return std::nullopt;
}

#ifdef CABIN_TEST

#  include <climits>

namespace tests {

static void
testValidateDepName() {
  assertException<CabinError>(
      []() { validateDepName(""); }, "dependency name is empty"
  );
  assertException<CabinError>(
      []() { validateDepName("-"); },
      "dependency name must start with an alphanumeric character"
  );
  assertException<CabinError>(
      []() { validateDepName("1-"); },
      "dependency name must end with an alphanumeric character or `+`"
  );

  for (char c = 0; c < CHAR_MAX; ++c) {
    if (std::isalnum(c) || ALLOWED_CHARS.contains(c)) {
      continue;
    }
    assertException<CabinError>(
        [c]() { validateDepName("1" + std::string(1, c) + "1"); },
        "dependency name must be alphanumeric, `-`, `_`, `/`, `.`, or `+`"
    );
  }

  assertException<CabinError>(
      []() { validateDepName("1--1"); },
      "dependency name must not contain consecutive non-alphanumeric characters"
  );
  assertNoException([]() { validateDepName("1-1-1"); });

  assertNoException([]() { validateDepName("1.1"); });
  assertNoException([]() { validateDepName("1.1.1"); });
  assertException<CabinError>(
      []() { validateDepName("a.a"); },
      "dependency name must contain `.` wrapped by digits"
  );

  assertNoException([]() { validateDepName("a/b"); });
  assertException<CabinError>(
      []() { validateDepName("a/b/c"); },
      "dependency name must not contain more than one `/`"
  );

  assertException<CabinError>(
      []() { validateDepName("a+"); },
      "dependency name must contain zero or two `+`"
  );
  assertException<CabinError>(
      []() { validateDepName("a+++"); },
      "dependency name must contain zero or two `+`"
  );

  assertException<CabinError>(
      []() { validateDepName("a+b+c"); },
      "`+` in the dependency name must be consecutive"
  );

  // issue #921
  assertNoException([]() { validateDepName("gtkmm-4.0"); });
  assertNoException([]() { validateDepName("ncurses++"); });

  pass();
}

}  // namespace tests

int
main() {
  tests::testValidateDepName();
}

#endif
