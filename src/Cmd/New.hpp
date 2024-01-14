#pragma once

#include "../Rustify.hpp"

#include <span>

// NOLINTNEXTLINE(readability-identifier-naming)
static inline constexpr StringRef newDesc = "Create a new poac project";

String createPoacToml(const StringRef);
bool verifyPackageName(const StringRef) noexcept;

int newMain(const std::span<const StringRef>);
void newHelp() noexcept;
