// Exercises Boost.Phoenix lazy expressions: placeholders, operators, ref/val, and
// lazy statements used with standard algorithms.
#include <boost/phoenix.hpp>
#include <algorithm>
#include <vector>

int main() {
    using boost::phoenix::arg_names::arg1;
    using boost::phoenix::arg_names::arg2;
    using boost::phoenix::ref;
    using boost::phoenix::val;

    if ((arg1 * 2 + arg2)(3, 4) != 10) return 1;
    if (val(7)() != 7) return 1;

    std::vector<int> v;
    for (int i = 1; i <= 6; ++i) v.push_back(i);

    std::vector<int>::iterator it = std::find_if(v.begin(), v.end(), arg1 > 3);
    if (it == v.end() || *it != 4) return 1;

    int sum = 0;
    std::for_each(v.begin(), v.end(), ref(sum) += arg1);
    if (sum != 21) return 1;

    int evens = 0;
    std::for_each(v.begin(), v.end(), boost::phoenix::if_(arg1 % 2 == 0)[++ref(evens)]);
    if (evens != 3) return 1;
    return 0;
}
