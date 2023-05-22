module;

// std
#include <cstdint>
#include <optional>
#include <stdexcept>
#include <variant>
#include <vector>

export module semver.range;

import semver.token;

export namespace semver {

enum class WildcardVersion {
  /// Wildcard minor version `1.*.3`.
  Minor,
  /// Wildcard patch version `1.2.*`.
  Patch
};

struct Op {
  enum Kind {
    /// Exact, `=`.
    Ex,
    /// Greater than, `>`.
    Gt,
    /// Greater than or equal to, `>=`.
    GtEq,
    /// Less than, `<`.
    Lt,
    /// Less than or equal to, `<=`.
    LtEq,
    /// Tilde requirements, like `~1.0.0` - a minimal version with some ability
    /// to update.
    Tilde,
    /// Compatible by definition of semver, indicated by `^`.
    Compatible,
    /// `x.y.*`, `x.*`, `*`.
    Wildcard
  };

  Kind kind;
  std::variant<std::monostate, WildcardVersion> component;

  Op() = delete;
  Op(const Op&) = default;
  auto operator=(const Op&) -> Op& = default;
  Op(Op&&) noexcept = default;
  auto operator=(Op&&) noexcept -> Op& = default;
  ~Op() = default;

  constexpr explicit Op(Kind k)
      : kind(
          k == Kind::Wildcard ? throw std::invalid_argument("semver::Op") : k
      ) {}

  constexpr Op(Kind k, WildcardVersion wv)
      : kind(
          k != Kind::Wildcard ? throw std::invalid_argument("semver::Op") : k
      ),
        component(wv) {}
};

struct Predicate {
  /// Operation code for this predicate, like "greater than" or "exact match".
  Op op;
  /// Major version.
  std::uint_fast64_t major;
  /// Optional minor version.
  std::optional<std::uint_fast64_t> minor;
  /// Optional patch version.
  std::optional<std::uint_fast64_t> patch;
  /// Collection of `Identifier`s of version, like `"alpha1"` in
  /// `"1.2.3-alpha1"`.
  std::vector<Identifier> pre;
};

struct VersionReq {
  /// Collection of predicates.
  std::vector<Predicate> predicates;
};

/// A single range set combining a number of ranges with an or (`||`).
///
/// If any range in this set matches, the whole set matches.
struct Comparator {
  std::vector<VersionReq> ranges;
};

} // end namespace semver
