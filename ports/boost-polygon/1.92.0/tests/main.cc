// Exercises Boost.Polygon: rectangle and polygon concepts (area, containment, perimeter)
// and boolean operations on polygon sets.
#include <boost/polygon/polygon.hpp>
#include <vector>

namespace gtl = boost::polygon;

int main() {
    typedef gtl::point_data<int> point;
    gtl::rectangle_data<int> r(0, 0, 10, 5);
    if (gtl::area(r) != 50 || gtl::perimeter(r) != 30) return 1;
    if (!gtl::contains(r, point(3, 3)) || gtl::contains(r, point(11, 3))) return 1;

    std::vector<point> pts;
    pts.push_back(point(0, 0));
    pts.push_back(point(4, 0));
    pts.push_back(point(4, 4));
    pts.push_back(point(0, 4));
    gtl::polygon_data<int> square;
    gtl::set_points(square, pts.begin(), pts.end());
    if (gtl::area(square) != 16) return 1;
    if (!gtl::contains(square, point(2, 2))) return 1;

    using namespace gtl::operators;
    gtl::polygon_set_data<int> a, b;
    a += r;
    b += square;
    gtl::polygon_set_data<int> both = a & b;
    gtl::polygon_set_data<int> either = a | b;
    if (gtl::area(both) != 16 || gtl::area(either) != 50) return 1;
    gtl::polygon_set_data<int> overhang;
    overhang += gtl::rectangle_data<int>(8, 0, 12, 4);
    if (gtl::area(a & overhang) != 8 || gtl::area(overhang - a) != 8) return 1;
    return 0;
}
