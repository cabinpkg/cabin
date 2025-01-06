#pragma once

#include "Global.hpp"

#include <cstddef>
#include <git2/oid.h>
#include <ostream>
#include <string>
#include <string_view>

namespace git2 {

inline constexpr std::size_t SHORT_HASH_LEN = 8;

struct Oid : public GlobalState {
  // TODO: ideally, use one of the following:
  git_oid oid{};
  git_oid* raw = nullptr;

  explicit Oid(git_oid oid);

  explicit Oid(git_oid* oid);

  explicit Oid(const git_oid* oid);

  /// Parse a hex-formatted object id into an oid structure.
  explicit Oid(std::string_view str);

  // Since Oid would not be constructed by itself, the destructor is not
  // responsible for freeing the raw pointer.
  Oid() = delete;
  ~Oid() = default;

  Oid(const Oid&) = delete;
  Oid(Oid&&) noexcept = default;
  Oid& operator=(const Oid&) = delete;
  Oid& operator=(Oid&&) noexcept = default;

  /// Test if this OID is all zeros.
  bool isZero() const;

  /// Format a git_oid into a buffer as a hex string.
  std::string toString() const;
};

std::ostream& operator<<(std::ostream& os, const Oid& oid);
bool operator==(const Oid& lhs, const Oid& rhs);

}  // end namespace git2
