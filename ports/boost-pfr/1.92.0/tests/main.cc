// Exercises Boost.PFR reflection on a plain aggregate: field count, indexed get,
// for_each_field, and field-wise comparison.
#include <boost/pfr.hpp>

struct point {
    int x;
    double y;
    char tag;
};

struct summer {
    double* total;
    template <class T> void operator()(const T& field) const { *total += static_cast<double>(field); }
};

int main() {
    static_assert(boost::pfr::tuple_size<point>::value == 3, "point has three fields");

    point p{1, 2.5, 'a'};
    if (boost::pfr::get<0>(p) != 1 || boost::pfr::get<2>(p) != 'a') return 1;
    boost::pfr::get<1>(p) = 4.0;
    if (p.y != 4.0) return 1;

    double total = 0;
    boost::pfr::for_each_field(p, summer{&total});
    if (total != 1 + 4.0 + 'a') return 1;

    point q{1, 4.0, 'b'};
    if (boost::pfr::eq(p, q) || !boost::pfr::lt(p, q)) return 1;
    q.tag = 'a';
    if (!boost::pfr::eq(p, q)) return 1;
    return 0;
}
