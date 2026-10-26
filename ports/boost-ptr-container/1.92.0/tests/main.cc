// Exercises Boost.Pointer Container: ptr_vector of polymorphic objects with indirected
// access and sorting, ptr_map ownership, and release of an element.
#include <boost/ptr_container/ptr_map.hpp>
#include <boost/ptr_container/ptr_vector.hpp>
#include <string>

struct shape {
    virtual ~shape() {}
    virtual int area() const = 0;
    bool operator<(const shape& o) const { return area() < o.area(); }
};
struct square : shape {
    int s;
    explicit square(int side) : s(side) {}
    int area() const { return s * s; }
};
struct rect : shape {
    int w, h;
    rect(int width, int height) : w(width), h(height) {}
    int area() const { return w * h; }
};

int main() {
    boost::ptr_vector<shape> shapes;
    shapes.push_back(new square(3));
    shapes.push_back(new rect(2, 1));
    shapes.push_back(new square(2));
    if (shapes.size() != 3 || shapes[0].area() != 9) return 1;

    shapes.sort();
    if (shapes.front().area() != 2 || shapes.back().area() != 9) return 1;

    int total = 0;
    for (boost::ptr_vector<shape>::const_iterator it = shapes.begin(); it != shapes.end(); ++it)
        total += it->area();
    if (total != 15) return 1;

    boost::ptr_vector<shape>::auto_type last = shapes.pop_back();
    if (last->area() != 9 || shapes.size() != 2) return 1;

    boost::ptr_map<std::string, shape> named;
    std::string key = "unit";
    named.insert(key, new square(1));
    if (named.at("unit").area() != 1 || named.count("missing") != 0) return 1;
    return 0;
}
