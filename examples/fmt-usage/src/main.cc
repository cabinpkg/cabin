#include <fmt/format.h>

int main() {
    fmt::print("fmt version: {}\n", FMT_VERSION);
    fmt::print("{}\n", fmt::format("Hello, {}!", "Cabin"));
    return 0;
}
