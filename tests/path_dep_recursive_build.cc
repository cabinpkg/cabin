#include "helpers.hpp"

#include <boost/ut.hpp>
#include <cstddef>
#include <filesystem>
#include <iostream>
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
    if (!result.status.success()) {
      std::cout << "==== cabin stdout ====\n"
                << tests::sanitizeOutput(result.out) << '\n';
      std::cout << "==== cabin stderr ====\n"
                << tests::sanitizeOutput(result.err) << '\n';
    }
    expect(result.status.success()) << result.status.toString();

    const std::string err = tests::sanitizeOutput(result.err);
    expect(err.contains("Analyzing project dependencies")) << err;

    std::vector<std::string> buildLines;
    std::istringstream iss(err);
    std::string line;
    while (std::getline(iss, line)) {
      if (line.contains("Building ")) {
        buildLines.push_back(line);
      }
    }

    std::size_t depIdx = std::string::npos;
    std::size_t innerIdx = std::string::npos;
    for (std::size_t i = 0; i < buildLines.size(); ++i) {
      if (depIdx == std::string::npos && buildLines[i].contains("Building dep (")) {
        depIdx = i;
      }
      if (innerIdx == std::string::npos
          && buildLines[i].contains("Building inner (")) {
        innerIdx = i;
      }
    }

    expect(depIdx != std::string::npos) << err;
    expect(innerIdx != std::string::npos) << err;
    expect(depIdx < innerIdx) << err;
  };
}
