#pragma once

#include <array>
#include <cstdint>

namespace tablegen {

// The 256-entry lookup table of the CRC-32 that zlib and PNG use.
std::array<std::uint32_t, 256> crc32_table();

} // namespace tablegen
