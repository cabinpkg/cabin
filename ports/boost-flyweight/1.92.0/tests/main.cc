// Exercises Boost.Flyweight: the default hashed, ref-counted, locked
// factory sharing equal values, a key-value flyweight that constructs
// each value once per key, and the set factory without tracking.
#include <boost/flyweight.hpp>
#include <boost/flyweight/key_value.hpp>
#include <boost/flyweight/no_tracking.hpp>
#include <boost/flyweight/set_factory.hpp>

#include <string>

namespace fw = boost::flyweights;

struct texture {
    static int loads;
    std::string name;
    explicit texture(const std::string& n) : name(n) { ++loads; }
};
int texture::loads = 0;

struct texture_name {
    const std::string& operator()(const texture& t) const { return t.name; }
};

int main() {
    typedef fw::flyweight<std::string> name;
    const name a("boost"), b(std::string("bo") + "ost"), c("cabin");
    if (a != b || &a.get() != &b.get() || a == c || a.get() != "boost") return 1;
    name d(c);
    if (&d.get() != &c.get()) return 1;
    d = a;
    if (&d.get() != &a.get() || d.get().size() != 5) return 1;

    typedef fw::flyweight<fw::key_value<std::string, texture, texture_name> > tex;
    const tex t1("stone"), t2("stone"), t3("grass");
    if (texture::loads != 2 || &t1.get() != &t2.get() || t3.get().name != "grass") return 1;

    typedef fw::flyweight<int, fw::set_factory<>, fw::no_tracking> number;
    const number n1(42), n2(42), n3(7);
    if (&n1.get() != &n2.get() || !(n3 < n1) || n1.get() != 42) return 1;
    return 0;
}
