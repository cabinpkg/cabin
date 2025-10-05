#pragma once

#include "Cli.hpp"

#include <string>
#include <string_view>

namespace cabin {

extern const Subcmd NEW_CMD;
std::string createCabinToml(std::string_view projectName,
                            bool useModules = false) noexcept;

} // namespace cabin
