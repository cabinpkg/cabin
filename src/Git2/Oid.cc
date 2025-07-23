#include "Oid.hpp"

#include "Exception.hpp"

#include <git2/deprecated.h>
#include <git2/oid.h>
#include <ostream>
#include <string>
#include <string_view>

namespace git2 {

Oid::Oid(git_oid oid) : oid(oid), raw(&this->oid) {}

Oid::Oid(git_oid* oid) : oid(*oid), raw(oid) {}

// NOLINTNEXTLINE(cppcoreguidelines-pro-type-const-cast)
Oid::Oid(const git_oid* oid) : Oid(const_cast<git_oid*>(oid)) {}

Oid::Oid(const std::string_view str) {
  git2Throw(git_oid_fromstrn(raw, str.data(), str.size()));
}

bool Oid::isZero() const { return git_oid_iszero(raw) == 1; }

std::string Oid::toString() const {
  std::string buf(GIT_OID_MAX_HEXSIZE, '\0');
  git_oid_tostr(buf.data(), buf.size() + 1, raw);
  return buf;
}

std::ostream& operator<<(std::ostream& os, const Oid& oid) {
  return (os << git_oid_tostr_s(oid.raw));
}

inline bool operator==(const Oid& lhs, const Oid& rhs) {
  return git_oid_equal(lhs.raw, rhs.raw) != 0;
}

}  // end namespace git2
