// Exercises Boost.Ratio: compile-time rational arithmetic, normalization, comparison,
// and the SI prefix typedefs.
#include <boost/ratio.hpp>

typedef boost::ratio_add<boost::ratio<1, 3>, boost::ratio<1, 6> >::type half;
typedef boost::ratio_multiply<boost::kilo, boost::milli>::type one;
typedef boost::ratio_divide<boost::mega, boost::kilo>::type thousand;
typedef boost::ratio_subtract<boost::ratio<1, 4>, boost::ratio<3, 4> >::type minus_half;

static_assert(half::num == 1 && half::den == 2, "1/3 + 1/6 == 1/2");
static_assert(one::num == 1 && one::den == 1, "kilo * milli == 1");
static_assert(boost::ratio<6, -8>::num == -3 && boost::ratio<6, -8>::den == 4, "normalized");
static_assert(boost::ratio_less<boost::ratio<1, 3>, half>::value, "1/3 < 1/2");
static_assert(boost::ratio_equal<boost::ratio<2, 4>, half>::value, "2/4 == 1/2");

int main() {
    long long n = thousand::num, d = thousand::den;
    if (n != 1000 || d != 1) return 1;
    if (minus_half::num != -1 || minus_half::den != 2) return 1;
    if (boost::nano::den != 1000000000LL) return 1;
    return 0;
}
