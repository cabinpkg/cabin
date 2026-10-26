// Exercises boost::numeric_cast range checking (in-range, positive/negative overflow) and
// boost::numeric::bounds.
#include <boost/numeric/conversion/cast.hpp>
#include <boost/numeric/conversion/bounds.hpp>

int main() {
    if (boost::numeric_cast<short>(1234L) != 1234) return 1;
    if (boost::numeric_cast<int>(42.0) != 42) return 1;

    bool caught = false;
    try {
        (void)boost::numeric_cast<unsigned char>(300);
    } catch (const boost::numeric::positive_overflow&) {
        caught = true;
    }
    if (!caught) return 1;

    caught = false;
    try {
        (void)boost::numeric_cast<unsigned int>(-1);
    } catch (const boost::numeric::bad_numeric_cast&) {
        caught = true;
    }
    if (!caught) return 1;

    if (boost::numeric::bounds<signed char>::lowest() != -128) return 1;
    if (boost::numeric::bounds<double>::lowest() >= 0.0) return 1;
    return 0;
}
