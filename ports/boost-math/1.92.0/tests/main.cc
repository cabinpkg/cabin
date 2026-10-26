// Exercises Boost.Math: special functions, a statistical distribution, constants,
// root finding, quadrature, and the default policy's error reporting.
#include <boost/math/constants/constants.hpp>
#include <boost/math/distributions/normal.hpp>
#include <boost/math/quadrature/gauss_kronrod.hpp>
#include <boost/math/special_functions/bessel.hpp>
#include <boost/math/special_functions/erf.hpp>
#include <boost/math/special_functions/gamma.hpp>
#include <boost/math/tools/roots.hpp>
#include <cmath>
#include <cstdint>
#include <stdexcept>
#include <utility>

static bool approx(double a, double b) { return std::fabs(a - b) <= 1e-12 * (std::fabs(b) + 1); }

struct Cubic {
    double operator()(double x) const { return x * x * x - 2; }
};

static double square(double x) { return x * x; }

int main() {
    namespace bm = boost::math;
    if (!approx(bm::tgamma(5.0), 24) || !approx(bm::tgamma(0.5), std::sqrt(bm::constants::pi<double>())))
        return 1;
    if (!approx(bm::erf(0.5), 0.5204998778130465) || !approx(bm::erfc(0.5) + bm::erf(0.5), 1)) return 1;
    if (!approx(bm::cyl_bessel_j(0, 1.0), 0.7651976865579666)) return 1;

    bm::normal_distribution<double> normal(0, 1);
    if (!approx(bm::cdf(normal, 0.0), 0.5)) return 1;
    if (!approx(bm::quantile(normal, 0.975), 1.959963984540054)) return 1;

    std::uintmax_t iterations = 100;
    std::pair<double, double> root = bm::tools::toms748_solve(
        Cubic(), 1.0, 2.0, bm::tools::eps_tolerance<double>(50), iterations);
    if (std::fabs(root.first - std::cbrt(2.0)) > 1e-14 || iterations >= 100) return 1;

    double area = bm::quadrature::gauss_kronrod<double, 15>::integrate(square, 0.0, 3.0);
    if (!approx(area, 9)) return 1;

    try {
        (void)bm::tgamma(-1.0);
        return 1;
    } catch (const std::domain_error&) {
    }
    return 0;
}
