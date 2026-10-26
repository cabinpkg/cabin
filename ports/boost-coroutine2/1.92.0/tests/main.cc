// Exercises Boost.Coroutine2 on Boost.Context's compiled fcontext: a pull
// coroutine generating Fibonacci numbers on a guarded stack, a push
// coroutine consuming values, and an exception crossing back to the
// caller.
#include <boost/coroutine2/all.hpp>
#include <stdexcept>
#include <string>
#include <vector>

typedef boost::coroutines2::coroutine<int> coro_t;

int main() {
    std::vector<int> fib;
    coro_t::pull_type source(boost::coroutines2::protected_fixedsize_stack(),
                             [](coro_t::push_type& yield) {
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

    std::string seen;
    coro_t::push_type sink([&seen](coro_t::pull_type& in) {
        for (int v : in) seen += static_cast<char>('a' + v);
    });
    for (int i = 0; i < 5; ++i) sink(i);
    if (seen != "abcde") return 1;

    try {
        coro_t::pull_type thrower([](coro_t::push_type&) { throw std::runtime_error("inside"); });
        return 1;
    } catch (const std::runtime_error& e) {
        return std::string(e.what()) == "inside" ? 0 : 1;
    }
}
