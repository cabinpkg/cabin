#pragma once

#include "Rustify/Result.hpp"

#include <span>

namespace cabin {

Result<void> cliMain(std::span<char* const> args) noexcept;

}  // namespace cabin
