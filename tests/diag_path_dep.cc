#include "helpers.hpp"

#include <boost/ut.hpp>
#include <cstddef>
#include <filesystem>
#include <string>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;
  namespace fs = std::filesystem;

  "diagnostics show dep build without extra finish"_test = [] {
    const tests::TempDir tmp;

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(depRoot / "include" / "dep" / "dep.hpp",
                     R"(#pragma once

inline int dep_value() { return 7; }
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

int main() { return dep_value() == 7 ? 0 : 1; }
)");

    const auto result =
        tests::runCabin({ "test" }, appRoot).expect("cabin test");
    expect(result.status.success()) << result.status.toString();

    const std::string err = tests::sanitizeOutput(result.err);
    const std::size_t analyzePos = err.find("Analyzing project dependencies");
    const std::size_t depPos = err.find("Building dep (");
    expect(analyzePos != std::string::npos);
    expect(depPos != std::string::npos);
    expect(analyzePos < depPos);

    std::size_t finishedCount = 0;
    std::string::size_type pos = 0;
    static constexpr std::string_view finishedTag = "Finished `test` profile";
    while ((pos = err.find(finishedTag, pos)) != std::string::npos) {
      ++finishedCount;
      pos += finishedTag.size();
    }
    expect(finishedCount <= std::size_t{ 1 });
  };
}
