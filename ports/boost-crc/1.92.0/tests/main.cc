// Computes standard CRC-32 and CRC-16/CCITT-FALSE check values with Boost.CRC,
// both incrementally and with the one-shot boost::crc function.
#include <boost/crc.hpp>
#include <cstring>

int main() {
    const char* data = "123456789";
    const std::size_t n = std::strlen(data);

    boost::crc_32_type crc32;
    crc32.process_bytes(data, n);
    if (crc32.checksum() != 0xCBF43926u) return 1;

    // Feeding the same bytes in two pieces must give the same result.
    boost::crc_32_type split;
    split.process_bytes(data, 4);
    split.process_bytes(data + 4, n - 4);
    if (split.checksum() != crc32.checksum()) return 1;

    boost::crc_optimal<16, 0x1021, 0xFFFF, 0, false, false> ccitt;
    ccitt.process_bytes(data, n);
    if (ccitt.checksum() != 0x29B1) return 1;

    if (boost::crc<16, 0x1021, 0xFFFF, 0, false, false>(data, n) != 0x29B1) return 1;
    return 0;
}
