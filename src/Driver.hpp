#pragma once

#include <rs/result.hpp>

namespace cabin {

// NOLINTNEXTLINE(*-avoid-c-arrays)
rs::Result<void, void> run(int argc, char* argv[]) noexcept;

} // namespace cabin
