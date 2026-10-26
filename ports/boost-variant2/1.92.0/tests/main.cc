// Boost.Variant2: never-valueless variant with index(), holds_alternative, get/get_if, visit,
// bad_variant_access, and monostate.
#include <boost/variant2/variant.hpp>
#include <string>

struct Describe {
    int operator()(int i) const { return i; }
    int operator()(double) const { return -1; }
    int operator()(const std::string& s) const { return static_cast<int>(s.size()); }
};

int main() {
    using namespace boost::variant2;
    variant<int, double, std::string> v(7);
    if (v.index() != 0 || !holds_alternative<int>(v) || get<int>(v) != 7) return 1;
    if (visit(Describe(), v) != 7) return 1;

    v = std::string("abc");
    if (get_if<int>(&v) != nullptr || *get_if<std::string>(&v) != "abc") return 1;
    if (visit(Describe(), v) != 3) return 1;

    v.emplace<double>(1.5);
    try {
        (void)get<std::string>(v);
        return 1;
    } catch (const bad_variant_access&) {
    }

    variant<monostate, int> m;
    if (m.index() != 0) return 1;
    m = 5;
    return (v == variant<int, double, std::string>(1.5) && get<1>(m) == 5) ? 0 : 1;
}
