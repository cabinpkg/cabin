#pragma once

#include "../Cli.hpp"

namespace cabin {

extern const Subcmd VERSION_CMD;
Result<void> versionMain(CliArgsView args) noexcept;

}  // namespace cabin
