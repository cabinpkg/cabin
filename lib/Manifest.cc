#include "Manifest.hpp"

#include "Builder/BuildProfile.hpp"
#include "Builder/Builder.hpp"
#include "Builder/Compiler.hpp"
#include "Diag.hpp"
#include "Semver.hpp"
#include "TermColor.hpp"
#include "VersionReq.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <fmt/core.h>
#include <optional>
#include <ranges>
#include <rs/result.hpp>
#include <spdlog/spdlog.h>
#include <string>
#include <string_view>
#include <system_error>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace toml {

template <typename T, typename... U>
// NOLINTNEXTLINE(readability-identifier-naming,cppcoreguidelines-macro-usage)
inline rs::Result<T> try_find(const toml::value& v, const U&... u) noexcept {
  using std::string_view_literals::operator""sv;

  if (cabin::shouldColorStderr()) {
    color::enable();
  } else {
    color::disable();
  }

  try {
    return rs::Ok(toml::find<T>(v, u...));
  } catch (const std::exception& e) {
    std::string what = e.what();

    static constexpr std::size_t errorPrefixSize = "[error] "sv.size();
    static constexpr std::size_t colorErrorPrefixSize =
        "\033[31m\033[01m[error]\033[00m "sv.size();

    if (cabin::shouldColorStderr()) {
      what = what.substr(colorErrorPrefixSize);
    } else {
      what = what.substr(errorPrefixSize);
    }

    if (what.back() == '\n') {
      what.pop_back(); // remove the last '\n' since Diag::error adds one.
    }
    return rs::Err(rs::anyhow(what));
  }
}

} // namespace toml

namespace cabin {

static const std::unordered_set<char> ALLOWED_CHARS = {
  '-', '_', '/', '.', '+' // allowed in the dependency name
};

template <typename... Ts>
struct Overloaded : Ts... {
  using Ts::operator()...;
};
template <typename... Ts>
Overloaded(Ts...) -> Overloaded<Ts...>;

enum class DepKind : std::uint8_t { Git, Path, System };

struct DepKey {
  DepKind kind;
  std::string detail;

  bool operator==(const DepKey& other) const = default;
};

static fs::path resolveIncludeDir(const fs::path& installDir) {
  fs::path includeDir = installDir / "include";
  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    return includeDir;
  }
  return installDir;
}

static fs::path canonicalizePathDep(const fs::path& baseDir,
                                    const std::string& relPath) {
  std::error_code ec;
  fs::path depRoot = fs::weakly_canonical(baseDir / relPath, ec);
  if (ec) {
    depRoot = fs::absolute(baseDir / relPath);
  }
  return depRoot;
}

static rs::Result<void>
installDependencies(const Manifest& manifest, const BuildProfile& buildProfile,
                    bool includeDevDeps, bool suppressDepDiag,
                    std::unordered_map<std::string, DepKey>& seenDeps,
                    std::unordered_set<fs::path>& visited,
                    std::vector<CompilerOpts>& installed);

static rs::Result<void>
installPathDependency(const Manifest& manifest, const PathDependency& pathDep,
                      const BuildProfile& buildProfile, bool includeDevDeps,
                      bool suppressDepDiag,
                      std::unordered_map<std::string, DepKey>& seenDeps,
                      std::unordered_set<fs::path>& visited,
                      std::vector<CompilerOpts>& installed) {
  const fs::path basePath = manifest.path.parent_path();
  const fs::path depRoot = canonicalizePathDep(basePath, pathDep.path);

  rs_ensure(fs::exists(depRoot) && fs::is_directory(depRoot),
            "{} can't be accessible as directory", depRoot.string());
  if (!visited.insert(depRoot).second) {
    return rs::Ok();
  }

  CompilerOpts pathOpts;
  const fs::path depManifestPath = depRoot / Manifest::FILE_NAME;
  rs_ensure(fs::exists(depManifestPath), "missing `{}` in path dependency {}",
            Manifest::FILE_NAME, depRoot.string());
  const Manifest depManifest =
      rs_try(Manifest::tryParse(depManifestPath, false));

  if (!suppressDepDiag) {
    Diag::info("Building", "{} ({})", depManifest.package.name,
               depRoot.string());
  }

  Builder depBuilder(depRoot, buildProfile);
  ScheduleOptions depOptions;
  depOptions.includeDevDeps = includeDevDeps;
  depOptions.suppressAnalysisLog = true;
  depOptions.suppressFinishLog = true;
  depOptions.suppressDepDiag = suppressDepDiag;
  rs_try(depBuilder.schedule(depOptions));
  rs_try(depBuilder.build());

  const BuildGraph& depGraph = depBuilder.graph();
  const fs::path depOutDir = depGraph.outBasePath();
  const fs::path libPath = depOutDir / depGraph.libraryName();

  pathOpts.cFlags.includeDirs.emplace_back(resolveIncludeDir(depRoot),
                                           /*isSystem=*/false);

  std::vector<CompilerOpts> nestedDeps;
  rs_try(installDependencies(depManifest, buildProfile, includeDevDeps,
                             suppressDepDiag, seenDeps, visited, nestedDeps));
  for (const CompilerOpts& opts : nestedDeps) {
    pathOpts.merge(opts);
  }

  const bool libBuilt = fs::exists(libPath);
  if (depGraph.hasLibraryTarget()) {
    rs_ensure(libBuilt, "expected `{}` to be built for dependency {}",
              libPath.string(), depManifest.package.name);
  }

  if (libBuilt) {
    pathOpts.ldFlags.libDirs.emplace(pathOpts.ldFlags.libDirs.begin(),
                                     libPath.parent_path());

    std::string libName = libPath.stem().string();
    if (libName.starts_with("lib")) {
      libName.erase(0, 3);
    }
    pathOpts.ldFlags.libs.emplace(pathOpts.ldFlags.libs.begin(),
                                  std::move(libName));
  }

  installed.emplace_back(std::move(pathOpts));
  return rs::Ok();
}

static DepKey makeDepKey(const Manifest& manifest, const Dependency& dep) {
  const fs::path basePath = manifest.path.parent_path();
  return std::visit(
      Overloaded{
          [&](const GitDependency& gitDep) -> DepKey {
            std::string detail = gitDep.url;
            if (gitDep.target.has_value()) {
              detail.push_back('#');
              detail.append(gitDep.target.value());
            }
            return DepKey{ .kind = DepKind::Git, .detail = std::move(detail) };
          },
          [&](const SystemDependency& sysDep) -> DepKey {
            return DepKey{ .kind = DepKind::System,
                           .detail = sysDep.versionReq.toString() };
          },
          [&](const PathDependency& pathDep) -> DepKey {
            const fs::path canon = canonicalizePathDep(basePath, pathDep.path);
            return DepKey{ .kind = DepKind::Path,
                           .detail = canon.generic_string() };
          },
      },
      dep);
}

static const std::string& depName(const Dependency& dep) {
  return std::visit(
      Overloaded{
          [](const GitDependency& gitDep) -> const std::string& {
            return gitDep.name;
          },
          [](const SystemDependency& sysDep) -> const std::string& {
            return sysDep.name;
          },
          [](const PathDependency& pathDep) -> const std::string& {
            return pathDep.name;
          },
      },
      dep);
}

static rs::Result<void>
rememberDep(const Manifest& manifest, const Dependency& dep,
            std::unordered_map<std::string, DepKey>& seen) {
  const DepKey key = makeDepKey(manifest, dep);
  const std::string& name = depName(dep);
  const auto [it, inserted] = seen.emplace(name, key);
  if (inserted) {
    return rs::Ok();
  }
  if (it->second == key) {
    return rs::Ok();
  }
  rs_bail("dependency `{}` conflicts across manifests", name);
}

static rs::Result<void>
installDependencies(const Manifest& manifest, const BuildProfile& buildProfile,
                    const bool includeDevDeps, const bool suppressDepDiag,
                    std::unordered_map<std::string, DepKey>& seenDeps,
                    std::unordered_set<fs::path>& visited,
                    std::vector<CompilerOpts>& installed) {
  const auto installOne = [&](const Dependency& dep) -> rs::Result<void> {
    rs_try(rememberDep(manifest, dep, seenDeps));
    return std::visit(
        Overloaded{
            [&](const GitDependency& gitDep) -> rs::Result<void> {
              CompilerOpts depOpts = rs_try(gitDep.install());

              const fs::path depManifestPath =
                  gitDep.installDir() / Manifest::FILE_NAME;
              if (fs::exists(depManifestPath)) {
                const Manifest depManifest =
                    rs_try(Manifest::tryParse(depManifestPath, false));

                std::vector<CompilerOpts> nestedDeps;
                rs_try(installDependencies(depManifest, buildProfile,
                                           includeDevDeps, suppressDepDiag,
                                           seenDeps, visited, nestedDeps));
                for (const CompilerOpts& opts : nestedDeps) {
                  depOpts.merge(opts);
                }
              }

              installed.emplace_back(std::move(depOpts));
              return rs::Ok();
            },
            [&](const SystemDependency& sysDep) -> rs::Result<void> {
              installed.emplace_back(rs_try(sysDep.install()));
              return rs::Ok();
            },
            [&](const PathDependency& pathDep) -> rs::Result<void> {
              return installPathDependency(manifest, pathDep, buildProfile,
                                           includeDevDeps, suppressDepDiag,
                                           seenDeps, visited, installed);
            },
        },
        dep);
  };

  for (const auto& dep : manifest.dependencies) {
    rs_try(installOne(dep));
  }
  if (includeDevDeps && manifest.path == Manifest::tryParse().unwrap().path) {
    for (const auto& dep : manifest.devDependencies) {
      rs_try(installOne(dep));
    }
  }

  return rs::Ok();
}

rs::Result<Edition> Edition::tryFromString(std::string str) noexcept {
  if (str == "98") {
    return rs::Ok(Edition(Edition::Cpp98, std::move(str)));
  } else if (str == "03") {
    return rs::Ok(Edition(Edition::Cpp03, std::move(str)));
  } else if (str == "0x" || str == "11") {
    return rs::Ok(Edition(Edition::Cpp11, std::move(str)));
  } else if (str == "1y" || str == "14") {
    return rs::Ok(Edition(Edition::Cpp14, std::move(str)));
  } else if (str == "1z" || str == "17") {
    return rs::Ok(Edition(Edition::Cpp17, std::move(str)));
  } else if (str == "2a" || str == "20") {
    return rs::Ok(Edition(Edition::Cpp20, std::move(str)));
  } else if (str == "2b" || str == "23") {
    return rs::Ok(Edition(Edition::Cpp23, std::move(str)));
  } else if (str == "2c") {
    return rs::Ok(Edition(Edition::Cpp26, std::move(str)));
  }
  rs_bail("invalid edition");
}

rs::Result<Package> Package::tryFromToml(const toml::value& val) noexcept {
  auto name = rs_try(toml::try_find<std::string>(val, "package", "name"));
  auto edition = rs_try(Edition::tryFromString(
      rs_try(toml::try_find<std::string>(val, "package", "edition"))));
  auto version = rs_try(Version::parse(
      rs_try(toml::try_find<std::string>(val, "package", "version"))));
  return rs::Ok(
      Package(std::move(name), std::move(edition), std::move(version)));
}

static rs::Result<std::uint8_t>
validateOptLevel(const std::uint8_t optLevel) noexcept {
  // TODO: use toml::format_error for better diagnostics.
  rs_ensure(optLevel <= 3, "opt-level must be between 0 and 3");
  return rs::Ok(optLevel);
}

static rs::Result<void> validateFlag(const char* type,
                                     const std::string_view flag) noexcept {
  rs_ensure(!flag.empty() && flag[0] == '-', "{} must start with `-`", type);

  static const std::unordered_set<char> allowed{
    '-', '_', '=', '+', ':', '.', ',' // `-fsanitize=address,undefined`
  };
  std::unordered_map<char, bool> allowedOnce{
    { ' ', false }, // `-framework Metal`
  };
  for (const char c : flag) {
    if (allowedOnce.contains(c)) {
      rs_ensure(!allowedOnce[c], "{} must only contain {} once", type,
                allowedOnce | std::views::keys);
      allowedOnce[c] = true;
      continue;
    }
    rs_ensure(std::isalnum(c) || allowed.contains(c),
              "{} must only contain {} or alphanumeric characters", type,
              allowed);
  }

  return rs::Ok();
}

static rs::Result<std::vector<std::string>>
validateFlags(const char* type,
              const std::vector<std::string>& flags) noexcept {
  for (const std::string& flag : flags) {
    rs_try(validateFlag(type, flag));
  }
  return rs::Ok(flags);
}

struct BaseProfile {
  const std::vector<std::string> cxxflags;
  const std::vector<std::string> ldflags;
  const bool lto;
  const mitama::maybe<bool> debug;
  const mitama::maybe<std::uint8_t> optLevel;

  BaseProfile(std::vector<std::string> cxxflags,
              std::vector<std::string> ldflags, const bool lto,
              const mitama::maybe<bool> debug,
              const mitama::maybe<std::uint8_t> optLevel) noexcept
      : cxxflags(std::move(cxxflags)), ldflags(std::move(ldflags)), lto(lto),
        debug(debug), optLevel(optLevel) {}
};

static rs::Result<BaseProfile>
parseBaseProfile(const toml::value& val) noexcept {
  auto cxxflags = rs_try(
      validateFlags("cxxflags", toml::find_or_default<std::vector<std::string>>(
                                    val, "profile", "cxxflags")));
  auto ldflags = rs_try(
      validateFlags("ldflags", toml::find_or_default<std::vector<std::string>>(
                                   val, "profile", "ldflags")));
  const bool lto = toml::try_find<bool>(val, "profile", "lto").unwrap_or(false);
  const mitama::maybe debug =
      toml::try_find<bool>(val, "profile", "debug").ok();
  const mitama::maybe optLevel =
      toml::try_find<std::uint8_t>(val, "profile", "opt-level").ok();

  return rs::Ok(BaseProfile(std::move(cxxflags), std::move(ldflags), lto, debug,
                            optLevel));
}

static rs::Result<Profile>
parseDevProfile(const toml::value& val,
                const BaseProfile& baseProfile) noexcept {
  static constexpr const char* key = "dev";

  auto cxxflags = rs_try(validateFlags(
      "cxxflags", toml::find_or<std::vector<std::string>>(
                      val, "profile", key, "cxxflags", baseProfile.cxxflags)));
  auto ldflags = rs_try(validateFlags(
      "ldflags", toml::find_or<std::vector<std::string>>(
                     val, "profile", key, "ldflags", baseProfile.ldflags)));
  const auto lto =
      toml::find_or<bool>(val, "profile", key, "lto", baseProfile.lto);
  const auto debug = toml::find_or<bool>(val, "profile", key, "debug",
                                         baseProfile.debug.unwrap_or(true));
  const auto optLevel = rs_try(validateOptLevel(toml::find_or<std::uint8_t>(
      val, "profile", key, "opt-level", baseProfile.optLevel.unwrap_or(0))));

  return rs::Ok(
      Profile(std::move(cxxflags), std::move(ldflags), lto, debug, optLevel));
}

static rs::Result<Profile>
parseReleaseProfile(const toml::value& val,
                    const BaseProfile& baseProfile) noexcept {
  static constexpr const char* key = "release";

  auto cxxflags = rs_try(validateFlags(
      "cxxflags", toml::find_or<std::vector<std::string>>(
                      val, "profile", key, "cxxflags", baseProfile.cxxflags)));
  auto ldflags = rs_try(validateFlags(
      "ldflags", toml::find_or<std::vector<std::string>>(
                     val, "profile", key, "ldflags", baseProfile.ldflags)));
  const auto lto =
      toml::find_or<bool>(val, "profile", key, "lto", baseProfile.lto);
  const auto debug = toml::find_or<bool>(val, "profile", key, "debug",
                                         baseProfile.debug.unwrap_or(false));
  const auto optLevel = rs_try(validateOptLevel(toml::find_or<std::uint8_t>(
      val, "profile", key, "opt-level", baseProfile.optLevel.unwrap_or(3))));

  return rs::Ok(
      Profile(std::move(cxxflags), std::move(ldflags), lto, debug, optLevel));
}

enum class InheritMode : uint8_t {
  Append,
  Overwrite,
};

static rs::Result<InheritMode>
parseInheritMode(std::string_view mode) noexcept {
  if (mode == "append") {
    return rs::Ok(InheritMode::Append);
  } else if (mode == "overwrite") {
    return rs::Ok(InheritMode::Overwrite);
  } else {
    rs_bail("invalid inherit-mode: `{}`", mode);
  }
}

static std::vector<std::string>
inheritFlags(const InheritMode inheritMode,
             const std::vector<std::string>& baseFlags,
             const std::vector<std::string>& newFlags) noexcept {
  if (newFlags.empty()) {
    return baseFlags; // No change, use base flags.
  }

  if (inheritMode == InheritMode::Append) {
    // Append new flags to the base flags.
    std::vector<std::string> merged = baseFlags;
    merged.insert(merged.end(), newFlags.begin(), newFlags.end());
    return merged;
  } else {
    // Overwrite base flags with new flags.
    return newFlags;
  }
}

// Inherits from `dev`.
static rs::Result<Profile>
parseTestProfile(const toml::value& val, const Profile& devProfile) noexcept {
  static constexpr const char* key = "test";

  const InheritMode inheritMode =
      rs_try(parseInheritMode(toml::find_or<std::string>(
          val, "profile", key, "inherit-mode", "append")));
  std::vector<std::string> cxxflags = inheritFlags(
      inheritMode, devProfile.cxxflags,
      rs_try(validateFlags("cxxflags",
                           toml::find_or_default<std::vector<std::string>>(
                               val, "profile", key, "cxxflags"))));
  std::vector<std::string> ldflags = inheritFlags(
      inheritMode, devProfile.ldflags,
      rs_try(validateFlags("ldflags",
                           toml::find_or_default<std::vector<std::string>>(
                               val, "profile", key, "ldflags"))));
  const auto lto =
      toml::find_or<bool>(val, "profile", key, "lto", devProfile.lto);
  const auto debug =
      toml::find_or<bool>(val, "profile", key, "debug", devProfile.debug);
  const auto optLevel = rs_try(validateOptLevel(toml::find_or<std::uint8_t>(
      val, "profile", key, "opt-level", devProfile.optLevel)));

  return rs::Ok(
      Profile(std::move(cxxflags), std::move(ldflags), lto, debug, optLevel));
}

static rs::Result<std::unordered_map<BuildProfile, Profile>>
parseProfiles(const toml::value& val) noexcept {
  std::unordered_map<BuildProfile, Profile> profiles;
  const BaseProfile baseProfile = rs_try(parseBaseProfile(val));
  Profile devProfile = rs_try(parseDevProfile(val, baseProfile));
  profiles.emplace(BuildProfile::Test,
                   rs_try(parseTestProfile(val, devProfile)));
  profiles.emplace(BuildProfile::Dev, std::move(devProfile));
  profiles.emplace(BuildProfile::Release,
                   rs_try(parseReleaseProfile(val, baseProfile)));
  return rs::Ok(profiles);
}

rs::Result<Cpplint> Cpplint::tryFromToml(const toml::value& val) noexcept {
  auto filters = toml::find_or_default<std::vector<std::string>>(
      val, "lint", "cpplint", "filters");
  return rs::Ok(Cpplint(std::move(filters)));
}

rs::Result<Lint> Lint::tryFromToml(const toml::value& val) noexcept {
  auto cpplint = rs_try(Cpplint::tryFromToml(val));
  return rs::Ok(Lint(std::move(cpplint)));
}

static rs::Result<void> validateDepName(const std::string_view name) noexcept {
  rs_ensure(!name.empty(), "dependency name must not be empty");
  rs_ensure(std::isalnum(name.front()),
            "dependency name must start with an alphanumeric character");
  rs_ensure(std::isalnum(name.back()) || name.back() == '+',
            "dependency name must end with an alphanumeric character or `+`");

  for (const char c : name) {
    if (!std::isalnum(c) && !ALLOWED_CHARS.contains(c)) {
      rs_bail("dependency name must be alphanumeric, `-`, `_`, `/`, "
              "`.`, or `+`");
    }
  }

  for (std::size_t i = 1; i < name.size(); ++i) {
    if (name[i] == '+') {
      // Allow consecutive `+` characters.
      continue;
    }

    if (!std::isalnum(name[i]) && name[i] == name[i - 1]) {
      rs_bail("dependency name must not contain consecutive non-alphanumeric "
              "characters");
    }
  }
  for (std::size_t i = 1; i < name.size() - 1; ++i) {
    if (name[i] != '.') {
      continue;
    }

    if (!std::isdigit(name[i - 1]) || !std::isdigit(name[i + 1])) {
      rs_bail("dependency name must contain `.` wrapped by digits");
    }
  }

  std::unordered_map<char, int> charsFreq;
  for (const char c : name) {
    ++charsFreq[c];
  }

  rs_ensure(charsFreq['/'] <= 1,
            "dependency name must not contain more than one `/`");
  rs_ensure(charsFreq['+'] == 0 || charsFreq['+'] == 2,
            "dependency name must contain zero or two `+`");
  if (charsFreq['+'] == 2) {
    if (name.find('+') + 1 != name.rfind('+')) {
      rs_bail("`+` in the dependency name must be consecutive");
    }
  }

  return rs::Ok();
}

static rs::Result<GitDependency> parseGitDep(const std::string& name,
                                             const toml::table& info) noexcept {
  rs_try(validateDepName(name));
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
  return rs::Ok(GitDependency(name, gitUrlStr, std::move(target)));
}

static rs::Result<PathDependency>
parsePathDep(const std::string& name, const toml::table& info) noexcept {
  rs_try(validateDepName(name));
  const auto& path = info.at("path");
  rs_ensure(path.is_string(), "path dependency must be a string");
  return rs::Ok(PathDependency(name, path.as_string()));
}

static rs::Result<SystemDependency>
parseSystemDep(const std::string& name, const toml::table& info) noexcept {
  rs_try(validateDepName(name));
  const auto& version = info.at("version");
  rs_ensure(version.is_string(), "system dependency version must be a string");

  const std::string versionReq = version.as_string();
  return rs::Ok(SystemDependency(name, rs_try(VersionReq::parse(versionReq))));
}

static rs::Result<std::vector<Dependency>>
parseDependencies(const toml::value& val, const char* key) noexcept {
  const auto tomlDeps = toml::try_find<toml::table>(val, key);
  if (tomlDeps.is_err()) {
    spdlog::debug("[{}] not found or not a table", key);
    return rs::Ok(std::vector<Dependency>{});
  }

  std::vector<Dependency> deps;
  for (const auto& dep : tomlDeps.unwrap()) {
    if (dep.second.is_table()) {
      const auto& info = dep.second.as_table();
      if (info.contains("git")) {
        deps.emplace_back(rs_try(parseGitDep(dep.first, info)));
        continue;
      } else if (info.contains("system") && info.at("system").as_boolean()) {
        deps.emplace_back(rs_try(parseSystemDep(dep.first, info)));
        continue;
      } else if (info.contains("path")) {
        deps.emplace_back(rs_try(parsePathDep(dep.first, info)));
        continue;
      }
    }

    rs_bail("Only Git dependency, path dependency, and system dependency are "
            "supported for now: {}",
            dep.first);
  }
  return rs::Ok(deps);
}

rs::Result<Manifest> Manifest::tryParse(fs::path path,
                                        const bool findParents) noexcept {
  if (findParents) {
    path = rs_try(findPath(path.parent_path()));
  }
  return tryFromToml(toml::parse(path), path);
}

rs::Result<Manifest> Manifest::tryFromToml(const toml::value& data,
                                           fs::path path) noexcept {
  auto package = rs_try(Package::tryFromToml(data));
  std::vector<Dependency> dependencies =
      rs_try(parseDependencies(data, "dependencies"));
  std::vector<Dependency> devDependencies =
      rs_try(parseDependencies(data, "dev-dependencies"));
  std::unordered_map<BuildProfile, Profile> profiles =
      rs_try(parseProfiles(data));
  auto lint = rs_try(Lint::tryFromToml(data));

  return rs::Ok(Manifest(std::move(path), std::move(package),
                         std::move(dependencies), std::move(devDependencies),
                         std::move(profiles), std::move(lint)));
}

rs::Result<fs::path> Manifest::findPath(fs::path candidateDir) noexcept {
  const fs::path origCandDir = candidateDir;
  while (true) {
    const fs::path configPath = candidateDir / FILE_NAME;
    spdlog::trace("Finding manifest: {}", configPath.string());
    if (fs::exists(configPath)) {
      return rs::Ok(configPath);
    }

    const fs::path parentPath = candidateDir.parent_path();
    if (candidateDir.has_parent_path()
        && parentPath != candidateDir.root_directory()) {
      candidateDir = parentPath;
    } else {
      break;
    }
  }

  rs_bail("{} not find in `{}` and its parents", FILE_NAME,
          origCandDir.string());
}

rs::Result<std::vector<CompilerOpts>>
Manifest::installDeps(const bool includeDevDeps,
                      const BuildProfile& buildProfile,
                      const bool suppressDepDiag) const {
  std::unordered_map<std::string, DepKey> seenDeps;
  std::unordered_set<fs::path> visited;
  std::vector<CompilerOpts> installed;
  rs_try(installDependencies(*this, buildProfile, includeDevDeps,
                             suppressDepDiag, seenDeps, visited, installed));
  return rs::Ok(installed);
}

// Returns an error message if the package name is invalid.
rs::Result<void> validatePackageName(const std::string_view name) noexcept {
  rs_ensure(!name.empty(), "package name must not be empty");
  rs_ensure(name.size() > 1, "package name must be more than one character");

  for (const char c : name) {
    if (!std::islower(c) && !std::isdigit(c) && c != '-' && c != '_') {
      rs_bail(
          "package name must only contain lowercase letters, numbers, dashes, "
          "and underscores");
    }
  }

  rs_ensure(std::isalpha(name[0]), "package name must start with a letter");
  rs_ensure(std::isalnum(name[name.size() - 1]),
            "package name must end with a letter or digit");

  static const std::unordered_set<std::string_view> keywords = {
#include "Keywords.def"
  };
  rs_ensure(!keywords.contains(name), "package name must not be a C++ keyword");

  return rs::Ok();
}

} // namespace cabin

#ifdef CABIN_TEST

#  include <climits>
#  include <fmt/ranges.h>
#  include <rs/tests.hpp>
#  include <toml11/fwd/literal_fwd.hpp>

// NOLINTBEGIN
using namespace cabin;
using namespace toml::literals::toml_literals;
// NOLINTEND

inline static void assertEditionEq(
    const Edition::Year left, const Edition::Year right,
    const std::source_location& loc = std::source_location::current()) {
  rs::assertEq(static_cast<uint16_t>(left), static_cast<uint16_t>(right), "",
               loc);
}
inline static void assertEditionEq(
    const Edition& left, const Edition::Year right,
    const std::source_location& loc = std::source_location::current()) {
  assertEditionEq(left.edition, right, loc);
}

static void testEditionTryFromString() { // Valid editions
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
  rs::assertEq(Edition::tryFromString("").unwrap_err()->what(),
               "invalid edition");
  rs::assertEq(Edition::tryFromString("abc").unwrap_err()->what(),
               "invalid edition");
  rs::assertEq(Edition::tryFromString("99").unwrap_err()->what(),
               "invalid edition");
  rs::assertEq(Edition::tryFromString("21").unwrap_err()->what(),
               "invalid edition");

  rs::pass();
}

static void testEditionComparison() {
  rs::assertTrue(Edition::tryFromString("98").unwrap()
                 <= Edition::tryFromString("03").unwrap());
  rs::assertTrue(Edition::tryFromString("03").unwrap()
                 <= Edition::tryFromString("11").unwrap());
  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 <= Edition::tryFromString("14").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 <= Edition::tryFromString("17").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 <= Edition::tryFromString("20").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 <= Edition::tryFromString("23").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 <= Edition::tryFromString("2c").unwrap());

  rs::assertTrue(Edition::tryFromString("98").unwrap()
                 < Edition::tryFromString("03").unwrap());
  rs::assertTrue(Edition::tryFromString("03").unwrap()
                 < Edition::tryFromString("11").unwrap());
  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 < Edition::tryFromString("14").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 < Edition::tryFromString("17").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 < Edition::tryFromString("20").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 < Edition::tryFromString("23").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 < Edition::tryFromString("2c").unwrap());

  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 == Edition::tryFromString("0x").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 == Edition::tryFromString("1y").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 == Edition::tryFromString("1z").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 == Edition::tryFromString("2a").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 == Edition::tryFromString("2b").unwrap());

  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 != Edition::tryFromString("03").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 != Edition::tryFromString("11").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 != Edition::tryFromString("14").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 != Edition::tryFromString("17").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 != Edition::tryFromString("20").unwrap());

  rs::assertTrue(Edition::tryFromString("2c").unwrap()
                 > Edition::tryFromString("23").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 > Edition::tryFromString("20").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 > Edition::tryFromString("17").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 > Edition::tryFromString("14").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 > Edition::tryFromString("11").unwrap());
  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 > Edition::tryFromString("03").unwrap());
  rs::assertTrue(Edition::tryFromString("03").unwrap()
                 > Edition::tryFromString("98").unwrap());

  rs::assertTrue(Edition::tryFromString("2c").unwrap()
                 >= Edition::tryFromString("23").unwrap());
  rs::assertTrue(Edition::tryFromString("23").unwrap()
                 >= Edition::tryFromString("20").unwrap());
  rs::assertTrue(Edition::tryFromString("20").unwrap()
                 >= Edition::tryFromString("17").unwrap());
  rs::assertTrue(Edition::tryFromString("17").unwrap()
                 >= Edition::tryFromString("14").unwrap());
  rs::assertTrue(Edition::tryFromString("14").unwrap()
                 >= Edition::tryFromString("11").unwrap());
  rs::assertTrue(Edition::tryFromString("11").unwrap()
                 >= Edition::tryFromString("03").unwrap());
  rs::assertTrue(Edition::tryFromString("03").unwrap()
                 >= Edition::tryFromString("98").unwrap());

  rs::assertTrue(Edition::tryFromString("17").unwrap() <= Edition::Cpp17);
  rs::assertTrue(Edition::tryFromString("17").unwrap() < Edition::Cpp20);
  rs::assertTrue(Edition::tryFromString("20").unwrap() == Edition::Cpp20);
  rs::assertTrue(Edition::tryFromString("20").unwrap() != Edition::Cpp23);
  rs::assertTrue(Edition::tryFromString("23").unwrap() > Edition::Cpp20);
  rs::assertTrue(Edition::tryFromString("20").unwrap() >= Edition::Cpp20);

  rs::pass();
}

static void testPackageTryFromToml() {
  // Valid package
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
      version = "1.2.3"
    )"_toml;

    auto pkg = Package::tryFromToml(val).unwrap();
    rs::assertEq(pkg.name, "test-pkg");
    rs::assertEq(pkg.edition.str, "20");
    rs::assertEq(pkg.version.toString(), "1.2.3");
  }

  // Missing fields
  {
    const toml::value val = R"(
      [package]
    )"_toml;

    rs::assertEq(Package::tryFromToml(val).unwrap_err()->what(),
                 R"(toml::value::at: key "name" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table)");
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
    )"_toml;

    rs::assertEq(Package::tryFromToml(val).unwrap_err()->what(),
                 R"(toml::value::at: key "edition" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table)");
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
    )"_toml;

    rs::assertEq(Package::tryFromToml(val).unwrap_err()->what(),
                 R"(toml::value::at: key "version" not found
 --> TOML literal encoded in a C++ code
   |
 2 |       [package]
   |       ^^^^^^^^^-- in this table)");
  }

  // Invalid fields
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "invalid"
      version = "1.2.3"
    )"_toml;

    rs::assertEq(Package::tryFromToml(val).unwrap_err()->what(),
                 "invalid edition");
  }
  {
    const toml::value val = R"(
      [package]
      name = "test-pkg"
      edition = "20"
      version = "invalid"
    )"_toml;

    rs::assertEq(Package::tryFromToml(val).unwrap_err()->what(),
                 R"(invalid semver:
invalid
^^^^^^^ expected number)");
  }

  rs::pass();
}

static void testParseProfiles() {
  const Profile devProfileDefault(
      /*cxxflags=*/{}, /*ldflags=*/{}, /*lto=*/false, /*debug=*/true,
      /*optLevel=*/0);
  const Profile relProfileDefault(
      /*cxxflags=*/{}, /*ldflags=*/{}, /*lto=*/false, /*debug=*/false,
      /*optLevel=*/3);

  {
    const toml::value empty = ""_toml;

    const auto profiles = parseProfiles(empty).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Release), relProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Test), devProfileDefault);
  }
  {
    const toml::value profOnly = "[profile]"_toml;

    const auto profiles = parseProfiles(profOnly).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Release), relProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Test), devProfileDefault);
  }
  {
    const toml::value baseOnly = R"(
      [profile]
      cxxflags = ["-fno-rtti"]
      ldflags = ["-lm"]
      lto = true
      debug = true
      opt-level = 2
    )"_toml;

    const Profile expected(
        /*cxxflags=*/{ "-fno-rtti" }, /*ldflags=*/{ "-lm" }, /*lto=*/true,
        /*debug=*/true,
        /*optLevel=*/2);

    const auto profiles = parseProfiles(baseOnly).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), expected);
    rs::assertEq(profiles.at(BuildProfile::Release), expected);
    rs::assertEq(profiles.at(BuildProfile::Test), expected);
  }
  {
    const toml::value overwrite = R"(
      [profile]
      cxxflags = ["-fno-rtti"]

      [profile.dev]
      cxxflags = []

      [profile.release]
      cxxflags = []
    )"_toml;

    const auto profiles = parseProfiles(overwrite).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Release), relProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Test), devProfileDefault);
  }
  {
    const toml::value overwrite = R"(
      [profile]
      opt-level = 2

      [profile.dev]
      opt-level = 1

      [profile.test]
      opt-level = 3
    )"_toml;

    const Profile devExpected(
        /*cxxflags=*/{}, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/1);
    const Profile relExpected(
        /*cxxflags=*/{}, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/false,
        /*optLevel=*/2 // here, the default is 3
    );
    const Profile testExpected(
        /*cxxflags=*/{}, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/3);

    const auto profiles = parseProfiles(overwrite).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devExpected);
    rs::assertEq(profiles.at(BuildProfile::Release), relExpected);
    rs::assertEq(profiles.at(BuildProfile::Test), testExpected);
  }
  {
    const toml::value append = R"(
      [profile.dev]
      cxxflags = ["-A"]

      [profile.test]
      cxxflags = ["-B"]
    )"_toml;

    const Profile devExpected(
        /*cxxflags=*/{ "-A" }, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/0);
    const Profile testExpected(
        /*cxxflags=*/{ "-A", "-B" }, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/0);

    const auto profiles = parseProfiles(append).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devExpected);
    rs::assertEq(profiles.at(BuildProfile::Release), relProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Test), testExpected);
  }
  {
    const toml::value overwrite = R"(
      [profile.dev]
      cxxflags = ["-A"]

      [profile.test]
      inherit-mode = "overwrite"
      cxxflags = ["-B"]
    )"_toml;

    const Profile devExpected(
        /*cxxflags=*/{ "-A" }, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/0);
    const Profile testExpected(
        /*cxxflags=*/{ "-B" }, /*ldflags=*/{}, /*lto=*/false,
        /*debug=*/true,
        /*optLevel=*/0);

    const auto profiles = parseProfiles(overwrite).unwrap();
    rs::assertEq(profiles.size(), 3UL);
    rs::assertEq(profiles.at(BuildProfile::Dev), devExpected);
    rs::assertEq(profiles.at(BuildProfile::Release), relProfileDefault);
    rs::assertEq(profiles.at(BuildProfile::Test), testExpected);
  }
  {
    const toml::value incorrect = R"(
      [profile.test]
      inherit-mode = "UNKNOWN"
    )"_toml;

    rs::assertEq(parseProfiles(incorrect).unwrap_err()->what(),
                 "invalid inherit-mode: `UNKNOWN`");
  }
}

static void testLintTryFromToml() {
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
    rs::assertEq(
        fmt::format("{}", fmt::join(lint.cpplint.filters, ",")),
        fmt::format(
            "{}", fmt::join(std::vector<std::string>{ "+filter1", "-filter2" },
                            ",")));
  }

  // Empty lint config
  {
    const toml::value val{};
    auto lint = Lint::tryFromToml(val).unwrap();
    rs::assertTrue(lint.cpplint.filters.empty());
  }

  rs::pass();
}

static void testValidateDepName() {
  rs::assertEq(validateDepName("").unwrap_err()->what(),
               "dependency name must not be empty");
  rs::assertEq(validateDepName("-").unwrap_err()->what(),
               "dependency name must start with an alphanumeric character");
  rs::assertEq(
      validateDepName("1-").unwrap_err()->what(),
      "dependency name must end with an alphanumeric character or `+`");

  for (char c = 0; c < CHAR_MAX; ++c) {
    if (std::isalnum(c) || ALLOWED_CHARS.contains(c)) {
      continue;
    }
    rs::assertEq(
        validateDepName("1" + std::string(1, c) + "1").unwrap_err()->what(),
        "dependency name must be alphanumeric, `-`, `_`, `/`, `.`, or `+`");
  }

  rs::assertEq(validateDepName("1--1").unwrap_err()->what(),
               "dependency name must not contain consecutive non-alphanumeric "
               "characters");
  rs::assertTrue(validateDepName("1-1-1").is_ok());

  rs::assertTrue(validateDepName("1.1").is_ok());
  rs::assertTrue(validateDepName("1.1.1").is_ok());
  rs::assertEq(validateDepName("a.a").unwrap_err()->what(),
               "dependency name must contain `.` wrapped by digits");

  rs::assertTrue(validateDepName("a/b").is_ok());
  rs::assertEq(validateDepName("a/b/c").unwrap_err()->what(),
               "dependency name must not contain more than one `/`");

  rs::assertEq(validateDepName("a+").unwrap_err()->what(),
               "dependency name must contain zero or two `+`");
  rs::assertEq(validateDepName("a+++").unwrap_err()->what(),
               "dependency name must contain zero or two `+`");

  rs::assertEq(validateDepName("a+b+c").unwrap_err()->what(),
               "`+` in the dependency name must be consecutive");

  // issue #921
  rs::assertTrue(validateDepName("gtkmm-4.0").is_ok());
  rs::assertTrue(validateDepName("ncurses++").is_ok());

  rs::pass();
}

static void testValidateFlag() {
  rs::assertTrue(
      validateFlag("cxxflags", "-fsanitize=address,undefined").is_ok());

  // issue #1183
  rs::assertTrue(validateFlag("ldflags", "-framework Metal").is_ok());
  rs::assertEq(
      validateFlag("ldflags", "-framework  Metal").unwrap_err()->what(),
      "ldflags must only contain [' '] once");
  rs::assertEq(
      validateFlag("ldflags", "-framework Metal && bash").unwrap_err()->what(),
      "ldflags must only contain [' '] once");

  rs::pass();
}

int main() {
  cabin::setColorMode("never");

  testEditionTryFromString();
  testEditionComparison();
  testPackageTryFromToml();
  testParseProfiles();
  testLintTryFromToml();
  testValidateDepName();
  testValidateFlag();
}

#endif
