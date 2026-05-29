#include <iostream>
#include <zlib.h>

int main() {
    std::cout << "zlib version: " << zlibVersion() << '\n';
    return 0;
}
