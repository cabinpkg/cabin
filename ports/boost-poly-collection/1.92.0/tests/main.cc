// Exercises Boost.PolyCollection's four containers: base_collection keeps
// each derived type in its own segment and dispatches virtual calls,
// function_collection stores callables, any_collection stores
// Boost.TypeErasure values, variant_collection visits alternatives, and
// the type-restituting poly_collection algorithms run across segments.
#include <boost/poly_collection/algorithm.hpp>
#include <boost/poly_collection/any_collection.hpp>
#include <boost/poly_collection/base_collection.hpp>
#include <boost/poly_collection/function_collection.hpp>
#include <boost/poly_collection/variant_collection.hpp>
#include <boost/type_erasure/operators.hpp>
#include <sstream>
#include <string>

struct shape {
    virtual ~shape() {}
    virtual int area() const = 0;
};
struct square : shape {
    int side;
    explicit square(int s) : side(s) {}
    int area() const override { return side * side; }
};
struct rect : shape {
    int w, h;
    rect(int w_, int h_) : w(w_), h(h_) {}
    int area() const override { return w * h; }
};

int main() {
    boost::base_collection<shape> shapes;
    shapes.insert(square(2));
    shapes.insert(rect(2, 3));
    shapes.insert(square(3));
    if (shapes.size() != 3 || shapes.size<square>() != 2 || shapes.size<rect>() != 1) return 1;
    int total = 0;
    boost::poly_collection::for_each<square, rect>(shapes.begin(), shapes.end(), [&](const shape& s) { total += s.area(); });
    if (total != 19) return 2;
    if (boost::poly_collection::count_if(shapes.begin(), shapes.end(), [](const shape& s) { return s.area() > 5; }) != 2) return 3;

    boost::function_collection<int(int)> fns;
    fns.insert([](int x) { return x + 1; });
    fns.insert([](int x) { return x * 10; });
    int sum = 0;
    for (const auto& f : fns) sum += f(4);
    if (sum != 45) return 4;

    boost::any_collection<boost::type_erasure::ostreamable<> > printable;
    printable.insert(7);
    printable.insert(std::string("x"));
    std::ostringstream os;
    for (const auto& p : printable) os << p << ';';
    if (os.str() != "7;x;" && os.str() != "x;7;") return 5;

    boost::variant_collection_of<int, std::string> values;
    values.insert(5);
    values.insert(std::string("abc"));
    values.insert(6);
    std::size_t weight = 0;
    for (const auto& v : values)
        weight += boost::poly_collection::holds_alternative<int>(v) ? 1 : boost::poly_collection::get<std::string>(v).size();
    return values.size<int>() == 2 && weight == 5 ? 0 : 6;
}
