#pragma once

#include <array>
#include <cassert>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <map>
#include <optional>
#include <ostream>
#include <set>
#include <string>
#include <string_view>
#include <tuple>
#include <unordered_map>
#include <unordered_set>
#include <variant>
#include <vector>

// NOLINTBEGIN(readability-identifier-naming)
#ifdef NDEBUG
#  define unreachable() __builtin_unreachable()
#else
#  define unreachable() assert(false && "unreachable")
#endif
// NOLINTEND(readability-identifier-naming)

namespace fs = std::filesystem;

// NOLINTBEGIN(readability-identifier-naming)
using u8 = std::uint8_t;
using u16 = std::uint16_t;
using u32 = std::uint32_t;
using u64 = std::uint64_t;

using i8 = std::int8_t;
using i16 = std::int16_t;
using i32 = std::int32_t;
using i64 = std::int64_t;

using isize = std::ptrdiff_t;
using usize = std::size_t;

using f32 = float;
using f64 = double;
// NOLINTEND(readability-identifier-naming)

using String = std::string;
using StringRef = std::string_view;
using Path = fs::path;

template <typename T, usize N>
using Arr = std::array<T, N>;
template <typename T>
using Vec = std::vector<T>;

template <typename K, typename V>
using Map = std::map<K, V>;
template <typename K, typename V>
using HashMap = std::unordered_map<K, V>;

template <typename K>
using Set = std::set<K>;
template <typename K>
using HashSet = std::unordered_set<K>;

template <typename T>
using Fn = std::function<T>;

template <typename T>
using Option = std::optional<T>;

template <typename... Ts>
using Tuple = std::tuple<Ts...>;

struct NoneT : protected std::monostate {
  constexpr bool operator==(const usize rhs) const {
    return String::npos == rhs;
  }

  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr operator std::nullopt_t() const {
    return std::nullopt;
  }

  template <typename T>
  constexpr operator Option<T>() const { // NOLINT(google-explicit-constructor)
    return std::nullopt;
  }
};
inline constexpr NoneT None; // NOLINT(readability-identifier-naming)

inline std::ostream& operator<<(std::ostream& os, const NoneT& /*unused*/) {
  return os << "None";
}

// NOLINTBEGIN(google-global-names-in-headers)
using std::literals::string_literals::operator""s;
using std::literals::string_view_literals::operator""sv;
// NOLINTEND(google-global-names-in-headers)

inline Path operator""_path(const char* str, usize /*unused*/) {
  return str;
}
