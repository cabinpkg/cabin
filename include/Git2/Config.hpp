#pragma once

#include "Global.hpp"

#include <git2/config.h>
#include <string>

namespace git2 {

struct Config : public GlobalState {
private:
  git_config* raw = nullptr;

public:
  Config();
  ~Config();

  explicit Config(git_config* raw);

  Config(const Config&) = delete;
  Config(Config&&) noexcept = default;
  Config& operator=(const Config&) = delete;
  Config& operator=(Config&&) noexcept = default;

  /// Open the global, XDG and system configuration files.
  Config& openDefault();

  /// Get the value of a string config variable as an owned string.
  std::string getString(const std::string& name);
};

} // end namespace git2
