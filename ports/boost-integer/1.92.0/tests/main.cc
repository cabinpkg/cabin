// Exercises Boost.Integer type selection, integer_traits, compile-time log2 and
// masks, and the runtime gcd/lcm algorithms.
#include <boost/integer.hpp>
#include <boost/integer_traits.hpp>
#include <boost/integer/common_factor.hpp>
#include <boost/integer/integer_mask.hpp>
#include <boost/integer/static_log2.hpp>
#include <boost/static_assert.hpp>

BOOST_STATIC_ASSERT(sizeof(boost::int_t<16>::least) * 8 >= 16);
BOOST_STATIC_ASSERT(sizeof(boost::uint_t<32>::fast) * 8 >= 32);
BOOST_STATIC_ASSERT(boost::static_log2<1024>::value == 10);
BOOST_STATIC_ASSERT(boost::low_bits_mask_t<4>::sig_bits == 0xF);
BOOST_STATIC_ASSERT(boost::integer_traits<unsigned char>::const_max == 255);

int main() {
    boost::uint_value_t<300>::least v = 300;
    if (v != 300) return 1;
    if (boost::integer::gcd(84, 36) != 12) return 1;
    if (boost::integer::lcm(4, 6) != 12) return 1;
    if (boost::integer::static_gcd<12, 18>::value != 6) return 1;
    if (boost::high_bit_mask_t<7>::high_bit != 0x80) return 1;
    return 0;
}
