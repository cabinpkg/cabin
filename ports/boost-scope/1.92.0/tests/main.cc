// Exercises Boost.Scope: scope_exit (including release), scope_fail/scope_success on normal
// and exceptional exit, and unique_resource with a checked invalid value.
#include <boost/scope/scope_exit.hpp>
#include <boost/scope/scope_fail.hpp>
#include <boost/scope/scope_success.hpp>
#include <boost/scope/unique_resource.hpp>
#include <stdexcept>

struct closer {
    int* closed;
    void operator()(int) const noexcept { ++*closed; }
};

int main() {
    int exits = 0, fails = 0, successes = 0;
    {
        auto a = boost::scope::make_scope_exit([&] { ++exits; });
        auto b = boost::scope::make_scope_exit([&] { ++exits; });
        b.set_active(false);
    }
    if (exits != 1) return 1;

    {
        auto f = boost::scope::make_scope_fail([&] { ++fails; });
        auto s = boost::scope::make_scope_success([&] { ++successes; });
    }
    try {
        auto f = boost::scope::make_scope_fail([&] { ++fails; });
        auto s = boost::scope::make_scope_success([&] { ++successes; });
        throw std::runtime_error("boom");
    } catch (const std::runtime_error&) {
    }
    if (fails != 1 || successes != 1) return 1;

    int closed = 0;
    {
        auto valid = boost::scope::make_unique_resource_checked(42, -1, closer{&closed});
        auto invalid = boost::scope::make_unique_resource_checked(-1, -1, closer{&closed});
        if (!valid.allocated() || valid.get() != 42 || invalid.allocated()) return 1;
    }
    if (closed != 1) return 1;
    return 0;
}
