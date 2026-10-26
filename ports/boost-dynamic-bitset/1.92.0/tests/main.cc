// Exercises boost::dynamic_bitset construction, bit queries, resizing, set
// operations, and string conversion.
#include <boost/dynamic_bitset.hpp>
#include <string>

int main() {
    boost::dynamic_bitset<> bits(8, 0xA5ul);
    if (bits.size() != 8 || bits.count() != 4) return 1;
    if (!bits.test(0) || bits.test(1) || !bits[7]) return 1;

    std::string s;
    boost::to_string(bits, s);
    if (s != "10100101") return 1;

    bits.resize(12);
    bits.set(11);
    if (bits.size() != 12 || bits.count() != 5) return 1;
    if (bits.find_first() != 0 || bits.find_next(0) != 2) return 1;

    boost::dynamic_bitset<> mask(std::string("000011110000"));
    boost::dynamic_bitset<> both = bits & mask;
    if (both.count() != 2 || both.to_ulong() != 0xA0ul) return 1;
    if (!bits.intersects(mask) || (~bits).count() != 7) return 1;
    return 0;
}
