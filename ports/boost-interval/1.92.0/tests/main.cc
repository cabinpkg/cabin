// Exercises boost::numeric::interval arithmetic with rounding control, bounds,
// containment, and interval comparison.
#include <boost/numeric/interval.hpp>

int main() {
    typedef boost::numeric::interval<double> I;
    I a(1.0, 2.0);
    I b(3.0, 5.0);
    I sum = a + b;
    if (sum.lower() != 4.0 || sum.upper() != 7.0) return 1;
    I prod = a * b;
    if (prod.lower() != 3.0 || prod.upper() != 10.0) return 1;
    if (boost::numeric::width(sum) != 3.0) return 1;
    if (boost::numeric::median(b) != 4.0) return 1;
    if (!boost::numeric::in(1.5, a) || boost::numeric::in(2.5, a)) return 1;
    if (!boost::numeric::subset(a, I(0.0, 3.0))) return 1;
    I third = I(1.0) / I(3.0);
    if (!(third.lower() <= 1.0 / 3.0 && 1.0 / 3.0 <= third.upper())) return 1;
    // 1/3 is inexact in binary, so outward rounding must split the bounds.
    if (!(third.lower() < third.upper())) return 1;
    if (!boost::numeric::interval_lib::cerlt(a, b)) return 1;
    return 0;
}
