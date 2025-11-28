#pragma once

#include "Builder/BuildProfile.hpp"
#include "Builder/Compiler.hpp"
#include "Dependency.hpp"
#include "Semver.hpp"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fmt/ostream.h>
#include <fmt/ranges.h>
#include <rs/result.hpp>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <utility>
#include <vector>

namespace cabin {

namespace fs = std::filesystem;

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
  auto operator==(const Year& otherYear) const { return edition == otherYear; }

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
  const std::uint8_t optLevel;

  Profile(std::vector<std::string> cxxflags, std::vector<std::string> ldflags,
          const bool lto, const bool debug,
          const std::uint8_t optLevel) noexcept
      : cxxflags(std::move(cxxflags)), ldflags(std::move(ldflags)), lto(lto),
        debug(debug), optLevel(optLevel) {}

  bool operator==(const Profile& other) const {
    return cxxflags == other.cxxflags && ldflags == other.ldflags
           && lto == other.lto && debug == other.debug
           && optLevel == other.optLevel;
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

class Manifest {
public:
  static constexpr const char* FILE_NAME = "cabin.toml";

  const fs::path path;
  const Package package;
  const std::vector<Dependency> dependencies;
  const std::vector<Dependency> devDependencies;
  const std::unordered_map<BuildProfile, Profile> profiles;
  const Lint lint;

  static Result<Manifest> tryParse(fs::path path = fs::current_path()
                                                   / FILE_NAME,
                                   bool findParents = true) noexcept;
  static Result<Manifest> tryFromToml(const toml::value& data,
                                      fs::path path = "unknown") noexcept;

  static Result<fs::path>
  findPath(fs::path candidateDir = fs::current_path()) noexcept;

  Result<std::vector<CompilerOpts>> installDeps(bool includeDevDeps) const;

private:
  Manifest(fs::path path, Package package, std::vector<Dependency> dependencies,
           std::vector<Dependency> devDependencies,
           std::unordered_map<BuildProfile, Profile> profiles,
           Lint lint) noexcept
      : path(std::move(path)), package(std::move(package)),
        dependencies(std::move(dependencies)),
        devDependencies(std::move(devDependencies)),
        profiles(std::move(profiles)), lint(std::move(lint)) {}
};

Result<void> validatePackageName(std::string_view name) noexcept;

} // namespace cabin

template <>
struct fmt::formatter<cabin::Profile> {
private:
  bool debugFmt = false;

public:
  constexpr auto parse(fmt::format_parse_context& ctx) {
    const char* itr = ctx.begin();
    const char* end = ctx.end();
    if (itr == end) {
      return itr;
    }

    if (*itr == '?') {
      debugFmt = true;
      ++itr; // NOLINT
    }
    return itr;
  }

  template <typename FormatContext>
  auto format(const cabin::Profile& p, FormatContext& ctx) const {
    if (!debugFmt) {
      std::vector<std::string_view> strs;
      if (p.optLevel == 0) {
        strs.emplace_back("unoptimized");
      } else {
        strs.emplace_back("optimized");
      }
      if (p.debug) {
        strs.emplace_back("debuginfo");
      }
      return fmt::format_to(ctx.out(), "{}", fmt::join(strs, " + "));
    } else {
      return fmt::format_to(ctx.out(),
                            R"(Profile {{
  cxxflags: {},
  ldflags: {},
  lto: {},
  debug: {},
  optLevel: {},
}})",
                            p.cxxflags, p.ldflags, p.lto, p.debug, p.optLevel);
    }
  }
};
