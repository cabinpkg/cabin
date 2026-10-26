// Exercises Boost.Range: irange, piped filtered/transformed adaptors, and range-based
// algorithms (sort, max_element, count_if, accumulate, push_back).
#include <boost/range/adaptors.hpp>
#include <boost/range/algorithm.hpp>
#include <boost/range/algorithm_ext.hpp>
#include <boost/range/irange.hpp>
#include <boost/range/numeric.hpp>
#include <iterator>
#include <vector>

struct is_even {
    bool operator()(int x) const { return x % 2 == 0; }
};
struct square {
    typedef int result_type;
    int operator()(int x) const { return x * x; }
};

int main() {
    std::vector<int> v;
    boost::push_back(v, boost::irange(10, 0, -1));
    if (boost::size(v) != 10 || v.front() != 10) return 1;

    boost::sort(v);
    if (v.front() != 1 || *boost::max_element(v) != 10) return 1;
    if (boost::count_if(v, is_even()) != 5) return 1;

    std::vector<int> sq;
    boost::copy(v | boost::adaptors::filtered(is_even()) | boost::adaptors::transformed(square()),
                std::back_inserter(sq));
    if (sq.size() != 5 || sq[0] != 4 || sq[4] != 100) return 1;
    if (boost::accumulate(sq, 0) != 220) return 1;

    std::vector<int> rev(v.size());
    boost::copy(v | boost::adaptors::reversed, rev.begin());
    if (rev.front() != 10 || rev.back() != 1) return 1;
    return 0;
}
