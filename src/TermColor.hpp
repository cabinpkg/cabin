#pragma once

#include <cstdint>
#include <cstdio>
#include <fmt/core.h>
#include <fmt/ranges.h>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>

namespace cabin {

void setColorMode(std::string_view str) noexcept;
bool shouldColor(FILE* file) noexcept;
bool shouldColorStdout() noexcept;
bool shouldColorStderr() noexcept;

class ColorStr {
  std::vector<std::uint8_t> codes;
  std::string str;

public:
  ColorStr(const std::uint8_t code, std::string str) noexcept
      : str(std::move(str)) {
    codes.push_back(code);
  }

  template <typename S>
    requires(
        std::is_convertible_v<std::remove_cvref_t<S>, std::string_view>  //
        && !std::is_same_v<std::remove_cvref_t<S>, std::string>
    )
  ColorStr(const std::uint8_t code, S&& str) noexcept
      : ColorStr(code, std::string(std::forward<S>(str))) {}

  ColorStr(const std::uint8_t code, ColorStr other) noexcept
      : codes(std::move(other.codes)), str(std::move(other.str)) {
    codes.push_back(code);
  }

  ColorStr(const ColorStr&) = delete;
  ColorStr& operator=(const ColorStr&) = delete;
  ColorStr(ColorStr&&) noexcept = default;
  ColorStr& operator=(ColorStr&&) noexcept = default;
  virtual ~ColorStr() noexcept = default;

  std::string toStr(FILE* file) const noexcept {
    if (shouldColor(file)) {
      return fmt::format("\033[{}m{}\033[0m", fmt::join(codes, ";"), str);
    }
    return str;
  }

  std::string toStr() const noexcept { return toStr(stdout); }
  std::string toErrStr() const noexcept { return toStr(stderr); }
};

class Gray : public ColorStr {
  static constexpr std::uint8_t CODE = 30;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Gray(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Gray(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Red : public ColorStr {
  static constexpr std::uint8_t CODE = 31;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Red(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Red(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Green : public ColorStr {
  static constexpr std::uint8_t CODE = 32;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Green(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Green(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Yellow : public ColorStr {
  static constexpr std::uint8_t CODE = 33;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Yellow(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Yellow(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Blue : public ColorStr {
  static constexpr std::uint8_t CODE = 34;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Blue(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Blue(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Magenta : public ColorStr {
  static constexpr std::uint8_t CODE = 35;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Magenta(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Magenta(ColorStr other) noexcept
      : ColorStr(CODE, std::move(other)) {}
};

class Cyan : public ColorStr {
  static constexpr std::uint8_t CODE = 36;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Cyan(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Cyan(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

class Bold : public ColorStr {
  static constexpr std::uint8_t CODE = 1;

public:
  template <typename S>
    requires(!std::is_base_of_v<ColorStr, S>)
  explicit Bold(S&& str) noexcept : ColorStr(CODE, std::forward<S>(str)) {}

  explicit Bold(ColorStr other) noexcept : ColorStr(CODE, std::move(other)) {}
};

}  // namespace cabin
