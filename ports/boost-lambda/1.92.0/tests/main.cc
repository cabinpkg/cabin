// Exercises Boost.Lambda placeholder expressions passed to standard algorithms,
// including variable capture with var().
#include <boost/lambda/lambda.hpp>
#include <algorithm>
#include <vector>

int main() {
    using namespace boost::lambda;
    std::vector<int> v;
    for (int i = 1; i <= 5; ++i) v.push_back(i);

    std::vector<int> out(v.size());
    std::transform(v.begin(), v.end(), out.begin(), _1 * 2 + 1);
    if (out[0] != 3 || out[4] != 11) return 1;

    if (std::count_if(v.begin(), v.end(), _1 > 3) != 2) return 1;

    int sum = 0;
    std::for_each(v.begin(), v.end(), var(sum) += _1);
    if (sum != 15) return 1;

    if ((_1 + _2)(v[1], v[2]) != 5) return 1;
    return 0;
}
