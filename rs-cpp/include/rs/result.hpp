#ifndef RS_RESULT_HPP
#define RS_RESULT_HPP

#include <memory>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/result/result.hpp>
#include <type_traits>
#include <utility>

// NOLINTBEGIN(readability-identifier-naming,cppcoreguidelines-macro-usage)

#define rs_try(...) MITAMA_TRY(__VA_ARGS__)
#define rs_bail(...) MITAMA_BAIL(__VA_ARGS__)
#define rs_ensure(...) MITAMA_ENSURE(__VA_ARGS__)

namespace rs {

using mitama::anyhow::anyhow;

// FIXME: shared_ptr is an implementation detail. Upstream the fix.
using AnyhowErr = mitama::failure_t<std::shared_ptr<mitama::anyhow::error>>;

struct UseAnyhow {};

template <typename T, typename E = UseAnyhow>
using Result =
    std::conditional_t<std::is_same_v<E, UseAnyhow>, mitama::anyhow::result<T>,
                       mitama::result<T, E>>;

template <typename... Args>
inline auto Ok(Args&&... args)
    -> decltype(mitama::success(std::forward<Args>(args)...)) {
  return mitama::success(std::forward<Args>(args)...);
}

template <typename E = void, typename... Args>
  requires std::is_void_v<E> || std::is_base_of_v<mitama::anyhow::error, E>
inline auto Err(Args&&... args) {
  if constexpr (std::is_void_v<E>) {
    return mitama::failure(std::forward<Args>(args)...);
  } else {
    return mitama::anyhow::failure<E>(std::forward<Args>(args)...);
  }
}

inline constexpr auto to_anyhow = [](auto... xs) {
  return anyhow(std::forward<decltype(xs)>(xs)...);
};

} // namespace rs

// NOLINTEND(readability-identifier-naming,cppcoreguidelines-macro-usage)

#endif // RS_RESULT_HPP
