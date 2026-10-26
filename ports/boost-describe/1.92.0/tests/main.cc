// Checks Boost.Describe enum and struct reflection. Before C++14 the describe
// macros are documented no-ops, so there the traits must report "not described".
#include <boost/describe.hpp>
#include <cstring>

enum class Color { red, green, blue };
BOOST_DESCRIBE_ENUM(Color, red, green, blue)

struct Point { int x; int y; };
BOOST_DESCRIBE_STRUCT(Point, (), (x, y))

enum Plain { plain_a };

int main() {
    using namespace boost::describe;
    static_assert(!has_describe_enumerators<Plain>::value, "undescribed enum");
#if defined(BOOST_DESCRIBE_CXX14)
    static_assert(has_describe_enumerators<Color>::value, "Color is described");
    static_assert(has_describe_members<Point>::value, "Point is described");
    if (std::strcmp(enum_to_string(Color::green, "?"), "green") != 0) return 1;
    Color c = Color::red;
    if (!enum_from_string("blue", c) || c != Color::blue) return 1;
    if (enum_from_string("purple", c)) return 1;

    Point p = {3, 4};
    int sum = 0;
    boost::mp11::mp_for_each<describe_members<Point, mod_public>>(
        [&](auto d) { sum += p.*d.pointer; });
    if (sum != 7) return 1;
#else
    if (has_describe_enumerators<Color>::value || has_describe_members<Point>::value) return 1;
#endif
    return 0;
}
