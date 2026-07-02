#include <cstdio>

#include "greeter/greeter.hpp"

int main() {
    std::printf("%s\n", greeter::greeting("Cabin").c_str());
    return 0;
}
