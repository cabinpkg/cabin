// Exercises Boost.Units: SI quantities combined by dimensional analysis, a conversion
// from CGS, the cmath overloads (hypot through Boost.Math), pow, and stream output.
#include <boost/units/cmath.hpp>
#include <boost/units/io.hpp>
#include <boost/units/pow.hpp>
#include <boost/units/quantity.hpp>
#include <boost/units/systems/cgs.hpp>
#include <boost/units/systems/si.hpp>
#include <cmath>
#include <sstream>

int main() {
    using namespace boost::units;
    quantity<si::length> width = 3.0 * si::meter;
    quantity<si::length> height = 4.0 * si::meter;
    quantity<si::area> area = width * height;
    if (area.value() != 12) return 1;

    quantity<si::length> diagonal = hypot(width, height);
    if (std::fabs(diagonal.value() - 5) > 1e-12) return 1;
    quantity<si::velocity> speed = diagonal / (2.0 * si::second);
    if (std::fabs(speed.value() - 2.5) > 1e-12) return 1;

    quantity<si::area> square = pow<2>(width);
    quantity<si::length> side = sqrt(square);
    if (square.value() != 9 || std::fabs(side.value() - 3) > 1e-12) return 1;

    quantity<cgs::length> centimeters = 150.0 * cgs::centimeter;
    quantity<si::length> meters(centimeters);
    if (std::fabs(meters.value() - 1.5) > 1e-12) return 1;

    std::ostringstream out;
    out << area << ", " << speed;
    return out.str() == "12 m^2, 2.5 m s^-1" ? 0 : 1;
}
