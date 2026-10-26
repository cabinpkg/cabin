// Exercises BOOST_LOCAL_FUNCTION binding locals by value and reference, with a
// default parameter, called directly and passed to a standard algorithm.
#include <boost/local_function.hpp>
#include <algorithm>

int main() {
    int sum = 0;
    const int factor = 10;
    void BOOST_LOCAL_FUNCTION(const bind factor, bind& sum, int num) {
        sum += factor * num;
    } BOOST_LOCAL_FUNCTION_NAME(add)

    add(1);
    int nums[] = {2, 3};
    std::for_each(nums, nums + 2, add);
    if (sum != 60) return 1;

    int BOOST_LOCAL_FUNCTION(const bind factor, int x, default 5) {
        return factor * x;
    } BOOST_LOCAL_FUNCTION_NAME(scale)
    if (scale(3) != 30 || scale() != 50) return 1;
    return 0;
}
