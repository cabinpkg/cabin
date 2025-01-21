#pragma once

#include "Traits.hpp"

#include <cstddef>
#include <cstdlib>
#include <iostream>
#include <source_location>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

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
  std::cout << "      test " << getModName(loc.file_name())
            << "::" << prettifyFuncName(loc.function_name()) << " ... " << GREEN
            << "ok" << RESET << '\n'
            << std::flush;
}

[[noreturn]] inline void
error(const std::source_location& loc, Display auto&&... msgs) {
  std::ostringstream oss;
  oss << "\n      test " << getModName(loc.file_name())
      << "::" << prettifyFuncName(loc.function_name()) << " ... " << RED
      << "FAILED" << RESET << "\n\n"
      << '\'' << prettifyFuncName(loc.function_name()) << "' failed at '"
      << std::boolalpha;
  (oss << ... << std::forward<decltype(msgs)>(msgs))
      << "', " << loc.file_name() << ':' << loc.line() << '\n';

  std::string str = oss.str();
  std::cerr << str;
  throw std::logic_error(str);
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
  requires(Eq<Lhs, Rhs> && Display<Lhs> && Display<Rhs>)
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
        loc, "assertion failed: `(left == right)`\n", "  left: `",
        std::forward<Lhs>(lhs), "`\n", " right: `", std::forward<Rhs>(rhs),
        "`\n"
    );
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires(Ne<Lhs, Rhs> && Display<Lhs> && Display<Rhs>)
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
        loc, "assertion failed: `(left != right)`\n", "  left: `",
        std::forward<Lhs>(lhs), "`\n", " right: `", std::forward<Rhs>(rhs),
        "`\n"
    );
  } else {
    error(loc, msg);
  }
}

template <typename Lhs, typename Rhs>
  requires(Lt<Lhs, Rhs> && Display<Lhs> && Display<Rhs>)
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
        loc, "assertion failed: `(left < right)`\n", "  left: `",
        std::forward<Lhs>(lhs), "`\n", " right: `", std::forward<Rhs>(rhs),
        "`\n"
    );
  } else {
    error(loc, msg);
  }
}

}  // namespace tests
