#pragma once

#include <fmt/format.h>
#include <string>
#include <utility>
#include <variant>

namespace cabin {

class BuildProfile {
  friend struct fmt::formatter<BuildProfile>;
  friend struct std::hash<BuildProfile>;

public:
  enum class Type : uint8_t {
    Dev,
    Release,
    Test,
  };
  using enum Type;

private:
  using TypeT = std::variant<Type, std::string>;
  TypeT type;

public:
  BuildProfile() : type(Type::Dev) {}
  BuildProfile(Type type) : type(type) {} // NOLINT
  explicit BuildProfile(std::string type) : type(std::move(type)) {}

  bool operator==(const BuildProfile& other) const {
    return type == other.type;
  }
};

} // namespace cabin

template <>
struct std::hash<cabin::BuildProfile> {
  std::size_t operator()(const cabin::BuildProfile& bp) const noexcept {
    return std::hash<cabin::BuildProfile::TypeT>{}(bp.type);
  }
};

template <>
struct fmt::formatter<cabin::BuildProfile> {
  // NOLINTNEXTLINE(*-static)
  constexpr auto parse(fmt::format_parse_context& ctx) { return ctx.begin(); }

  template <typename FormatContext>
  auto format(const cabin::BuildProfile& buildProfile,
              FormatContext& ctx) const {
    if (std::holds_alternative<cabin::BuildProfile::Type>(buildProfile.type)) {
      switch (std::get<cabin::BuildProfile::Type>(buildProfile.type)) {
      case cabin::BuildProfile::Dev:
        return fmt::format_to(ctx.out(), "dev");
      case cabin::BuildProfile::Release:
        return fmt::format_to(ctx.out(), "release");
      case cabin::BuildProfile::Test:
        return fmt::format_to(ctx.out(), "test");
      }
      __builtin_unreachable();
    } else {
      return fmt::format_to(ctx.out(), "{}",
                            std::get<std::string>(buildProfile.type));
    }
  }
};
