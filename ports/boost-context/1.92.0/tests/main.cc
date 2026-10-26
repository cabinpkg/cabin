// Exercises Boost.Context's compiled fcontext switch: a Fibonacci fiber
// on a guarded stack hands values back to main (make/jump_fcontext),
// resume_with runs a function on top of it (ontop_fcontext), and
// stack_traits comes from the per-OS source.
#include <boost/context/fiber.hpp>
#include <boost/context/protected_fixedsize_stack.hpp>
#include <boost/context/stack_traits.hpp>
#include <memory>
#include <utility>
#include <vector>

namespace ctx = boost::context;

int main() {
    if (ctx::stack_traits::page_size() == 0 ||
        ctx::stack_traits::default_size() < ctx::stack_traits::minimum_size())
        return 1;

    std::vector<int> seen;
    int value = 0;
    ctx::fiber fib{std::allocator_arg, ctx::protected_fixedsize_stack(),
                   [&value](ctx::fiber&& main) -> ctx::fiber {
                       int a = 0, b = 1;
                       for (int i = 0; i < 10; ++i) {
                           value = a;
                           int next = a + b;
                           a = b;
                           b = next;
                           main = std::move(main).resume();
                       }
                       value = -1;
                       return std::move(main);
                   }};
    for (int i = 0; i < 10; ++i) {
        if (i == 5)
            fib = std::move(fib).resume_with([&seen](ctx::fiber&& main) -> ctx::fiber {
                seen.push_back(100);
                return std::move(main);
            });
        else
            fib = std::move(fib).resume();
        seen.push_back(value);
    }
    fib = std::move(fib).resume();  // the loop ends and the fiber returns

    const int expected[] = {0, 1, 1, 2, 3, 100, 5, 8, 13, 21, 34};
    if (seen != std::vector<int>(expected, expected + 11)) return 1;
    return !fib && value == -1 ? 0 : 1;
}
