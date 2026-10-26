// Exercises Boost.Coroutine's compiled pieces: stack_traits from the
// per-OS source, coroutine_context's fcontext switches in both a pull
// (generator) and a push (sink) coroutine, an exception thrown inside a
// coroutine reaching its caller, and the library's error category.
#include <boost/coroutine/all.hpp>
#include <stdexcept>
#include <string>
#include <vector>

namespace coro = boost::coroutines;

int main() {
    if (coro::stack_traits::page_size() == 0 ||
        coro::stack_traits::default_size() < coro::stack_traits::minimum_size())
        return 1;

    std::vector<int> fib;
    coro::asymmetric_coroutine<int>::pull_type source(
        [](coro::asymmetric_coroutine<int>::push_type& yield) {
            int a = 0, b = 1;
            for (int i = 0; i < 10; ++i) {
                yield(a);
                int next = a + b;
                a = b;
                b = next;
            }
        });
    for (int v : source) fib.push_back(v);
    if (fib != std::vector<int>{0, 1, 1, 2, 3, 5, 8, 13, 21, 34}) return 1;

    int total = 0;
    coro::asymmetric_coroutine<int>::push_type adder(
        [&total](coro::asymmetric_coroutine<int>::pull_type& in) {
            for (int v : in) total += v;
        });
    for (int i = 1; i <= 4; ++i) adder(i);
    if (total != 10) return 1;

    try {
        coro::asymmetric_coroutine<void>::pull_type thrower(
            [](coro::asymmetric_coroutine<void>::push_type&) { throw std::runtime_error("inside"); });
        return 1;
    } catch (const std::runtime_error& e) {
        if (std::string(e.what()) != "inside") return 1;
    }

    const boost::system::error_category& cat = coro::coroutine_category();
    if (std::string(cat.name()) != "coroutine") return 1;
    const std::string no_data = cat.message(static_cast<int>(coro::coroutine_errc::no_data));
    return no_data.find("no valid result") != std::string::npos ? 0 : 1;
}
