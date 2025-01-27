#pragma once

#include "Rustify/Result.hpp"

namespace cabin {

// NOLINTNEXTLINE(*-avoid-c-arrays)
Result<void, void> cabinMain(int argc, char* argv[]) noexcept;

}  // namespace cabin
