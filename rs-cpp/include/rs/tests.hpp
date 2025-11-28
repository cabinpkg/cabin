#ifndef RS_TESTS_HPP
#define RS_TESTS_HPP

#include <concepts>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fmt/core.h>
#include <fmt/std.h>
#include <print>
#include <source_location>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

namespace tests {

inline constinit const std::string_view GREEN = "\033[32m";
inline constinit const std::string_view RED = "\033[31m";
inline constinit const std::string_view RESET = "\033[0m";

template <typename T, typename U>
concept Eq = requires(T lhs, U rhs) {
  { lhs == rhs } -> std::convertible_to<bool>;
};

template <typename T, typename U>
concept Ne = requires(T lhs, U rhs) {
  { lhs != rhs } -> std::convertible_to<bool>;
};

template <typename T, typename U>
concept Lt = requires(T lhs, U rhs) {
  { lhs < rhs } -> std::convertible_to<bool>;
};

std::string getModName(std::string_view file) noexcept {
  return std::filesystem::relative(file).replace_extension().string();
}

constexpr std::string_view prettifyFuncName(std::string_view func) noexcept {
  if (func.empty()) {
    return func;
  }

  const std::size_t end = func.find_last_of('(');
  if (end == std::string_view::npos) {
    return func;
  }
  func = func.substr(0, end);

  const std::size_t start = func.find_last_of(' ');
  if (start == std::string_view::npos) {
    return func;
  }
  return func.substr(start + 1);
}

inline void pass(const std::source_location& loc =
                     std::source_location::current()) noexcept {
  std::print("        test {}::{} ... {}ok{}\n", getModName(loc.file_name()),
             prettifyFuncName(loc.function_name()), GREEN, RESET);
}

[[noreturn]] inline void error(const std::source_location& loc,
                               const std::string_view msg) {
  std::print(stderr,
             "\n        test {}::{} ... {}FAILED{}\n\n"
             "'{}' failed at '{}', {}:{}\n",
             getModName(loc.file_name()), prettifyFuncName(loc.function_name()),
             RED, RESET, prettifyFuncName(loc.function_name()), msg,
             loc.file_name(), loc.line());
  throw std::logic_error("test failed");
}

inline void
assertTrue(const bool cond, const std::string_view msg = "",
           const std::source_location& loc = std::source_location::current()) {
  if (cond) {
    return; // OK
  }

  if (msg.empty()) {
    error(loc, "expected `true` but got `false`");
  } else {
    error(loc, msg);
  }
}

inline void
assertFalse(const bool cond, const std::string_view msg = "",
            const std::source_location& loc = std::source_location::current()) {
  if (!cond) {
    return; // OK
  }

  if (msg.empty()) {
    error(loc, "expected `false` but got `true`");
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires Eq<Lhs, Rhs> && fmt::is_formattable<Lhs>::value
           && fmt::is_formattable<Rhs>::value
inline void
assertEq(Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
         const std::source_location& loc = std::source_location::current()) {
  if (lhs == rhs) {
    return; // OK
  }

  if (msg.empty()) {
    std::string msg;
    try {
      msg = fmt::format(fmt::runtime("assertion failed: `(left == right)`\n"
                                     "  left: `{:?}`\n"
                                     " right: `{:?}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    } catch (const fmt::format_error& e) {
      msg = fmt::format(fmt::runtime("assertion failed: `(left == right)`\n"
                                     "  left: `{}`\n"
                                     " right: `{}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    }
    error(loc, msg);
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires Ne<Lhs, Rhs> && fmt::is_formattable<Lhs>::value
           && fmt::is_formattable<Rhs>::value
inline void
assertNe(Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
         const std::source_location& loc = std::source_location::current()) {
  if (lhs != rhs) {
    return; // OK
  }

  if (msg.empty()) {
    std::string msg;
    try {
      msg = fmt::format(fmt::runtime("assertion failed: `(left != right)`\n"
                                     "  left: `{:?}`\n"
                                     " right: `{:?}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    } catch (const fmt::format_error& e) {
      msg = fmt::format(fmt::runtime("assertion failed: `(left != right)`\n"
                                     "  left: `{}`\n"
                                     " right: `{}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    }
    error(loc, msg);
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires Lt<Lhs, Rhs> && fmt::is_formattable<Lhs>::value
           && fmt::is_formattable<Rhs>::value
inline void
assertLt(Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
         const std::source_location& loc = std::source_location::current()) {
  if (lhs < rhs) {
    return; // OK
  }

  if (msg.empty()) {
    std::string msg;
    try {
      msg = fmt::format(fmt::runtime("assertion failed: `(left < right)`\n"
                                     "  left: `{:?}`\n"
                                     " right: `{:?}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    } catch (const fmt::format_error& e) {
      msg = fmt::format(fmt::runtime("assertion failed: `(left < right)`\n"
                                     "  left: `{}`\n"
                                     " right: `{}`\n"),
                        std::forward<Lhs>(lhs), std::forward<Rhs>(rhs));
    }
    error(loc, msg);
  } else {
    error(loc, msg);
  }
}

} // namespace tests

#endif // RS_TESTS_HPP
