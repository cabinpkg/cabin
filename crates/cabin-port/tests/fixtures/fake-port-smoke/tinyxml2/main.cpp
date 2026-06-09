#include <iostream>
#include <tinyxml2.h>

int main() {
    const char *version = tinyxml2::fake_version();
    std::cout << "fake tinyxml2: " << version << "\n";
    return version[0] == '\0';
}
