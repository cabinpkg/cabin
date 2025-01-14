// Semver version requirement parser
//
// Syntax:
//   versionReq ::= (("=" | compOp)? optVersion) | (comparator "&&" comparator)
//   comparator ::= compOp optVersion
//   optVersion ::= num ("." num ("." num ("-" pre)? ("+" build)? )? )?
//   compOp     ::= ">=" | "<=" | ">" | "<"
//   num        ::= <defined in Semver.hpp>
//   pre        ::= <defined in Semver.hpp>
//   build      ::= <defined in Semver.hpp>
//
// Note: Whitespace is permitted around versionReq, comparator, and
// optVersion.  Build metadata will be just ignored but accepted by the
// parser.
#pragma once

#include "Semver.hpp"

#include <cstdint>
#include <optional>
#include <ostream>
#include <string>
#include <string_view>

struct OptVersion {
  uint64_t major{};
  std::optional<uint64_t> minor;
  std::optional<uint64_t> patch;
  Prerelease pre;
};

// 1. NoOp: (Caret (^), "compatible" updates)
//   1.1. `A.B.C` (where A > 0) is equivalent to `>=A.B.C && <(A+1).0.0`
//   1.2. `A.B` (where A > 0 & B > 0) is equivalent to `^A.B.0` (i.e., 1.1)
//   1.3. `A` is equivalent to `=A` (i.e., 2.3)
//   1.4. `0.B.C` (where B > 0) is equivalent to `>=0.B.C && <0.(B+1).0`
//   1.5. `0.0.C` is equivalent to `=0.0.C` (i.e., 2.1)
//   1.6. `0.0` is equivalent to `=0.0` (i.e., 2.2)
//
// 2. Exact:
//   2.1. `=A.B.C` is exactly the version `A.B.C`
//   2.2. `=A.B` is equivalent to `>=A.B.0 && <A.(B+1).0`
//   2.3. `=A` is equivalent to `>=A.0.0 && <(A+1).0.0`
//
// 3. Gt:
//   3.1. `>A.B.C` is equivalent to `>=A.B.(C+1)`
//   3.2. `>A.B` is equivalent to `>=A.(B+1).0`
//   3.3. `>A` is equivalent to `>=(A+1).0.0`
//
// 4. Gte:
//   4.1. `>=A.B.C`
//   4.2. `>=A.B` is equivalent to `>=A.B.0`
//   4.3. `>=A` is equivalent to `>=A.0.0`
//
// 5. Lt:
//   5.1. `<A.B.C`
//   5.2. `<A.B` is equivalent to `<A.B.0`
//   5.3. `<A` is equivalent to `<A.0.0`
//
// 6. Lte:
//   6.1. `<=A.B.C` is equivalent to `<A.B.(C+1)`
//   6.2. `<=A.B` is equivalent to `<A.(B+1).0`
//   6.3. `<=A` is equivalent to `<(A+1).0.0`
struct Comparator {
  enum class Op : uint8_t {
    Exact,  // =
    Gt,     // >
    Gte,    // >=
    Lt,     // <
    Lte,    // <=
  };
  using enum Op;

  std::optional<Op> op;
  uint64_t major{};
  std::optional<uint64_t> minor;
  std::optional<uint64_t> patch;
  Prerelease pre;

  static Result<Comparator> parse(std::string_view str) noexcept;
  void from(const OptVersion& ver) noexcept;
  std::string toString() const noexcept;
  std::string toPkgConfigString() const noexcept;
  bool satisfiedBy(const Version& ver) const noexcept;
  Comparator canonicalize() const noexcept;
};

struct VersionReq {
  Comparator left;
  std::optional<Comparator> right;

  static Result<VersionReq> parse(std::string_view str) noexcept;
  static Result<VersionReq> tryParse(std::string_view str) noexcept;
  bool satisfiedBy(const Version& ver) const noexcept;
  std::string toString() const noexcept;
  std::string toPkgConfigString(std::string_view name) const noexcept;
  VersionReq canonicalize() const noexcept;
  bool canSimplify() const noexcept;
};

std::ostream& operator<<(std::ostream& os, const VersionReq& req);
