#include <CLI/CLI.hpp>
#include <cstdio>
#include <fmt/format.h>
#include <spdlog/spdlog.h>
#include <string>

int main(int argc, char **argv) {
    CLI::App app{"cli-with-spdlog: CLI11 flags, fmt formatting, spdlog logging"};
    std::string name = "Cabin";
    int count = 2;
    app.add_option("-n,--name", name, "Who to greet");
    app.add_option("-c,--count", count, "How many greetings");
    CLI11_PARSE(app, argc, argv);

    spdlog::info("preparing {} greeting(s) for {}", count, name);
    for (int i = 1; i <= count; ++i) {
        std::printf("%s\n", fmt::format("{}/{}: Hello, {}!", i, count, name).c_str());
    }
    std::printf("spdlog version: %d.%d.%d\n", SPDLOG_VER_MAJOR, SPDLOG_VER_MINOR,
                SPDLOG_VER_PATCH);
    // FMT_VERSION comes from the external fmt port's headers here,
    // not from spdlog's bundled copy (see SPDLOG_FMT_EXTERNAL in
    // cabin.toml).
    std::printf("fmt version (external): %d\n", FMT_VERSION);
    return 0;
}
