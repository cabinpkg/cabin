#include "Version.hpp"

#include "Exception.hpp"

#include <git2/common.h>

namespace git2 {

Version::Version() : features(git2Throw(git_libgit2_features())) {
  git2Throw(git_libgit2_version(&this->major, &this->minor, &this->rev));
}

bool
Version::hasThread() const noexcept {
  return this->features & GIT_FEATURE_THREADS;
}

bool
Version::hasHttps() const noexcept {
  return this->features & GIT_FEATURE_HTTPS;
}

bool
Version::hasSsh() const noexcept {
  return this->features & GIT_FEATURE_SSH;
}

bool
Version::hasNsec() const noexcept {
  return this->features & GIT_FEATURE_NSEC;
}

std::ostream&
operator<<(std::ostream& os, const Version& version) {
  const auto flagStr = [](const bool flag) { return flag ? "on" : "off"; };
  return os << version.major << '.' << version.minor << '.' << version.rev
            << " (threads: " << flagStr(version.hasThread()) << ", "
            << "https: " << flagStr(version.hasHttps()) << ", "
            << "ssh: " << flagStr(version.hasSsh()) << ", "
            << "nsec: " << flagStr(version.hasNsec()) << ')';
}

}  // namespace git2
