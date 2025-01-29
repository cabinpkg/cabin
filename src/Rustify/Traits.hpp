#pragma once

#include <concepts>

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
