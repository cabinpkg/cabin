#pragma once

#include <rs/result.hpp>

namespace cabin {

// NOLINTNEXTLINE(*-avoid-c-arrays)
Result<void, void> run(int argc, char* argv[]) noexcept;

} // namespace cabin
