// Exercises Boost.Hana heterogeneous tuples, type-level computation, and
// algorithms checked both at compile time and at runtime.
#include <boost/hana.hpp>
#include <string>

namespace hana = boost::hana;

int main() {
    auto t = hana::make_tuple(1, 2.5, std::string("three"));
    BOOST_HANA_CONSTANT_CHECK(hana::length(t) == hana::size_c<3>);
    if (hana::at_c<2>(t) != "three") return 1;

    constexpr auto types = hana::tuple_t<int, char, double>;
    static_assert(hana::contains(types, hana::type_c<char>), "char present");
    static_assert(!hana::contains(types, hana::type_c<float>), "float absent");

    constexpr auto nums = hana::make_tuple(1, 2, 3, 4);
    constexpr int sum = hana::fold_left(nums, 0, hana::plus);
    static_assert(sum == 10, "compile-time fold");

    auto doubled = hana::transform(nums, [](int x) { return x * 2; });
    if (hana::at_c<3>(doubled) != 8) return 1;

    auto evens = hana::filter(hana::tuple_c<int, 1, 2, 3, 4, 5, 6>,
                              [](auto x) { return x % hana::int_c<2> == hana::int_c<0>; });
    BOOST_HANA_CONSTANT_CHECK(hana::length(evens) == hana::size_c<3>);
    if (hana::fold_left(evens, 0, hana::plus) != 12) return 1;
    return 0;
}
