#include "calc/calc.hpp"

#include <cstdio>

// Second test target for the same library. `cabin test` discovers and
// runs both, in a deterministic order (by package, then target name),
// so this one always runs after `calc_test`.
namespace {

int failures = 0;

void check(bool condition, const char* what) {
    if (!condition) {
        std::fprintf(stderr, "FAILED check: %s\n", what);
        ++failures;
    }
}

}  // namespace

int main() {
    check(calc::is_even(0), "is_even(0)");
    check(calc::is_even(4), "is_even(4)");
    check(!calc::is_even(7), "!is_even(7)");
    return failures;
}
