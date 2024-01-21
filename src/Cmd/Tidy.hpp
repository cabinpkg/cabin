#pragma once

#include "../Rustify.hpp"

#include <span>

// NOLINTNEXTLINE(readability-identifier-naming)
static inline constexpr StringRef tidyDesc = "Run clang-tidy";

void tidyHelp() noexcept;
int tidyMain(std::span<const StringRef> args);
