// Boost.Variant: hold alternatives, query which(), get<> with failure, and apply a static_visitor,
// including a recursive variant.
#include <boost/variant.hpp>
#include <string>
#include <vector>

struct Size : boost::static_visitor<std::size_t> {
    std::size_t operator()(int) const { return 1; }
    std::size_t operator()(const std::string& s) const { return s.size(); }
};

typedef boost::make_recursive_variant<int, std::vector<boost::recursive_variant_> >::type Tree;

struct Sum : boost::static_visitor<int> {
    int operator()(int i) const { return i; }
    int operator()(const std::vector<Tree>& v) const {
        int total = 0;
        for (std::size_t i = 0; i < v.size(); ++i) total += boost::apply_visitor(*this, v[i]);
        return total;
    }
};

int main() {
    boost::variant<int, std::string> v = 42;
    if (v.which() != 0 || boost::get<int>(v) != 42 || boost::apply_visitor(Size(), v) != 1) return 1;
    v = std::string("hello");
    if (v.which() != 1 || boost::get<int>(&v) != 0 || boost::apply_visitor(Size(), v) != 5) return 1;
    try {
        (void)boost::get<int>(v);
        return 1;
    } catch (const boost::bad_get&) {
    }

    std::vector<Tree> inner(2, Tree(3));
    std::vector<Tree> outer(1, Tree(4));
    outer.push_back(Tree(inner));
    return boost::apply_visitor(Sum(), Tree(outer)) == 10 ? 0 : 1;
}
