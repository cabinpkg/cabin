// Exercises Boost.Multiprecision: cpp_int arithmetic, modular exponentiation and a
// Miller-Rabin test driven by Boost.Random, decimal and binary floating point, and a
// Boost.Math special function evaluated at 50 digits.
#include <boost/math/constants/constants.hpp>
#include <boost/math/special_functions/gamma.hpp>
#include <boost/multiprecision/cpp_bin_float.hpp>
#include <boost/multiprecision/cpp_dec_float.hpp>
#include <boost/multiprecision/cpp_int.hpp>
#include <boost/multiprecision/miller_rabin.hpp>
#include <boost/random/mersenne_twister.hpp>

int main() {
    using namespace boost::multiprecision;
    cpp_int f = 1;
    for (int i = 2; i <= 30; ++i) f *= i;
    if (f.str() != "265252859812191058636308480000000") return 1;
    if ((cpp_int(1) << 128) - 1 != cpp_int("340282366920938463463374607431768211455")) return 1;
    if (cpp_int(-7) / 2 != -3 || cpp_int(-7) % 2 != -1) return 1;
    if (powm(cpp_int(2), 100, cpp_int(1000000007)) != 976371285) return 1;
    if (gcd(f, cpp_int("1000000000000")) != 320000000) return 1;

    boost::random::mt19937 gen(42);
    cpp_int mersenne = (cpp_int(1) << 61) - 1;
    if (!miller_rabin_test(mersenne, 25, gen) || miller_rabin_test(mersenne * 3, 25, gen))
        return 1;

    cpp_dec_float_50 root2 = sqrt(cpp_dec_float_50(2));
    if (root2.str(50) != "1.4142135623730950488016887242096980785696718753769") return 1;

    cpp_bin_float_50 half("0.5");
    cpp_bin_float_50 gamma_half = boost::math::tgamma(half);
    cpp_bin_float_50 expected = sqrt(boost::math::constants::pi<cpp_bin_float_50>());
    if (abs(gamma_half - expected) > cpp_bin_float_50("1e-45")) return 1;
    return 0;
}
