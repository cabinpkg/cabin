#ifndef POAC_UTIL_SEMVER_INTERVAL_HPP_
#define POAC_UTIL_SEMVER_INTERVAL_HPP_

// std
#include <optional>
#include <regex>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility> // std::move
#include <variant>

// external
#include <fmt/core.h> // NOLINT(build/include_order)

// internal
#include "poac/util/semver/comparison.hpp"
#include "poac/util/semver/exception.hpp"
#include "poac/util/semver/parser.hpp"
#include "poac/util/semver/token.hpp"

namespace semver {

inline constexpr std::string_view base_version_format = "{}.{}.{}"; // 1.2.3
inline constexpr std::string_view version_format =
    "{}.{}.{}{}{}"; // 1.2.3-beta+11.22.33

inline auto
make_pre_or_build(const char& prefix, std::smatch::const_reference match_item)
    -> std::string {
  return match_item.matched ? (prefix + match_item.str()) : "";
}

template <std::size_t Start, std::size_t... Is>
constexpr auto add_offset(std::index_sequence<Is...> /*unused*/)
    -> std::index_sequence<(Start + Is)...> {
  return {};
}
template <std::size_t Start, std::size_t End>
constexpr auto make_range() {
  return add_offset<Start>(std::make_index_sequence<End - Start + 1>{});
}

template <std::size_t Pre, std::size_t Build, std::size_t... Is>
auto make_version_impl(
    const std::smatch& match, std::index_sequence<Is...> /*unused*/
) -> std::string {
  if constexpr (Pre == 0 || Build == 0) {
    return fmt::format(base_version_format, match[Is].str()...);
  } else {
    return fmt::format(
        version_format, match[Is].str()..., make_pre_or_build('-', match[Pre]),
        make_pre_or_build('+', match[Build])
    );
  }
}

template <
    std::size_t Start, std::size_t End, std::size_t Pre, std::size_t Build>
inline auto make_version(const std::smatch& match) -> std::string {
  static_assert(End - Start == 2, "make_version: range diff should be 2");
  return make_version_impl<Pre, Build>(match, make_range<Start, End>());
}

template <std::size_t Start, std::size_t End>
auto make_version(const std::smatch& match) -> std::string {
  if constexpr (End - Start == 2) { // not include pre & build
    return make_version<Start, End, 0, 0>(match);
  } else if constexpr (End - Start == 4) {
    // pop back pre & build
    return make_version<Start, End - 2, End - 1, End>(match);
  } else {
    static_assert(
        [] { return false; }(), "make_version: range diff should be 2 or 4"
    );
  }
}

class ExactVersion {
private:
  const std::string version;

public:
  explicit ExactVersion(std::string_view version) : version{version} {}

  [[nodiscard]] inline auto satisfies(std::string_view interval) const noexcept
      -> bool {
    return version == interval;
  }
};

class BoundedInterval {
private:
  const std::string left_comp_op;
  const std::string right_comp_op;
  const std::string left_version;
  const std::string right_version;

  [[nodiscard]] auto satisfies_impl(std::string_view v) const -> bool;

  // e.g. `>0.1.3 and >=0.3.2`, `<0.1.3 and <0.3.2`
  [[nodiscard]] auto is_wasteful_comparison_operation() const
      -> std::optional<std::string>;

  // Check if it is bounded interval
  //  (If it is unbounded, throw error)
  // (1, 6) => open bounded interval => OK!
  // [1, 6] => closed bounded interval => OK!
  // [a, ∞) => closed unbounded interval => one_exp
  // (-∞, ∞) => closed unbounded interval => ERR!
  // e.g. <0.1.1 and >=0.3.2
  [[nodiscard]] auto is_bounded_interval() const -> std::optional<std::string>;

public:
  explicit BoundedInterval(const std::smatch& match, std::string_view interval);

  [[nodiscard]] inline auto satisfies(std::string_view version) const -> bool {
    return satisfies_impl(version);
  }
};

class ClosedUnboundedInterval {
private:
  const std::string comp_op;
  const std::string version_str;

  // >2.3.0, 1.0.0, <=1.2.3-alpha, ...
  [[nodiscard]] auto satisfies_impl(std::string_view v) const -> bool;

public:
  explicit ClosedUnboundedInterval(
      std::string_view comp_op, std::string_view version_str
  )
      : comp_op{comp_op}, version_str{version_str} {}

  [[nodiscard]] inline auto satisfies(std::string_view version) const -> bool {
    return satisfies_impl(version);
  }
};

// TODO(ken-matsui): implement a parser for interval
// The following Regular Expressions can be used for tokenizing,
// validating, and parsing SemVer version strings.
// A regular expression before binding is
// https://github.com/semver/semver/issues/232#issue-48635632

// ## Numeric Identifier
// A single `0`, or a non-zero digit followed by zero or more digits.
inline const std::string NUMERIC_IDENTIFIER = R"(0|[1-9]\d*)";

// ## Non-numeric Identifier
// Zero or more digits, followed by a letter or hyphen, and then zero or
// more letters, digits, or hyphens.
inline const std::string NON_NUMERIC_IDENTIFIER =
    R"(\d*[a-zA-Z-][0-9a-zA-Z-]*)";

// ## Main Version
// Three dot-separated numeric identifiers.
inline const std::string MAIN_VERSION = "(" + NUMERIC_IDENTIFIER + R"()\.)"
                                        + "(" + NUMERIC_IDENTIFIER + R"()\.)"
                                        + "(" + NUMERIC_IDENTIFIER + ")";

// ## Pre-release Version Identifier
// A numeric identifier, or a non-numeric identifier.
inline const std::string PRE_RELEASE_IDENTIFIER =
    "(?:" + NUMERIC_IDENTIFIER + "|" + NON_NUMERIC_IDENTIFIER + ")";

// ## Pre-release Version
// Hyphen, followed by one or more dot-separated pre-release version
// identifiers.
inline const std::string PRE_RELEASE = "(?:-(" + PRE_RELEASE_IDENTIFIER
                                       + R"((?:\.)" + PRE_RELEASE_IDENTIFIER
                                       + ")*))";

// ## Build Metadata Identifier
// Any combination of digits, letters, or hyphens.
inline const std::string BUILD_IDENTIFIER = "[0-9A-Za-z-]+";

// ## Build Metadata
// Plus sign, followed by one or more period-separated build metadata
// identifiers.
inline const std::string BUILD =
    R"((?:\+()" + BUILD_IDENTIFIER + R"((?:\.)" + BUILD_IDENTIFIER + ")*))";

// Note that the only major, minor, patch, and pre-release sections of
// the version string are capturing groups.  The build metadata is not a
// capturing group, because it should not ever be used in version
// comparison.
inline const std::string FULL_PLAIN =
    //            "v?" +
    MAIN_VERSION + PRE_RELEASE + "?" + BUILD + "?";

// ## Interval of Version String
// Something like ">1.2.0 and <=2.0.0".
// A simple gt/lt/eq thing, or just "" to indicate "any version"
inline const std::string GT_LT = "((?:<|>)?=?)";
inline const std::string CLOSED_UNBOUNDED_INTERVAL =
    "(^" + GT_LT + "?" + FULL_PLAIN + "$)";
inline const std::string BOUNDED_INTERVAL =
    "(^" + GT_LT + FULL_PLAIN + "( and )" + GT_LT + FULL_PLAIN + "$)";

class Interval {
private:
  using IntervalClass =
      std::variant<ExactVersion, BoundedInterval, ClosedUnboundedInterval>;

  const std::string interval;
  const IntervalClass interval_class;

  template <typename T, typename... U>
  [[nodiscard]] inline auto make_interval_class(U&&... args) const
      -> IntervalClass {
    return IntervalClass{std::in_place_type<T>, std::forward<U>(args)...};
  }

  inline auto interval_match(std::smatch& match, const std::string& re) const
      -> bool {
    return std::regex_match(interval, match, std::regex(re));
  }

  [[nodiscard]] auto get_interval_class() const -> IntervalClass;

public:
  explicit Interval(std::string_view i)
      : interval(i), interval_class{get_interval_class()} {}

  [[nodiscard]] inline auto satisfies(std::string_view version) const -> bool {
    return std::visit(
        [version](const auto& i) { return i.satisfies(version); },
        interval_class
    );
  }
};

} // end namespace semver

#endif // POAC_UTIL_SEMVER_INTERVAL_HPP_
