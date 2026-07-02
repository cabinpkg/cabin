#include <cstdio>
#include <spdlog/spdlog.h>

int main() {
    spdlog::info("Hello from spdlog!");
    std::printf("spdlog version: %d.%d.%d\n", SPDLOG_VER_MAJOR,
                SPDLOG_VER_MINOR, SPDLOG_VER_PATCH);
    return 0;
}
