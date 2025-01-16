#pragma once

#include <concepts>
#include <optional>
#include <ostream>

inline std::ostream&
operator<<(std::ostream& os, std::nullopt_t /*unused*/) {
  os << "None";
  return os;
}

template <typename T>
std::ostream&
operator<<(std::ostream& os, const std::optional<T>& opt) {
  if (opt.has_value()) {
    os << opt.value();
  } else {
    os << "None";
  }
  return os;
}

template <typename T>
concept Display = requires(T val, std::ostream& os) {
  { os << val } -> std::convertible_to<std::ostream&>;
};

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
