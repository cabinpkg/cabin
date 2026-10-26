// Exercises Boost.Proto: building an expression tree from terminals, matching it against a
// grammar, and evaluating it with the default context.
#include <boost/proto/proto.hpp>

namespace proto = boost::proto;

struct arithmetic
    : proto::or_<proto::terminal<int>,
                 proto::plus<arithmetic, arithmetic>,
                 proto::multiplies<arithmetic, arithmetic> > {};

int main() {
    proto::terminal<int>::type a = {6}, b = {7};
    proto::default_context ctx;

    if (proto::eval(a * b, ctx) != 42) return 1;
    if (proto::eval(a + b * 2, ctx) != 20) return 1;
    if (proto::value(a) != 6) return 1;

    static_assert(proto::matches<proto::result_of::as_expr<int>::type, arithmetic>::value, "terminal");
    static_assert(proto::matches<decltype(a + b * a), arithmetic>::value, "plus/multiplies");
    static_assert(!proto::matches<decltype(a - b), arithmetic>::value, "minus is excluded");

    if (proto::arity_of<decltype(a + b)>::value != 2) return 1;
    if (proto::value(proto::left(a + b)) != 6) return 1;
    return 0;
}
