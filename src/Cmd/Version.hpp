#pragma once

#include "../Cli.hpp"

#include <span>
#include <string_view>

namespace cabin {

extern const Subcmd VERSION_CMD;
Result<void> versionMain(std::span<const std::string_view> args) noexcept;

}  // namespace cabin
