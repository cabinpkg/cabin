// Exercises Boost.Rational: normalized construction, arithmetic, comparison, rational_cast,
// and the bad_rational exception on a zero denominator.
#include <boost/rational.hpp>

int main() {
    typedef boost::rational<int> q;
    q a(1, 3), b(1, 6);
    q s = a + b;
    if (s.numerator() != 1 || s.denominator() != 2) return 1;

    q c(6, -8);
    if (c.numerator() != -3 || c.denominator() != 4) return 1;
    if (abs(c) != q(3, 4)) return 1;
    if (a * b != q(1, 18) || a / b != q(2)) return 1;
    if (!(a > b) || !(c < 0)) return 1;
    if (boost::rational_cast<double>(s) != 0.5) return 1;

    try {
        q z(1, 0);
        (void)z;
        return 1;
    } catch (const boost::bad_rational&) {
    }
    return 0;
}
