#include "calc/calc.hpp"

#include <cstdio>

// A Cabin `test` target is an executable: it passes when `main`
// returns 0 and fails otherwise.
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
    check(calc::add(2, 3) == 5, "add(2, 3) == 5");
    check(calc::add(-4, 4) == 0, "add(-4, 4) == 0");
    check(calc::factorial(0) == 1, "factorial(0) == 1");
    check(calc::factorial(5) == 120, "factorial(5) == 120");
    return failures;
}
