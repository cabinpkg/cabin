#include "calc/calc.hpp"

#include <cstdio>

// Second test target for the same library. `cabin test` runs both and
// starts them in ascending (package, target name) order, so this one
// starts after `calc_test`.
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
