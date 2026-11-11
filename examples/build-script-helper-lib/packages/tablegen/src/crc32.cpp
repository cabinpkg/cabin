#include <tablegen/crc32.hpp>

namespace tablegen {

std::array<std::uint32_t, 256> crc32_table() {
    std::array<std::uint32_t, 256> table{};
    for (std::uint32_t index = 0; index < table.size(); ++index) {
        std::uint32_t entry = index;
        for (int bit = 0; bit < 8; ++bit) {
            entry = (entry >> 1) ^ ((entry & 1U) != 0U ? 0xEDB88320U : 0U);
        }
        table[index] = entry;
    }
    return table;
}

} // namespace tablegen
