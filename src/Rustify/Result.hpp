#pragma once

#include <exception>
#include <fmt/core.h>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/result/result.hpp>
#include <mitama/thiserror/thiserror.hpp>
#include <string>
#include <type_traits>
#include <utility>

namespace anyhow = mitama::anyhow;
namespace thiserror = mitama::thiserror;

// NOLINTBEGIN(readability-identifier-naming,cppcoreguidelines-macro-usage)

#define Try(...) MITAMA_TRY(__VA_ARGS__)
#define Bail(...) return Err(Anyhow(__VA_ARGS__))

template <typename T, typename E = void>
using Result = std::conditional_t<
    std::is_void_v<E>, anyhow::result<T>, mitama::result<T, E>>;

template <typename... Args>
inline auto
Ok(Args&&... args)
    -> decltype(mitama::success(std::forward<Args>(args)...)) {
  return mitama::success(std::forward<Args>(args)...);
}

template <typename E = void, typename... Args>
inline auto
Err(Args&&... args) {
  if constexpr (std::is_void_v<E>) {
    return mitama::failure(std::forward<Args>(args)...);
  } else {
    return anyhow::failure<E>(std::forward<Args>(args)...);
  }
}

template <thiserror::fixed_string S, typename... T>
using Error = thiserror::error<S, T...>;

template <typename... T>
inline auto
Anyhow(fmt::format_string<T...> f, T&&... args) {
  return anyhow::anyhow(fmt::format(f, std::forward<T>(args)...));
}
template <typename T>
inline auto
Anyhow(T&& arg) {
  return anyhow::anyhow(std::forward<T>(arg));
}

inline constexpr auto to_anyhow = [](std::string e) {
  return anyhow::anyhow(std::move(e));
};

#if __has_include(<toml.hpp>)

#  include <toml.hpp>

namespace toml {

template <typename T, typename... U>
inline auto
try_find(const toml::value& v, const U&... u) noexcept
    -> Result<decltype(toml::find<T>(v, u...))> {
  try {
    return Ok(toml::find<T>(v, u...));
  } catch (const std::exception& e) {
    return Err(anyhow::anyhow(std::string(e.what())));
  }
}

}  // namespace toml

#endif

// NOLINTEND(readability-identifier-naming,cppcoreguidelines-macro-usage)
