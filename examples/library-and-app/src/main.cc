#include <iostream>

#include "greet/greet.hpp"

int main() {
    std::cout << greet::hello("Cabin") << '\n';
    return 0;
}
