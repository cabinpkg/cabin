// Boost.YAP: build lazy expression trees from terminals and placeholders, evaluate them with
// arguments, and rewrite terminals with a transform.
#include <boost/yap/yap.hpp>

struct double_terminals {
    auto operator()(boost::yap::expr_tag<boost::yap::expr_kind::terminal>, int i) {
        return boost::yap::make_terminal(i * 2);
    }
};

int main() {
    using namespace boost::yap::literals;

    auto expr = boost::yap::make_terminal(1) + 2 * boost::yap::make_terminal(3);
    if (boost::yap::evaluate(expr) != 7) return 1;

    auto with_placeholders = 1_p * 2_p + 3;
    if (boost::yap::evaluate(with_placeholders, 4, 5) != 23) return 1;

    if (expr.kind != boost::yap::expr_kind::plus) return 1;
    auto doubled = boost::yap::transform(expr, double_terminals());
    return boost::yap::evaluate(doubled) == 26 ? 0 : 1;
}
