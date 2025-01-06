#pragma once

#include "Rustify/Aliases.hpp"
#include "Semver.hpp"

#include <compare>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_set>
#include <vector>

struct DepMetadata {
  std::string includes;  // -Isomething
  std::string libs;      // -Lsomething -lsomething
};

struct Profile {
  std::unordered_set<std::string> cxxflags;
  bool lto = false;
  std::optional<bool> debug = std::nullopt;
  std::optional<std::size_t> optLevel = std::nullopt;

  // Merges this profile with another profile. If a field in this profile is
  // set, it will not be overwritten by the other profile. Only default values
  // will be overwritten.
  void merge(const Profile& other);
};

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

private:
  Year edition = Year::Cpp20;
  std::string str = "20";

public:
  Edition() = default;
  explicit Edition(const std::string& str);

  std::string getString() const noexcept;

  auto operator<=>(const Edition& otherEdition) const {
    return edition <=> otherEdition.edition;
  }
  auto operator<=>(const Year& otherYear) const {
    return edition <=> otherYear;
  }
};

const fs::path& getManifestPath();
fs::path getProjectBasePath();
std::optional<std::string> validatePackageName(std::string_view name) noexcept;
const std::string& getPackageName();
const Edition& getPackageEdition();
const Version& getPackageVersion();
const Profile& getDevProfile();
const Profile& getReleaseProfile();
const std::vector<std::string>& getLintCpplintFilters();
std::vector<DepMetadata> installDependencies(bool includeDevDeps);
