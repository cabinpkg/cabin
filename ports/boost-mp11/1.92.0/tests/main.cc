// Exercises Boost.Mp11 type-list algorithms at compile time and mp_for_each /
// mp_with_index dispatch at runtime.
#include <boost/mp11.hpp>
#include <cstddef>
#include <type_traits>

using namespace boost::mp11;

template <class T> using add_ptr = T*;

typedef mp_list<char, int, double> L;
static_assert(mp_size<L>::value == 3, "size");
static_assert(std::is_same<mp_transform<add_ptr, L>, mp_list<char*, int*, double*> >::value, "transform");
static_assert(mp_find<L, double>::value == 2, "find");
static_assert(std::is_same<mp_reverse<L>, mp_list<double, int, char> >::value, "reverse");
static_assert(mp_count_if<L, std::is_integral>::value == 2, "count_if");

struct sum_sizes {
    std::size_t* total;
    template <class T> void operator()(T) const { *total += sizeof(T); }
};
struct index_value {
    template <class I> std::size_t operator()(I) const { return I::value * 10; }
};

int main() {
    std::size_t total = 0;
    mp_for_each<L>(sum_sizes{&total});
    if (total != sizeof(char) + sizeof(int) + sizeof(double)) return 1;
    if (mp_with_index<3>(2, index_value()) != 20) return 1;
    return 0;
}
