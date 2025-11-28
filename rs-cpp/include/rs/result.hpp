#ifndef RS_RESULT_HPP
#define RS_RESULT_HPP

#include <memory>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/result/result.hpp>
#include <type_traits>
#include <utility>

// NOLINTBEGIN(readability-identifier-naming,cppcoreguidelines-macro-usage)

#define Try(...) MITAMA_TRY(__VA_ARGS__)
#define Bail(...) MITAMA_BAIL(__VA_ARGS__)
#define Ensure(...) MITAMA_ENSURE(__VA_ARGS__)

namespace anyhow = mitama::anyhow;

// FIXME: shared_ptr is an implementation detail. Upstream the fix.
using AnyhowErr = mitama::failure_t<std::shared_ptr<anyhow::error>>;

struct UseAnyhow {};

template <typename T, typename E = UseAnyhow>
using Result = std::conditional_t<std::is_same_v<E, UseAnyhow>,
                                  anyhow::result<T>, mitama::result<T, E>>;

template <typename... Args>
inline auto Ok(Args&&... args)
    -> decltype(mitama::success(std::forward<Args>(args)...)) {
  return mitama::success(std::forward<Args>(args)...);
}

template <typename E = void, typename... Args>
  requires std::is_void_v<E> || std::is_base_of_v<anyhow::error, E>
inline auto Err(Args&&... args) {
  if constexpr (std::is_void_v<E>) {
    return mitama::failure(std::forward<Args>(args)...);
  } else {
    return anyhow::failure<E>(std::forward<Args>(args)...);
  }
}

inline constexpr auto to_anyhow = [](auto... xs) {
  return anyhow::anyhow(std::forward<decltype(xs)>(xs)...);
};

// NOLINTEND(readability-identifier-naming,cppcoreguidelines-macro-usage)

#endif // RS_RESULT_HPP
