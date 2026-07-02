#include <cstdio>
#include <nlohmann/json.hpp>
#include <string>

// A miniature manifest-inspector: parse a JSON document, pull typed
// values out of nested objects and arrays, and emit a derived JSON
// summary. nlohmann::json stores object keys sorted, so the dumped
// summary is deterministic.
int main() {
    const nlohmann::json manifest = nlohmann::json::parse(R"({
        "package": { "name": "json-cli", "version": "0.1.0" },
        "dependencies": [ "fmt", "spdlog", "sqlite3" ]
    })");

    const auto &package = manifest["package"];
    std::printf("package: %s v%s\n", package["name"].get<std::string>().c_str(),
                package["version"].get<std::string>().c_str());

    const auto &deps = manifest["dependencies"];
    std::printf("dependency count: %zu\n", deps.size());
    for (const auto &dep : deps) {
        std::printf("  dep: %s\n", dep.get<std::string>().c_str());
    }

    const nlohmann::json summary = {
        {"name", package["name"]},
        {"deps", deps},
    };
    std::printf("summary: %s\n", summary.dump().c_str());
    return 0;
}
