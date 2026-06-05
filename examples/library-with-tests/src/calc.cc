#include "calc/calc.hpp"

namespace calc {

int add(int a, int b) {
    return a + b;
}

long factorial(int n) {
    long result = 1;
    for (int i = 2; i <= n; ++i) {
        result *= i;
    }
    return result;
}

bool is_even(int n) {
    return n % 2 == 0;
}

}  // namespace calc
