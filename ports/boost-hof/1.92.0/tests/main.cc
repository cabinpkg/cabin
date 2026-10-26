// Exercises Boost.HOF function adaptors: compose, first_of overload selection,
// pipable, partial application, and fix-point recursion.
#include <boost/hof.hpp>

struct increment {
    template <class T> constexpr T operator()(T x) const { return x + 1; }
};
struct twice {
    template <class T> constexpr T operator()(T x) const { return x * 2; }
};
struct add {
    constexpr int operator()(int a, int b) const { return a + b; }
};
struct int_tag {
    constexpr int operator()(int) const { return 1; }
};
struct other_tag {
    template <class T> constexpr int operator()(T) const { return 2; }
};
struct factorial_step {
    template <class Self> int operator()(Self self, int n) const { return n <= 1 ? 1 : n * self(n - 1); }
};

int main() {
    namespace hof = boost::hof;
    if (hof::compose(increment(), twice())(3) != 7) return 1;

    auto pick = hof::first_of(int_tag(), other_tag());
    if (pick(5) != 1 || pick("x") != 2) return 1;

    auto piped_add = hof::pipable(add());
    if ((4 | piped_add(3)) != 7) return 1;

    if (hof::partial(add())(10)(5) != 15) return 1;

    if (hof::fix(factorial_step())(5) != 120) return 1;
    return 0;
}
