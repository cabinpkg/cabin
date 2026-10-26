// Exercises Boost.StaticAssert: BOOST_STATIC_ASSERT and BOOST_STATIC_ASSERT_MSG at namespace,
// class, function, and dependent template scope.
#include <boost/static_assert.hpp>

BOOST_STATIC_ASSERT(sizeof(char) == 1);
BOOST_STATIC_ASSERT_MSG(sizeof(long) >= sizeof(int), "long must be at least as wide as int");

template <int N>
struct power_of_two {
    BOOST_STATIC_ASSERT_MSG(N >= 0 && N < 31, "exponent out of range");
    static const int value = 1 << N;
};

template <class T>
T twice(T x) {
    BOOST_STATIC_ASSERT(sizeof(T) >= sizeof(short));
    return x + x;
}

int main() {
    BOOST_STATIC_ASSERT(power_of_two<10>::value == 1024);
    if (power_of_two<4>::value != 16) return 1;
    if (twice(21) != 42 || twice(1.5) != 3.0) return 1;
    return 0;
}
