// Exercises Boost.Lambda2 placeholder expressions as function objects and as
// predicates/transforms for standard algorithms.
#include <boost/lambda2.hpp>
#include <algorithm>
#include <vector>

int main() {
    using namespace boost::lambda2;
    std::vector<int> v{1, 2, 3, 4, 5};

    if ((_1 + _2 * 2)(1, 3) != 7) return 1;
    if (std::count_if(v.begin(), v.end(), _1 > 3) != 2) return 1;

    std::vector<int> out(v.size());
    std::transform(v.begin(), v.end(), out.begin(), _1 * _1);
    if (out[4] != 25) return 1;

    auto in_range = _1 >= 2 && _1 <= 4;
    if (std::count_if(v.begin(), v.end(), in_range) != 3) return 1;
    return 0;
}
