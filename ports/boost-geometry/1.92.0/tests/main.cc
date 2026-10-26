// Exercises Boost.Geometry's core algorithms on simple Cartesian shapes
// with exact results: WKT input, polygon area, perimeter and centroid,
// point-point and point-polygon distances, within and intersects, the
// crossing point of two segments, and polygon intersection and union.
#include <boost/geometry/algorithms/area.hpp>
#include <boost/geometry/algorithms/centroid.hpp>
#include <boost/geometry/algorithms/distance.hpp>
#include <boost/geometry/algorithms/equals.hpp>
#include <boost/geometry/algorithms/intersection.hpp>
#include <boost/geometry/algorithms/intersects.hpp>
#include <boost/geometry/algorithms/perimeter.hpp>
#include <boost/geometry/algorithms/union.hpp>
#include <boost/geometry/algorithms/within.hpp>
#include <boost/geometry/geometries/multi_polygon.hpp>
#include <boost/geometry/geometries/point_xy.hpp>
#include <boost/geometry/geometries/polygon.hpp>
#include <boost/geometry/geometries/segment.hpp>
#include <boost/geometry/io/wkt/read.hpp>
#include <vector>

namespace bg = boost::geometry;
typedef bg::model::d2::point_xy<double> Point;
typedef bg::model::polygon<Point> Polygon;
typedef bg::model::multi_polygon<Polygon> MultiPolygon;
typedef bg::model::segment<Point> Segment;

int main() {
    Polygon a, b, overlap;
    bg::read_wkt("POLYGON((0 0,0 4,4 4,4 0,0 0))", a);
    bg::read_wkt("POLYGON((2 2,2 6,6 6,6 2,2 2))", b);
    bg::read_wkt("POLYGON((2 2,2 4,4 4,4 2,2 2))", overlap);
    if (bg::area(a) != 16 || bg::perimeter(a) != 16) return 1;
    Point c;
    bg::centroid(a, c);
    if (c.x() != 2 || c.y() != 2) return 2;

    if (bg::distance(Point(0, 0), Point(3, 4)) != 5 || bg::distance(Point(7, 2), a) != 3) return 3;
    if (!bg::within(Point(1, 1), a) || bg::within(Point(5, 5), a) || !bg::intersects(a, b)) return 4;

    std::vector<Point> crossing;
    bg::intersection(Segment(Point(0, 0), Point(4, 4)), Segment(Point(0, 4), Point(4, 0)), crossing);
    if (crossing.size() != 1 || crossing[0].x() != 2 || crossing[0].y() != 2) return 5;

    MultiPolygon both, either;
    bg::intersection(a, b, both);
    bg::union_(a, b, either);
    if (both.size() != 1 || bg::area(both) != 4 || !bg::equals(both[0], overlap)) return 6;
    return either.size() == 1 && bg::area(either) == 28 && bg::perimeter(either) == 24 ? 0 : 7;
}
