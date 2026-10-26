// Exercises Boost.Endian byte-order conversion functions and the
// fixed-byte-order buffer types.
#include <boost/endian.hpp>
#include <cstdint>

int main() {
    namespace be = boost::endian;

    if (be::endian_reverse(std::uint32_t(0x01020304u)) != 0x04030201u) return 1;
    if (be::endian_reverse(std::uint16_t(0xABCD)) != 0xCDAB) return 1;

    std::uint32_t v = 0xDEADBEEFu;
    if (be::big_to_native(be::native_to_big(v)) != v) return 1;
    if (be::little_to_native(be::native_to_little(v)) != v) return 1;

    // Buffer types store bytes in a fixed order regardless of the host.
    be::big_uint32_buf_t big(0x01020304u);
    be::little_uint32_buf_t little(0x01020304u);
    const unsigned char* bb = big.data();
    const unsigned char* lb = little.data();
    if (bb[0] != 0x01 || bb[3] != 0x04) return 1;
    if (lb[0] != 0x04 || lb[3] != 0x01) return 1;
    if (big.value() != 0x01020304u || little.value() != 0x01020304u) return 1;

    unsigned char raw[4];
    be::store_big_u32(raw, 0xCAFEBABEu);
    if (raw[0] != 0xCA || be::load_little_u32(raw) != 0xBEBAFECAu) return 1;
    return 0;
}
