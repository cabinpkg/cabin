// Exercises Boost.Core reference wrappers, addressof, invoke_swap, noncopyable,
// and the bit-manipulation helpers.
#include <boost/core/addressof.hpp>
#include <boost/core/bit.hpp>
#include <boost/core/invoke_swap.hpp>
#include <boost/core/noncopyable.hpp>
#include <boost/core/ref.hpp>

struct Counter : private boost::noncopyable {
    int value;
    Counter() : value(0) {}
    // An overloaded unary & must not fool boost::addressof.
    Counter* operator&() { return 0; }
};

static void bump(Counter& c) { ++c.value; }

int main() {
    Counter c;
    boost::reference_wrapper<Counter> r = boost::ref(c);
    bump(r);
    bump(r.get());
    if (c.value != 2) return 1;
    if (boost::addressof(c) == 0 || boost::addressof(r.get()) != r.get_pointer()) return 1;

    int a = 1, b = 2;
    boost::core::invoke_swap(a, b);
    if (a != 2 || b != 1) return 1;

    if (boost::core::popcount(0xF0u) != 4) return 1;
    if (boost::core::bit_width(8u) != 4) return 1;
    if (boost::core::bit_ceil(5u) != 8u) return 1;
    if (!boost::core::has_single_bit(64u)) return 1;
    return 0;
}
