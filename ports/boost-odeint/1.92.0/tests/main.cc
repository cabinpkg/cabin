// Exercises Boost.Odeint's steppers on ODEs with closed-form solutions:
// fixed-step Runge-Kutta 4 on exponential decay, step-size-controlled
// Dormand-Prince on a harmonic oscillator (checked at every observed
// step), and the implicit Rosenbrock method (uBLAS) on a stiff decay.
#include <boost/numeric/odeint/integrate/integrate_adaptive.hpp>
#include <boost/numeric/odeint/integrate/integrate_const.hpp>
#include <boost/numeric/odeint/stepper/generation.hpp>
#include <boost/numeric/odeint/stepper/rosenbrock4.hpp>
#include <boost/numeric/odeint/stepper/rosenbrock4_controller.hpp>
#include <boost/numeric/odeint/stepper/runge_kutta4.hpp>
#include <boost/numeric/odeint/stepper/runge_kutta_dopri5.hpp>
#include <algorithm>
#include <cmath>
#include <utility>
#include <vector>

namespace odeint = boost::numeric::odeint;
typedef std::vector<double> State;

int main() {
    // x' = -x, x(0) = 1, so x(1) = 1/e.
    State x(1, 1.0);
    const std::size_t steps = odeint::integrate_const(
        odeint::runge_kutta4<State>(), [](const State& s, State& d, double) { d[0] = -s[0]; }, x, 0.0, 1.0, 0.01);
    if (steps != 100 || std::abs(x[0] - std::exp(-1.0)) > 1e-9) return 1;

    // x'' = -x, x(0) = 1, x'(0) = 0, so x(t) = cos t and x'(t) = -sin t.
    State y(2);
    y[0] = 1;
    y[1] = 0;
    double max_err = 0;
    int observed = 0;
    odeint::integrate_adaptive(
        odeint::make_controlled<odeint::runge_kutta_dopri5<State> >(1e-10, 1e-10),
        [](const State& s, State& d, double) {
            d[0] = s[1];
            d[1] = -s[0];
        },
        y, 0.0, 2.0, 0.1,
        [&](const State& s, double t) {
            ++observed;
            max_err = std::max(max_err, std::abs(s[0] - std::cos(t)));
        });
    if (observed < 10 || max_err > 1e-8 || std::abs(y[1] + std::sin(2.0)) > 1e-8) return 2;

    // Stiff x' = -50 x, x(0) = 1, so x(0.2) = e^-10.
    typedef boost::numeric::ublas::vector<double> Vec;
    typedef boost::numeric::ublas::matrix<double> Mat;
    Vec z(1);
    z(0) = 1;
    odeint::integrate_adaptive(odeint::rosenbrock4_controller<odeint::rosenbrock4<double> >(1e-12, 1e-10),
                               std::make_pair([](const Vec& s, Vec& d, double) { d(0) = -50 * s(0); },
                                              [](const Vec&, Mat& j, double, Vec& dfdt) {
                                                  j(0, 0) = -50;
                                                  dfdt(0) = 0;
                                              }),
                               z, 0.0, 0.2, 0.001);
    return std::abs(z(0) / std::exp(-10.0) - 1) < 1e-6 ? 0 : 3;
}
