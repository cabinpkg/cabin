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
#include <optional>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <variant>
#include <vector>

Edition::Edition(const std::string& str) : str(str) {
  if (str == "98") {
    edition = Cpp98;
    return;
  } else if (str == "03") {
    edition = Cpp03;
    return;
  } else if (str == "0x" || str == "11") {
    edition = Cpp11;
    return;
  } else if (str == "1y" || str == "14") {
    edition = Cpp14;
    return;
  } else if (str == "1z" || str == "17") {
    edition = Cpp17;
    return;
  } else if (str == "2a" || str == "20") {
    edition = Cpp20;
    return;
  } else if (str == "2b" || str == "23") {
    edition = Cpp23;
    return;
  } else if (str == "2c") {
    edition = Cpp26;
    return;
  }
  throw CabinError("invalid edition: ", str);
}

std::string
Edition::getString() const noexcept {
  return str;
}

struct Package {
  std::string name;
  Edition edition;
  Version version;
};

// NOLINTBEGIN(readability-identifier-naming)
namespace toml {
template <>
struct from<Edition> {
  static Edition from_toml(const value& val) {
    const std::string& editionStr = toml::get<std::string>(val);
    return Edition(editionStr);
  }
};
template <>
struct into<Edition> {
  static toml::value into_toml(const Edition& edition) {
    return edition.getString();
  }
};

template <>
struct from<Version> {
  static Version from_toml(const value& val) {
    const std::string& versionStr = toml::get<std::string>(val);
    return Version::parse(versionStr);
  }
};
template <>
struct into<Version> {
  static std::string into_toml(const Version& ver) {
    return ver.toString();
  }
};
}  // namespace toml
// NOLINTEND(readability-identifier-naming)

TOML11_DEFINE_CONVERSION_NON_INTRUSIVE(Package, name, edition, version);

static fs::path
findManifest() {
  fs::path candidate = fs::current_path();
  while (true) {
    const fs::path configPath = candidate / "cabin.toml";
    logger::trace("Finding manifest: {}", configPath.string());
    if (fs::exists(configPath)) {
      return configPath;
    }

    const fs::path parentPath = candidate.parent_path();
    if (candidate.has_parent_path()
        && parentPath != candidate.root_directory()) {
      candidate = parentPath;
    } else {
      break;
    }
  }

  throw CabinError("could not find `cabin.toml` here and in its parents");
}

struct GitDependency {
  std::string name;
  std::string url;
  std::optional<std::string> target;

  DepMetadata install() const;
};

struct PathDependency {
  std::string name;
  std::string path;

  DepMetadata install() const;
};

struct SystemDependency {
  std::string name;
  VersionReq versionReq;

  DepMetadata install() const;
};

using Dependency =
    std::variant<GitDependency, PathDependency, SystemDependency>;

void
Profile::merge(const Profile& other) {
  cxxflags.insert(other.cxxflags.begin(), other.cxxflags.end());
  if (!lto) {  // false is the default value
    lto = other.lto;
  }
  if (other.debug.has_value() && !debug.has_value()) {
    debug = other.debug;
  }
  if (other.optLevel.has_value() && !optLevel.has_value()) {
    optLevel = other.optLevel;
  }
}

struct Manifest {
  // Manifest is a singleton
  Manifest(const Manifest&) = delete;
  Manifest(Manifest&&) noexcept = delete;
  Manifest& operator=(const Manifest&) = delete;
  Manifest& operator=(Manifest&&) noexcept = delete;
  ~Manifest() noexcept = default;

  static Manifest& instance() {
    static Manifest instance;
    instance.load();
    return instance;
  }

  std::optional<fs::path> manifestPath = std::nullopt;

  std::optional<toml::value> data = std::nullopt;

  std::optional<Package> package = std::nullopt;
  std::optional<std::vector<Dependency>> dependencies = std::nullopt;
  std::optional<std::vector<Dependency>> devDependencies = std::nullopt;

  std::optional<Profile> profile = std::nullopt;
  std::optional<Profile> devProfile = std::nullopt;
  std::optional<Profile> releaseProfile = std::nullopt;

  std::optional<std::vector<std::string>> cpplintFilters = std::nullopt;

private:
  Manifest() noexcept = default;

  void load() {
    if (data.has_value()) {
      return;
    }

    if (shouldColor()) {
      toml::color::enable();
    } else {
      toml::color::disable();
    }

    manifestPath = findManifest();
    data = toml::parse(manifestPath.value());
  }
};

const fs::path&
getManifestPath() {
  return Manifest::instance().manifestPath.value();
}

fs::path
getProjectBasePath() {
  return fs::absolute(getManifestPath().parent_path());
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

static Package&
parsePackage() {
  Manifest& manifest = Manifest::instance();
  if (manifest.package.has_value()) {
    return manifest.package.value();
  }

  const toml::value& data = manifest.data.value();
  const auto package = toml::find<Package>(data, "package");

  if (const auto err = validatePackageName(package.name)) {
    throw CabinError(
        toml::format_error("invalid name", data.at("package.name"), err.value())
    );
  }

  manifest.package = package;
  return manifest.package.value();
}

const std::string&
getPackageName() {
  return parsePackage().name;
}
const Edition&
getPackageEdition() {
  return parsePackage().edition;
}
const Version&
getPackageVersion() {
  return parsePackage().version;
}

static void
validateCxxflag(const std::string_view cxxflag) {
  // cxxflag must start with `-`
  if (cxxflag.empty() || cxxflag[0] != '-') {
    throw CabinError("cxxflag must start with `-`");
  }

  // cxxflag only contains alphanumeric characters, `-`, `_`, `=`, `+`, `:`,
  // or `.`.
  for (const char c : cxxflag) {
    if (!std::isalnum(c) && c != '-' && c != '_' && c != '=' && c != '+'
        && c != ':' && c != '.') {
      throw CabinError(
          "cxxflag must only contain alphanumeric characters, `-`, `_`, `=`, "
          "`+`, `:`, or `.`"
      );
    }
  }
}

static Profile
parseProfile(const toml::table& table) {
  Profile profile;
  if (table.contains("cxxflags") && table.at("cxxflags").is_array()) {
    const auto& cxxflags = table.at("cxxflags").as_array();
    for (const auto& flag : cxxflags) {
      if (!flag.is_string()) {
        throw CabinError("[profile.cxxflags] must be an array of strings");
      }
      const std::string& flagStr = flag.as_string();
      validateCxxflag(flagStr);
      profile.cxxflags.insert(flagStr);
    }
  }
  if (table.contains("lto") && table.at("lto").is_boolean()) {
    profile.lto = table.at("lto").as_boolean();
  }
  if (table.contains("debug") && table.at("debug").is_boolean()) {
    profile.debug = table.at("debug").as_boolean();
  }
  if (table.contains("opt_level") && table.at("opt_level").is_integer()) {
    const int32_t optLevel =
        static_cast<int32_t>(table.at("opt_level").as_integer());
    if (optLevel < 0 || optLevel > 3) {
      throw CabinError("opt_level must be between 0 and 3");
    }
    profile.optLevel = optLevel;
  }
  return profile;
}

static Profile
getProfile(std::optional<std::string> profileName) {
  Manifest& manifest = Manifest::instance();
  if (!manifest.data.value().contains("profile")) {
    return {};
  }
  if (!manifest.data.value().at("profile").is_table()) {
    throw CabinError("[profile] must be a table");
  }
  const auto& table = toml::find<toml::table>(manifest.data.value(), "profile");

  if (profileName.has_value()) {
    if (!table.contains(profileName.value())) {
      return {};
    }
    if (!table.at(profileName.value()).is_table()) {
      throw CabinError("[profile.", profileName.value(), "] must be a table");
    }
    const auto& profileTable = toml::find<toml::table>(
        manifest.data.value(), "profile", profileName.value()
    );
    return parseProfile(profileTable);
  } else {
    return parseProfile(table);
  }
}

static const Profile&
getBaseProfile() {
  Manifest& manifest = Manifest::instance();
  if (manifest.profile.has_value()) {
    return manifest.profile.value();
  }

  const Profile baseProfile = getProfile(std::nullopt);
  manifest.profile = baseProfile;
  return manifest.profile.value();
}

const Profile&
getDevProfile() {
  Manifest& manifest = Manifest::instance();
  if (manifest.devProfile.has_value()) {
    return manifest.devProfile.value();
  }

  Profile devProfile = getProfile("dev");
  devProfile.merge(getBaseProfile());
  if (!devProfile.debug.has_value()) {
    devProfile.debug = true;
  }
  if (!devProfile.optLevel.has_value()) {
    devProfile.optLevel = 0;
  }
  manifest.devProfile = devProfile;
  return manifest.devProfile.value();
}

const Profile&
getReleaseProfile() {
  Manifest& manifest = Manifest::instance();
  if (manifest.releaseProfile.has_value()) {
    return manifest.releaseProfile.value();
  }

  Profile releaseProfile = getProfile("release");
  releaseProfile.merge(getBaseProfile());
  if (!releaseProfile.debug.has_value()) {
    releaseProfile.debug = false;
  }
  if (!releaseProfile.optLevel.has_value()) {
    releaseProfile.optLevel = 3;
  }
  manifest.releaseProfile = releaseProfile;
  return manifest.releaseProfile.value();
}

const std::vector<std::string>&
getLintCpplintFilters() {
  Manifest& manifest = Manifest::instance();
  if (manifest.cpplintFilters.has_value()) {
    return manifest.cpplintFilters.value();
  }

  const auto& table = toml::get<toml::table>(*manifest.data);
  std::vector<std::string> filters;
  if (!table.contains("lint")) {
    filters = {};
  } else {
    filters = toml::find_or<std::vector<std::string>>(
        *manifest.data, "lint", "cpplint", "filters", std::vector<std::string>{}
    );
  }
  manifest.cpplintFilters = filters;
  return manifest.cpplintFilters.value();
}

static fs::path
getXdgCacheHome() {
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

static std::optional<std::vector<Dependency>>
parseDependencies(const char* key) {
  Manifest& manifest = Manifest::instance();
  const auto& table = toml::get<toml::table>(manifest.data.value());
  if (!table.contains(key)) {
    logger::debug("[dependencies] not found");
    return std::nullopt;
  }
  const auto tomlDeps = toml::find<toml::table>(manifest.data.value(), key);

  std::vector<Dependency> deps;
  for (const auto& dep : tomlDeps) {
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

    throw CabinError(
        "Only Git dependency, path dependency, and system dependency are "
        "supported for now: ",
        dep.first
    );
  }
  return deps;
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

std::vector<DepMetadata>
installDependencies(const bool includeDevDeps) {
  Manifest& manifest = Manifest::instance();
  if (!manifest.dependencies.has_value()) {
    manifest.dependencies = parseDependencies("dependencies");
  }
  if (includeDevDeps && !manifest.devDependencies.has_value()) {
    manifest.devDependencies = parseDependencies("dev-dependencies");
  }

  std::vector<DepMetadata> installed;
  if (manifest.dependencies.has_value()) {
    for (const auto& dep : manifest.dependencies.value()) {
      std::visit(
          [&installed](auto&& arg) { installed.emplace_back(arg.install()); },
          dep
      );
    }
  }
  if (includeDevDeps && manifest.devDependencies.has_value()) {
    for (const auto& dep : manifest.devDependencies.value()) {
      std::visit(
          [&installed](auto&& arg) { installed.emplace_back(arg.install()); },
          dep
      );
    }
  }
  return installed;
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
