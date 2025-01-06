#pragma once

#include <git2/types.h>
#include <string>

namespace git2 {

struct Time {
  git_time_t time;

  std::string toString() const;
};

}  // namespace git2
