#pragma once

#include "Traits.hpp"

#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <fmt/core.h>
#include <fmt/std.h>
#include <optional>
#include <source_location>
#include <stdexcept>
#include <string_view>
#include <utility>

template <>
struct fmt::formatter<std::nullopt_t> : fmt::formatter<std::string_view> {
  auto format(std::nullopt_t /* none */, format_context& ctx) const {
    return formatter<std::string_view>::format("none", ctx);
  }
};

namespace tests {

inline constinit const std::string_view GREEN = "\033[32m";
inline constinit const std::string_view RED = "\033[31m";
inline constinit const std::string_view RESET = "\033[0m";

// Returns the module name from a file path.  There are two cases:
//
// 1. src/Rustify/Tests.cc -> Rustify/Tests
//    (when we run `make test` from the root directory)
// 2. ../../src/Rustify/Tests.cc -> Rustify/Tests
//    (when we run `cabin test`)
//
// We first should remove `../../` if it exists, then remove the first
// directory name since it can be either `src` or `tests`.  Finally, we remove
// the file extension, which is basically any of C++ source file extensions.
constexpr std::string_view
getModName(std::string_view file) noexcept {
  if (file.empty()) {
    return file;
  }

  const std::size_t start = file.find("src/");
  if (start == std::string_view::npos) {
    return file;
  }

  const std::size_t end = file.find_last_of('.');
  if (end == std::string_view::npos) {
    return file;
  }

  return file.substr(start, end - start);
}

constexpr std::string_view
prettifyFuncName(std::string_view func) noexcept {
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

inline void
pass(
    const std::source_location& loc = std::source_location::current()
) noexcept {
  fmt::print(
      "        test {}::{} ... {}ok{}\n", getModName(loc.file_name()),
      prettifyFuncName(loc.function_name()), GREEN, RESET
  );
}

[[noreturn]] inline void
error(const std::source_location& loc, const std::string_view msg) {
  fmt::print(
      stderr,
      "\n        test {}::{} ... {}FAILED{}\n\n"
      "'{}' failed at '{}', {}:{}\n",
      getModName(loc.file_name()), prettifyFuncName(loc.function_name()), RED,
      RESET, prettifyFuncName(loc.function_name()), msg, loc.file_name(),
      loc.line()
  );
  throw std::logic_error("test failed");
}

inline void
assertTrue(
    const bool cond, const std::string_view msg = "",
    const std::source_location& loc = std::source_location::current()
) {
  if (cond) {
    return;  // OK
  }

  if (msg.empty()) {
    error(loc, "expected `true` but got `false`");
  } else {
    error(loc, msg);
  }
}

inline void
assertFalse(
    const bool cond, const std::string_view msg = "",
    const std::source_location& loc = std::source_location::current()
) {
  if (!cond) {
    return;  // OK
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
assertEq(
    Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
    const std::source_location& loc = std::source_location::current()
) {
  if (lhs == rhs) {
    return;  // OK
  }

  if (msg.empty()) {
    error(
        loc, fmt::format(
                 "assertion failed: `(left == right)`\n"
                 "  left: `{}`\n"
                 " right: `{}`\n",
                 std::forward<Lhs>(lhs), std::forward<Rhs>(rhs)
             )
    );
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires Ne<Lhs, Rhs> && fmt::is_formattable<Lhs>::value
           && fmt::is_formattable<Rhs>::value
inline void
assertNe(
    Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
    const std::source_location& loc = std::source_location::current()
) {
  if (lhs != rhs) {
    return;  // OK
  }

  if (msg.empty()) {
    error(
        loc, fmt::format(
                 "assertion failed: `(left != right)`\n"
                 "  left: `{}`\n"
                 " right: `{}`\n",
                 std::forward<Lhs>(lhs), std::forward<Rhs>(rhs)
             )
    );
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires Lt<Lhs, Rhs> && fmt::is_formattable<Lhs>::value
           && fmt::is_formattable<Rhs>::value
inline void
assertLt(
    Lhs&& lhs, Rhs&& rhs, const std::string_view msg = "",
    const std::source_location& loc = std::source_location::current()
) {
  if (lhs < rhs) {
    return;  // OK
  }

  if (msg.empty()) {
    error(
        loc, fmt::format(
                 "assertion failed: `(left < right)`\n"
                 "  left: `{}`\n"
                 " right: `{}`\n",
                 std::forward<Lhs>(lhs), std::forward<Rhs>(rhs)
             )
    );
  } else {
    error(loc, msg);
  }
}

}  // namespace tests
