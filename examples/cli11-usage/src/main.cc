#include <CLI/CLI.hpp>
#include <cstdio>

int main(int argc, char **argv) {
    CLI::App app{"CLI11 consumer example"};
    int count = 3;
    app.add_option("-c,--count", count, "Greeting count");
    CLI11_PARSE(app, argc, argv);

    std::printf("CLI11 parsed count: %d\n", count);
    std::printf("CLI11 version: %s\n", CLI11_VERSION);
    return 0;
}
