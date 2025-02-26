#pragma once

#include "Rustify/Result.hpp"
#include "VersionReq.hpp"

#include <filesystem>
#include <fmt/format.h>
#include <fmt/std.h>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cabin {

namespace fs = std::filesystem;

struct Macro {
  std::string name;
  std::string value;

  Macro(std::string name, std::string value) noexcept
      : name(std::move(name)), value(std::move(value)) {}
};

struct IncludeDir {
  fs::path dir;
  bool isSystem = true;

  explicit IncludeDir(fs::path dir) noexcept : dir(std::move(dir)) {}
  IncludeDir(fs::path dir, bool isSystem) noexcept
      : dir(std::move(dir)), isSystem(isSystem) {}
};

struct CFlags {
  std::vector<Macro> macros;            // -D<name>=<val>
  std::vector<IncludeDir> includeDirs;  // -I<dir>
  std::vector<std::string> others;      // e.g., -pthread, -fPIC

  CFlags() noexcept = default;
  CFlags(
      std::vector<Macro> macros, std::vector<IncludeDir> includeDirs,
      std::vector<std::string> others
  ) noexcept
      : macros(std::move(macros)), includeDirs(std::move(includeDirs)),
        others(std::move(others)) {}

  static Result<CFlags> parsePkgConfig(std::string_view pkgConfigVer) noexcept;

  void merge(const CFlags& other) noexcept;
};

struct LibDir {
  fs::path dir;

  explicit LibDir(fs::path dir) noexcept : dir(std::move(dir)) {}
};

struct Lib {
  std::string name;

  explicit Lib(std::string name) noexcept : name(std::move(name)) {}
};

struct LdFlags {
  std::vector<LibDir> libDirs;      // -L<dir>
  std::vector<Lib> libs;            // -l<lib>
  std::vector<std::string> others;  // e.g., -Wl,...

  LdFlags() noexcept = default;
  LdFlags(
      std::vector<LibDir> libDirs, std::vector<Lib> libs,
      std::vector<std::string> others
  ) noexcept
      : libDirs(std::move(libDirs)), libs(std::move(libs)),
        others(std::move(others)) {}

  static Result<LdFlags> parsePkgConfig(std::string_view pkgConfigVer) noexcept;

  void merge(const LdFlags& other) noexcept;
};

struct CompilerOptions {
  CFlags cFlags;
  LdFlags ldFlags;

  CompilerOptions() noexcept = default;
  CompilerOptions(CFlags cFlags, LdFlags ldFlags) noexcept
      : cFlags(std::move(cFlags)), ldFlags(std::move(ldFlags)) {}

  static Result<CompilerOptions> parsePkgConfig(
      const VersionReq& pkgVerReq, std::string_view pkgName
  ) noexcept;

  void merge(const CompilerOptions& other) noexcept;
};

}  // namespace cabin

template <>
struct fmt::formatter<cabin::Macro> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::Macro& m, FormatContext& ctx) const {
    if (m.value.empty()) {
      return fmt::format_to(ctx.out(), "-D{}", m.name);
    }
    return fmt::format_to(ctx.out(), "-D{}={}", m.name, m.value);
  }
};

template <>
struct fmt::formatter<cabin::IncludeDir> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::IncludeDir& id, FormatContext& ctx) const {
    if (id.isSystem) {
      return fmt::format_to(ctx.out(), "-isystem{}", id.dir.string());
    }
    return fmt::format_to(ctx.out(), "-I{}", id.dir.string());
  }
};

template <>
struct fmt::formatter<cabin::CFlags> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::CFlags& cf, FormatContext& ctx) const {
    return fmt::format_to(
        ctx.out(), "{} {} {}", fmt::join(cf.macros, " "),
        fmt::join(cf.includeDirs, " "), fmt::join(cf.others, " ")
    );
  }
};

template <>
struct fmt::formatter<cabin::LibDir> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::LibDir& ld, FormatContext& ctx) const {
    return fmt::format_to(ctx.out(), "-L{}", ld.dir.string());
  }
};

template <>
struct fmt::formatter<cabin::Lib> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::Lib& l, FormatContext& ctx) const {
    return fmt::format_to(ctx.out(), "-l{}", l.name);
  }
};

template <>
struct fmt::formatter<cabin::LdFlags> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::LdFlags& lf, FormatContext& ctx) const {
    return fmt::format_to(
        ctx.out(), "{} {} {}", fmt::join(lf.libDirs, " "),
        fmt::join(lf.libs, " "), fmt::join(lf.others, " ")
    );
  }
};

template <>
struct fmt::formatter<cabin::CompilerOptions> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) {
    return ctx.begin();
  }

  template <typename FormatContext>
  auto format(const cabin::CompilerOptions& co, FormatContext& ctx) const {
    return fmt::format_to(ctx.out(), "{} {}", co.cFlags, co.ldFlags);
  }
};
