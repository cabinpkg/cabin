#pragma once

#include "../Cli.hpp"

#include <string>
#include <string_view>

extern const Subcmd NEW_CMD;
std::string createCabinToml(std::string_view projectName) noexcept;
