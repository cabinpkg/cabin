#pragma once

#include "Rustify/Result.hpp"
#include "Semver.hpp"
#include "VersionReq.hpp"

#include <compare>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>

namespace cabin {

namespace fs = std::filesystem;
using std::string_view_literals::operator""sv;

struct Edition {
  enum class Year : uint16_t {
    Cpp98 = 1998,
    Cpp03 = 2003,
    Cpp11 = 2011,
    Cpp14 = 2014,
    Cpp17 = 2017,
    Cpp20 = 2020,
    Cpp23 = 2023,
    Cpp26 = 2026,
  };
  using enum Year;

  const Year edition;
  const std::string str;

  static Result<Edition> tryFromString(std::string str) noexcept;

  bool operator==(const Edition& otherEdition) const {
    return edition == otherEdition.edition;
  }
  auto operator==(const Year& otherYear) const {
    return edition == otherYear;
  }

  auto operator<=>(const Edition& otherEdition) const {
    return edition <=> otherEdition.edition;
  }
  auto operator<=>(const Year& otherYear) const {
    return edition <=> otherYear;
  }

private:
  Edition(const Year year, std::string str) noexcept
      : edition(year), str(std::move(str)) {}
};

struct Package {
  const std::string name;
  const Edition edition;
  const Version version;

  static Result<Package> tryFromToml(const toml::value& val) noexcept;

private:
  Package(std::string name, Edition edition, Version version) noexcept
      : name(std::move(name)), edition(std::move(edition)),
        version(std::move(version)) {}
};

struct Profile {
  const std::vector<std::string> cxxflags;
  const std::vector<std::string> ldflags;
  const bool lto;
  const bool debug;
  const bool compDb;
  const std::uint8_t optLevel;

  Profile(
      std::vector<std::string> cxxflags, std::vector<std::string> ldflags,
      const bool lto, const bool debug, const bool compDb,
      const std::uint8_t optLevel
  ) noexcept
      : cxxflags(std::move(cxxflags)), ldflags(std::move(ldflags)), lto(lto),
        debug(debug), compDb(compDb), optLevel(optLevel) {}

  bool operator==(const Profile& other) const {
    return cxxflags == other.cxxflags && ldflags == other.ldflags
           && lto == other.lto && debug == other.debug && compDb == other.compDb
           && optLevel == other.optLevel;
  }

  friend std::ostream& operator<<(std::ostream& os, const Profile& p) {
    const std::string str = fmt::format(
        R"(Profile {{
  cxxflags: {},
  ldflags: {},
  lto: {},
  debug: {},
  compDb: {},
  optLevel: {},
}})",
        p.cxxflags, p.ldflags, p.lto, p.debug, p.compDb, p.optLevel
    );
    os << str;
    return os;
  }
};

struct Cpplint {
  const std::vector<std::string> filters;

  static Result<Cpplint> tryFromToml(const toml::value& val) noexcept;

private:
  explicit Cpplint(std::vector<std::string> filters) noexcept
      : filters(std::move(filters)) {}
};

struct Lint {
  const Cpplint cpplint;

  static Result<Lint> tryFromToml(const toml::value& val) noexcept;

private:
  explicit Lint(Cpplint cpplint) noexcept : cpplint(std::move(cpplint)) {}
};

struct DepMetadata {
  const std::string includes;  // -Isomething
  const std::string libs;      // -Lsomething -lsomething
};

struct GitDependency {
  const std::string name;
  const std::string url;
  const std::optional<std::string> target;

  Result<DepMetadata> install() const;

  GitDependency(
      std::string name, std::string url, std::optional<std::string> target
  )
      : name(std::move(name)), url(std::move(url)), target(std::move(target)) {}
};

struct PathDependency {
  const std::string name;
  const std::string path;

  Result<DepMetadata> install() const;

  PathDependency(std::string name, std::string path)
      : name(std::move(name)), path(std::move(path)) {}
};

struct SystemDependency {
  const std::string name;
  const VersionReq versionReq;

  Result<DepMetadata> install() const;

  SystemDependency(std::string name, VersionReq versionReq)
      : name(std::move(name)), versionReq(std::move(versionReq)) {};
};

using Dependency =
    std::variant<GitDependency, PathDependency, SystemDependency>;

class Manifest {
public:
  static constexpr const char* NAME = "cabin.toml";

  const fs::path path;
  const Package package;
  const std::vector<Dependency> dependencies;
  const std::vector<Dependency> devDependencies;
  const std::unordered_map<std::string, Profile> profiles;
  const Lint lint;

  static Result<Manifest> tryParse(
      fs::path path = fs::current_path() / NAME, bool findParents = true
  ) noexcept;
  static Result<Manifest>
  tryFromToml(const toml::value& data, fs::path path = "unknown") noexcept;

  Result<std::vector<DepMetadata>> installDeps(bool includeDevDeps) const;

private:
  Manifest(
      fs::path path, Package package, std::vector<Dependency> dependencies,
      std::vector<Dependency> devDependencies,
      std::unordered_map<std::string, Profile> profiles, Lint lint
  ) noexcept
      : path(std::move(path)), package(std::move(package)),
        dependencies(std::move(dependencies)),
        devDependencies(std::move(devDependencies)),
        profiles(std::move(profiles)), lint(std::move(lint)) {}
};

Result<fs::path>
findManifest(fs::path candidateDir = fs::current_path()) noexcept;
Result<void> validatePackageName(std::string_view name) noexcept;

}  // namespace cabin
