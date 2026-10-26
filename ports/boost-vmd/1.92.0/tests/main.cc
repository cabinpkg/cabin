// Boost.VMD: identify emptiness, tuples, seqs, numbers, and registered identifiers in macro
// arguments, and compare, index, and count elements of a VMD sequence.
#include <boost/vmd/vmd.hpp>

#define BOOST_VMD_REGISTER_red (red)
#define BOOST_VMD_DETECT_red_red

int main() {
    if (!(BOOST_VMD_IS_EMPTY() == 1)) return 1;
    if (!(BOOST_VMD_IS_EMPTY(x) == 0)) return 1;
    if (!(BOOST_VMD_IS_TUPLE((1, 2, 3)) == 1)) return 1;
    if (!(BOOST_VMD_IS_SEQ((a)(b)) == 1)) return 1;
    if (!(BOOST_VMD_IS_NUMBER(42) == 1)) return 1;
    if (!(BOOST_VMD_IS_NUMBER(red) == 0)) return 1;
    if (!(BOOST_VMD_IS_IDENTIFIER(red) == 1)) return 1;
    if (!(BOOST_VMD_EQUAL((1, 2), (1, 2)) == 1)) return 1;
    if (!(BOOST_VMD_EQUAL((1, 2), (1, 3)) == 0)) return 1;
    if (!(BOOST_VMD_IS_TUPLE(BOOST_VMD_ELEM(1, 10 (a, b) 30)) == 1)) return 1;
    int values[] = { BOOST_VMD_ELEM(2, 10 (a, b) 30), BOOST_VMD_SIZE(10 (a, b) red (x)(y)) };
    if (values[0] != 30 || values[1] != 4) return 1;
    return BOOST_VMD_SIZE((4, 5, 6)) == 1 ? 0 : 1;
}
