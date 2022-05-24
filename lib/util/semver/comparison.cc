// internal
#include "poac/util/semver/comparison.hpp"

namespace semver {
namespace detail {

  bool
  gt_pre(const Version& lhs, const Version& rhs) {
    // NOT having a prerelease is > having one
    if (lhs.pre.empty() && !rhs.pre.empty()) { // gt
      return true;
    }

    const int lhs_pre_size = static_cast<int>(lhs.pre.size());
    const int rhs_pre_size = static_cast<int>(rhs.pre.size());
    const int max_size = std::max(lhs_pre_size, rhs_pre_size);
    for (int i = 0; i <= max_size; ++i) {
      if (i >= lhs_pre_size && i >= rhs_pre_size) {
        return false; // eq
      } else if (i >= lhs_pre_size) {
        return false; // lt
      } else if (i >= rhs_pre_size) {
        return true; // gt
      } else if (lhs.pre[i] == rhs.pre[i]) {
        continue;
      } else if (!lhs.pre[i].is_numeric() && rhs.pre[i].is_numeric()) {
        return true;
      } else if (lhs.pre[i].is_alpha_numeric() && rhs.pre[i].is_alpha_numeric()) {
        if (lhs.pre[i].get_alpha_numeric() > rhs.pre[i].get_alpha_numeric()) {
          return true;
        }
      } else if (lhs.pre[i].is_numeric() && rhs.pre[i].is_numeric()) {
        if (lhs.pre[i].get_numeric() > rhs.pre[i].get_numeric()) {
          return true;
        }
      } else {
        return false;
      }
    }
    return false;
  }
  bool
  eq_pre(const Version& lhs, const Version& rhs) {
    if (lhs.pre.empty() && rhs.pre.empty()) { // eq
      return true;
    }

    const int lhs_pre_size = static_cast<int>(lhs.pre.size());
    const int rhs_pre_size = static_cast<int>(rhs.pre.size());
    const int max_size = std::max(lhs_pre_size, rhs_pre_size);
    for (int i = 0; i <= max_size; ++i) {
      if (i >= lhs_pre_size && i >= rhs_pre_size) {
        return true; // eq
      } else if (i >= rhs_pre_size) {
        return false; // gt
      } else if (i >= lhs_pre_size) {
        return false; // lt
      } else if (lhs.pre[i] == rhs.pre[i]) {
        continue;
      } else {
        return false;
      }
    }
    return true;
  }

} // end namespace detail

bool
operator>(const Version& lhs, const Version& rhs) { // gt
  if (lhs.major != rhs.major) {
    return lhs.major > rhs.major;
  } else if (lhs.minor != rhs.minor) {
    return lhs.minor > rhs.minor;
  } else if (lhs.patch != rhs.patch) {
    return lhs.patch > rhs.patch;
  } else {
    return detail::gt_pre(lhs, rhs);
  }
}

} // end namespace semver
