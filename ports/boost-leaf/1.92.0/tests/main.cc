// Exercises Boost.LEAF result<T> error propagation with BOOST_LEAF_AUTO and
// error-object dispatch to handlers in try_handle_all.
#include <boost/leaf.hpp>

namespace leaf = boost::leaf;

struct e_negative { int value; };

leaf::result<int> doubled(int x) {
    if (x < 0) return leaf::new_error(e_negative{x});
    return x * 2;
}

int run(int x) {
    return leaf::try_handle_all(
        [x]() -> leaf::result<int> {
            BOOST_LEAF_AUTO(v, doubled(x));
            return v + 1;
        },
        [](const e_negative& e) { return -1000 + e.value; },
        []() { return -1; });
}

int main() {
    if (run(4) != 9) return 1;
    if (run(-5) != -1005) return 1;
    leaf::result<int> r = doubled(3);
    if (!r || r.value() != 6) return 1;
    return 0;
}
