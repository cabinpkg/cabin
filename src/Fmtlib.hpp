#pragma once

#include "TermColor.hpp"

#include <cstdio>
#include <fmt/core.h>
#include <iostream>
#include <string>
#include <type_traits>
#include <utility>

namespace cabin {

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

template <typename... T>
inline void
eprintln(fmt::format_string<T...> fmt, T&&... args) {
  (toStderr(args), ...);
  fmt::print(stderr, "{}\n", fmt::format(fmt, std::forward<T>(args)...));
}
inline void
eprintln() {
  fmt::print(stderr, "\n");
}

}  // namespace cabin
