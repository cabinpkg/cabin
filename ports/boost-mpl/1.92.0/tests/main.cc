// Exercises Boost.MPL sequences and metafunctions at compile time, plus
// runtime iteration over a type sequence with mpl::for_each.
#include <boost/mpl/at.hpp>
#include <boost/mpl/equal.hpp>
#include <boost/mpl/fold.hpp>
#include <boost/mpl/for_each.hpp>
#include <boost/mpl/if.hpp>
#include <boost/mpl/int.hpp>
#include <boost/mpl/placeholders.hpp>
#include <boost/mpl/plus.hpp>
#include <boost/mpl/push_back.hpp>
#include <boost/mpl/size.hpp>
#include <boost/mpl/vector.hpp>
#include <boost/mpl/vector_c.hpp>
#include <boost/static_assert.hpp>

namespace mpl = boost::mpl;

typedef mpl::push_back<mpl::vector<char, short, int>, long>::type types;
BOOST_STATIC_ASSERT(mpl::size<types>::value == 4);
BOOST_STATIC_ASSERT((mpl::equal<types, mpl::vector<char, short, int, long> >::value));

typedef mpl::vector_c<int, 1, 2, 3, 4> numbers;
typedef mpl::fold<numbers, mpl::int_<0>, mpl::plus<mpl::_1, mpl::_2> >::type total;
BOOST_STATIC_ASSERT(total::value == 10);
BOOST_STATIC_ASSERT((mpl::at_c<numbers, 2>::type::value == 3));
BOOST_STATIC_ASSERT((mpl::if_c<true, mpl::int_<1>, mpl::int_<2> >::type::value == 1));

struct size_summer {
    explicit size_summer(int* t) : sum(t) {}
    template <class T> void operator()(T) const { *sum += static_cast<int>(sizeof(T)); }
    int* sum;
};

int main() {
    int sum = 0;
    mpl::for_each<types>(size_summer(&sum));
    if (sum != static_cast<int>(sizeof(char) + sizeof(short) + sizeof(int) + sizeof(long))) return 1;
    if (total::value != 10) return 1;
    return 0;
}
