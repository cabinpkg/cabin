#pragma once

#include <concepts>
#include <exception>
#include <fmt/core.h>
#include <memory>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/result/result.hpp>
#include <string>
#include <type_traits>
#include <utility>

namespace anyhow = mitama::anyhow;

// NOLINTBEGIN(readability-identifier-naming,cppcoreguidelines-macro-usage)

#define Try(...) MITAMA_TRY(__VA_ARGS__)
#define Bail(...) MITAMA_BAIL(__VA_ARGS__)
#define Ensure(...) MITAMA_ENSURE(__VA_ARGS__)

// FIXME: shared_ptr is an implementation detail. Upstream the fix.
using AnyhowErr = mitama::failure_t<std::shared_ptr<anyhow::error>>;

struct UseAnyhow {};

template <typename T, typename E = UseAnyhow>
using Result = std::conditional_t<
    std::is_same_v<E, UseAnyhow>, anyhow::result<T>, mitama::result<T, E>>;

template <typename... Args>
inline auto
Ok(Args&&... args) -> decltype(mitama::success(std::forward<Args>(args)...)) {
  return mitama::success(std::forward<Args>(args)...);
}

template <typename E = void, typename... Args>
  requires std::is_void_v<E> || std::is_base_of_v<anyhow::error, E>
inline auto
Err(Args&&... args) {
  if constexpr (std::is_void_v<E>) {
    return mitama::failure(std::forward<Args>(args)...);
  } else {
    return anyhow::failure<E>(std::forward<Args>(args)...);
  }
}

inline constexpr auto to_anyhow = [](auto... xs) {
  return anyhow::anyhow(std::forward<decltype(xs)>(xs)...);
};

#if __has_include(<toml.hpp>)

#  include <toml.hpp>

namespace toml {

template <typename T, typename... U>
inline Result<T>
try_find(const toml::value& v, const U&... u) noexcept {
  using namespace std::string_view_literals;  // NOLINT

  try {
    return Ok(toml::find<T>(v, u...));
  } catch (const std::exception& e) {
    std::string what = e.what();
    // TODO: make the same fix on upstream
    if (what.starts_with("[error] ")) {
      what = what.substr("[error] "sv.size());
    }
    if (what.back() == '\n') {
      what.pop_back();  // remove the last '\n' since logger::error adds one.
    }
    return Err(anyhow::anyhow(what));
  }
}

}  // namespace toml

#endif

// NOLINTEND(readability-identifier-naming,cppcoreguidelines-macro-usage)
