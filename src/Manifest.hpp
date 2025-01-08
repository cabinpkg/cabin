#pragma once

#include "Rustify/Aliases.hpp"
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

  auto operator<=>(const Edition& otherEdition) const {
    return edition <=> otherEdition.edition;
  }
  auto operator<=>(const Year& otherYear) const {
    return edition <=> otherYear;
  }

private:
  Edition(Year year, std::string str) noexcept
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
  const bool lto;
  const bool debug;
  const std::size_t optLevel;

  Profile(
      std::vector<std::string> cxxflags, bool lto, bool debug,
      std::size_t optLevel
  ) noexcept
      : cxxflags(std::move(cxxflags)), lto(lto), debug(debug),
        optLevel(optLevel) {}
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

  DepMetadata install() const;
};

struct PathDependency {
  const std::string name;
  const std::string path;

  DepMetadata install() const;
};

struct SystemDependency {
  const std::string name;
  const VersionReq versionReq;

  DepMetadata install() const;
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

  std::vector<DepMetadata> installDeps(bool includeDevDeps) const;

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
std::optional<std::string> validatePackageName(std::string_view name) noexcept;

// } // namespace cabin
