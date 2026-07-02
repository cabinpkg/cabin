#include <cstdio>
#include <nlohmann/json.hpp>

int main() {
    const nlohmann::json doc = nlohmann::json::parse(R"({"name":"Cabin","answer":42})");

    std::printf("json parsed name: %s\n", doc["name"].get<std::string>().c_str());
    std::printf("json parsed answer: %d\n", doc["answer"].get<int>());
    std::printf("nlohmann_json version: %d.%d.%d\n", NLOHMANN_JSON_VERSION_MAJOR,
                NLOHMANN_JSON_VERSION_MINOR, NLOHMANN_JSON_VERSION_PATCH);
    return 0;
}
