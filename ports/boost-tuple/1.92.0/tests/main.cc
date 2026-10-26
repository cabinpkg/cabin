// Boost.Tuple: construct and access tuples, compare them, unpack with tie/ignore, and stream them.
#include <boost/tuple/tuple.hpp>
#include <boost/tuple/tuple_comparison.hpp>
#include <boost/tuple/tuple_io.hpp>
#include <sstream>
#include <string>

int main() {
    boost::tuple<int, std::string, double> t(1, "two", 3.5);
    if (boost::get<0>(t) != 1 || t.get<1>() != "two" || boost::get<2>(t) != 3.5) return 1;
    if (boost::tuples::length<boost::tuple<int, std::string, double> >::value != 3) return 1;

    int a = 0;
    double c = 0;
    boost::tie(a, boost::tuples::ignore, c) = t;
    if (a != 1 || c != 3.5) return 1;

    boost::get<0>(t) = 7;
    if (t.get<0>() != 7) return 1;

    if (!(boost::make_tuple(1, 2) < boost::make_tuple(1, 3))) return 1;
    if (boost::make_tuple(1, 'x') != boost::make_tuple(1, 'x')) return 1;

    std::ostringstream os;
    os << boost::make_tuple(1, 2, 3);
    if (os.str() != "(1 2 3)") return 1;
    std::ostringstream os2;
    os2 << boost::tuples::set_delimiter(',') << boost::make_tuple(4, 5);
    return os2.str() == "(4,5)" ? 0 : 1;
}
