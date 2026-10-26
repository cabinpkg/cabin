// Exercises Boost.SafeNumerics: safe<T> arithmetic matches native results and throws on
// signed overflow, unsigned wraparound, narrowing, and division by zero.
#include <boost/safe_numerics/safe_integer.hpp>
#include <cstdint>
#include <limits>

using boost::safe_numerics::safe;

template <class F>
bool throws(F f) {
    try {
        f();
    } catch (const std::exception&) {
        return true;
    }
    return false;
}

int main() {
    safe<int> x = 20;
    safe<int> y = x * 3 + 2;
    if (y != 62 || y / 4 != 15) return 1;

    if (!throws([] { safe<int> m = std::numeric_limits<int>::max(); return m + 1; })) return 1;
    if (!throws([] { safe<unsigned> z = 0u; return z - 1u; })) return 1;
    if (!throws([] { safe<std::int8_t> n = 0; n = 1000; return n; })) return 1;
    if (!throws([] { safe<int> d = 0; return 10 / d; })) return 1;
    return 0;
}
