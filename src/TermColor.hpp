#pragma once

#include <cstdint>
#include <cstdio>
#include <fmt/core.h>
#include <fmt/ostream.h>
#include <fmt/ranges.h>
#include <iostream>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>

namespace cabin {

void setColorMode(std::string_view str) noexcept;
bool shouldColor(const std::ostream& os) noexcept;
bool shouldColorStdout() noexcept;
bool shouldColorStderr() noexcept;

class ColorStr {
  std::vector<std::uint8_t> codes;
  mutable std::string str;
  mutable bool finalized = false;

public:
  ColorStr(const std::uint8_t code, std::string str) noexcept
      : str(std::move(str)) {
    codes.push_back(code);
  }

  template <typename S>
    requires(std::is_convertible_v<std::remove_cvref_t<S>, std::string_view> //
             && !std::is_same_v<std::remove_cvref_t<S>, std::string>)
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

  std::string toStr() const noexcept {
    finalize(std::cout);
    return str;
  }
  std::string toErrStr() const noexcept {
    finalize(std::cerr);
    return str;
  }

  void finalize(const std::ostream& os) const noexcept {
    if (!finalized && shouldColor(os)) {
      finalized = true;
      str = fmt::format("\033[{}m{}\033[0m", fmt::join(codes, ";"), str);
    }
  }

  friend std::ostream&
  operator<<(std::ostream& os, const ColorStr& c) noexcept {
    if (!c.finalized) {
      // NOTE: If this operator<< is called from fmtlib, we cannot detect if
      // the stream is a terminal since they are using a custom stream.  As a
      // result, we always don't color the output in this case.
      c.finalize(os);
    }
    return os << c.str;
  }
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

template <typename T>
inline void
toStdout(T& value) noexcept {
  if constexpr (std::is_base_of_v<ColorStr, T>) {
    value.finalize(std::cout);
  }
}
template <typename T>
inline void
toStderr(T& value) noexcept {
  if constexpr (std::is_base_of_v<ColorStr, T>) {
    value.finalize(std::cerr);
  }
}

template <typename... T>
inline std::string
format(fmt::format_string<T...> fmt, T&&... args) {
  (toStdout(args), ...);
  return fmt::format(fmt, std::forward<T>(args)...);
}
template <typename... T>
inline std::string
eformat(fmt::format_string<T...> fmt, T&&... args) {
  (toStderr(args), ...);
  return fmt::format(fmt, std::forward<T>(args)...);
}

template <typename... T>
inline void
print(fmt::format_string<T...> fmt, T&&... args) {
  (toStdout(args), ...);
  fmt::print(fmt, std::forward<T>(args)...);
}
template <typename... T>
inline void
eprint(fmt::format_string<T...> fmt, T&&... args) {
  (toStderr(args), ...);
  fmt::print(stderr, fmt, std::forward<T>(args)...);
}

template <typename... T>
inline void
println(fmt::format_string<T...> fmt, T&&... args) {
  (toStdout(args), ...);
  fmt::print("{}\n", fmt::format(fmt, std::forward<T>(args)...));
}
inline void
println() {
  fmt::print("\n");
}

}  // namespace cabin

template <>
struct fmt::formatter<cabin::ColorStr> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Gray> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Red> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Green> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Yellow> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Blue> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Magenta> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Cyan> : ostream_formatter {};

template <>
struct fmt::formatter<cabin::Bold> : ostream_formatter {};
