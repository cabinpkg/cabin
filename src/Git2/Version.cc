#include "Version.hpp"

#include "Exception.hpp"

#include <fmt/format.h>
#include <git2/common.h>
#include <string>

namespace git2 {

Version::Version() : features(git2Throw(git_libgit2_features())) {
  git2Throw(git_libgit2_version(&this->major, &this->minor, &this->rev));
}

std::string
Version::toString() const {
  const auto flagStr = [](const bool flag) { return flag ? "on" : "off"; };
  return fmt::format(
      "{}.{}.{} (threads: {}, https: {}, ssh: {}, nsec: {})", major, minor, rev,
      flagStr(hasThread()), flagStr(hasHttps()), flagStr(hasSsh()),
      flagStr(hasNsec())
  );
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

}  // namespace git2

auto
fmt::formatter<git2::Version>::format(
    const git2::Version& v, format_context& ctx
) const -> format_context::iterator {
  return formatter<std::string>::format(v.toString(), ctx);
}
