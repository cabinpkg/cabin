#ifndef POAC_HPP
#define POAC_HPP

// std
#include <cstdint>
#include <cstddef> // std::size_t
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <variant> // std::monostate
#include <vector>

// external
#include <fmt/core.h>
#include <mitama/result/result.hpp>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/thiserror/thiserror.hpp>

// internal
#include <poac/util/termcolor2/termcolor2.hpp>

#ifdef NDEBUG
#  define unreachable() __builtin_unreachable()
#else
#  define unreachable() assert(0 && "unreachable")
#endif

#define alias_fn(lhs, rhs) \
template <typename... Args> \
inline auto lhs(Args&&... args) -> decltype(rhs(std::forward<Args>(args)...)) { \
    return rhs(std::forward<Args>(args)...); \
}

namespace poac {
    //
    // namespaces
    //
    namespace fs = std::filesystem;
    namespace anyhow = mitama::anyhow;
    namespace thiserror = mitama::thiserror;

    using namespace std::literals::string_literals;
    using namespace std::literals::string_view_literals;
    using namespace termcolor2::color_literals;

    //
    // data types
    //
    using  u8 = std::uint8_t;
    using u16 = std::uint16_t;
    using u32 = std::uint32_t;
    using u64 = std::uint64_t;

    using  i8 = std::int8_t;
    using i16 = std::int16_t;
    using i32 = std::int32_t;
    using i64 = std::int64_t;

    using usize = std::size_t;

    using String = std::string;
    using StringRef = std::string_view;
    static_assert(String::npos == StringRef::npos, "npos should be the same");
    inline constexpr usize SNone = StringRef::npos;

    template <typename T>
    using Vec = std::vector<T>;

    template <typename T, typename E = std::monostate>
    using Result =
        std::conditional_t<
            std::is_same_v<E, std::monostate>,
            anyhow::result<T>,
            mitama::result<T, E>
        >;
    alias_fn(Ok, mitama::success);
    template <typename E = std::monostate, typename... Args>
    inline auto Err(Args&&... args)
        -> std::conditional_t<
               std::is_same_v<E, std::monostate>,
               decltype(anyhow::failure<E>(std::forward<Args>(args)...)),
               decltype(mitama::failure(std::forward<Args>(args)...))
           > {
        if constexpr (std::is_same_v<E, std::monostate>) {
            return anyhow::failure<E>(std::forward<Args>(args)...);
        } else {
            return mitama::failure(std::forward<Args>(args)...);
        }
    }

    template <typename T>
    using Option = std::optional<T>;
    inline constexpr std::nullopt_t None = std::nullopt;

    //
    // utilities
    //
    using fmt::format;
    using fmt::print;

    const auto to_anyhow = [](const String& e){ return anyhow::anyhow(e); };
} // end namespace

#endif // !POAC_HPP
