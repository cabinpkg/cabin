// Boost.Typeof: BOOST_TYPEOF/BOOST_AUTO deduce expression types (dropping cv/ref), nested typedefs
// in templates, and the registration macros still accept user types.
#include <boost/typeof/typeof.hpp>
#include <string>
#include <type_traits>

#include BOOST_TYPEOF_INCREMENT_REGISTRATION_GROUP()

struct Point { int x, y; };
template <class T, class U> struct Pair { T first; U second; };
BOOST_TYPEOF_REGISTER_TYPE(Point)
BOOST_TYPEOF_REGISTER_TEMPLATE(Pair, 2)

template <class T, class U>
struct product {
    BOOST_TYPEOF_NESTED_TYPEDEF_TPL(nested, T() * U())
    typedef typename nested::type type;
};

static_assert(std::is_same<BOOST_TYPEOF(1 + 2L), long>::value, "int + long is long");
static_assert(std::is_same<product<int, double>::type, double>::value, "int * double is double");

int main() {
    const std::string s = "boost";
    BOOST_TYPEOF(s) copy = s;
    static_assert(std::is_same<decltype(copy), std::string>::value, "cv dropped");
    copy += "!";

    Pair<Point, char> p = {{1, 2}, 'c'};
    BOOST_AUTO(q, p);
    static_assert(std::is_same<decltype(q), Pair<Point, char> >::value, "registered template");
    if (q.first.y != 2 || q.second != 'c') return 1;

    BOOST_AUTO(n, s.size() * 2);
    return (copy == "boost!" && n == 10 && s == "boost") ? 0 : 1;
}
