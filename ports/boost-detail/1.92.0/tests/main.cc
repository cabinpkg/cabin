// Exercises Boost.Detail utilities: boost::blank, the binary_search algorithms,
// and numeric_traits.
#include <boost/blank.hpp>
#include <boost/detail/binary_search.hpp>
#include <boost/detail/numeric_traits.hpp>

int main() {
    boost::blank x, y;
    if (!(x == y) || x != y || x < y) return 1;

    const int sorted[] = {1, 3, 3, 5, 8};
    const int* first = sorted;
    const int* last = sorted + 5;
    if (boost::detail::lower_bound(first, last, 3) != sorted + 1) return 1;
    if (boost::detail::upper_bound(first, last, 3) != sorted + 3) return 1;
    if (!boost::detail::binary_search(first, last, 5)) return 1;
    if (boost::detail::binary_search(first, last, 4)) return 1;

    if (boost::detail::numeric_distance(2, 7) != 5) return 1;
    boost::detail::numeric_traits<unsigned char>::difference_type d = -1;
    if (d >= 0) return 1;
    return 0;
}
