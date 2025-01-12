#pragma once

#include "Rustify/Result.hpp"

#include <span>
#include <string>

namespace cabin {

Result<void, std::string> cliMain(std::span<char* const> args) noexcept;

}  // namespace cabin
