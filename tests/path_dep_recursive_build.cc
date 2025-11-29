#include "helpers.hpp"

#include <boost/ut.hpp>
#include <cstddef>
#include <filesystem>
#include <string>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;
  namespace fs = std::filesystem;

  "recursive path deps are built in order"_test = [] {
    const tests::TempDir tmp;

    const fs::path innerRoot = tmp.path / "inner";
    fs::create_directories(innerRoot / "include" / "inner");
    fs::create_directories(innerRoot / "lib");
    tests::writeFile(innerRoot / "cabin.toml",
                     R"([package]
name = "inner"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(innerRoot / "include" / "inner" / "inner.hpp",
                     R"(#pragma once

int inner_value();
)");
    tests::writeFile(innerRoot / "lib" / "inner.cc",
                     R"(#include "inner/inner.hpp"

int inner_value() { return 3; }
)");

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
    fs::create_directories(depRoot / "lib");
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"

[dependencies]
inner = {path = "../inner"}
)");
    tests::writeFile(depRoot / "include" / "dep" / "dep.hpp",
                     R"(#pragma once

int dep_value();
)");
    tests::writeFile(depRoot / "lib" / "dep.cc",
                     R"(#include "dep/dep.hpp"
#include "inner/inner.hpp"

int dep_value() { return inner_value() + 1; }
)");

    const fs::path appRoot = tmp.path / "app";
    fs::create_directories(appRoot / "src");
    tests::writeFile(appRoot / "cabin.toml",
                     R"([package]
name = "app"
version = "0.1.0"
edition = "23"

[dependencies]
dep = {path = "../dep"}
)");
    tests::writeFile(appRoot / "src" / "main.cc",
                     R"(#include "dep/dep.hpp"

int main() { return dep_value() == 4 ? 0 : 1; }
)");

    const auto result =
        tests::runCabin({ "build" }, appRoot).expect("cabin build");
    expect(result.status.success()) << result.status.toString();

    const std::string err = tests::sanitizeOutput(result.err);
    const std::size_t analyzePos = err.find("Analyzing project dependencies");
    const std::size_t depPos = err.find("Building dep (");
    const std::size_t innerPos = err.find("Building inner (");
    expect(analyzePos != std::string::npos);
    expect(depPos != std::string::npos);
    expect(innerPos != std::string::npos);
    expect(analyzePos < depPos);
    expect(depPos < innerPos);
  };
}
