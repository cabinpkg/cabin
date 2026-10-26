// Exercises Boost.Functional's factory, value_factory, and forward_adapter
// function-object utilities.
#include <boost/functional/factory.hpp>
#include <boost/functional/forward_adapter.hpp>
#include <boost/functional/value_factory.hpp>
#include <memory>
#include <string>

struct Pair {
    int a;
    std::string b;
    Pair(int x, const std::string& y) : a(x), b(y) {}
};

struct Sum {
    template <class A, class B> int operator()(A& x, B& y) const { return x + y; }
};

int main() {
    std::unique_ptr<Pair> heap(boost::factory<Pair*>()(1, std::string("one")));
    if (!heap || heap->a != 1 || heap->b != "one") return 1;

    Pair value = boost::value_factory<Pair>()(2, std::string("two"));
    if (value.a != 2 || value.b != "two") return 1;

    // Sum binds its arguments by reference; forward_adapter lets it accept rvalues.
    boost::forward_adapter<Sum> sum;
    int x = 3;
    if (sum(x, 4) != 7 || sum(5, 6) != 11) return 1;
    return 0;
}
