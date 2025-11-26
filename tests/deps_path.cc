#include "helpers.hpp"

#include <boost/ut.hpp>
#include <filesystem>
#include <string>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;
  namespace fs = std::filesystem;

  "path dependency installs transitive deps"_test = [] {
    const tests::TempDir tmp;

    const fs::path innerRoot = tmp.path / "inner";
    fs::create_directories(innerRoot / "include" / "inner");
    tests::writeFile(innerRoot / "cabin.toml",
                     R"([package]
name = "inner"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(innerRoot / "include" / "inner" / "inner.hpp",
                     R"(#pragma once

inline int inner_value() { return 5; }
)");

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
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

#include "inner/inner.hpp"

inline int dep_value() { return inner_value(); }
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

int main() {
  return dep_value() == 5 ? 0 : 1;
}
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();

    expect(result.status.success()) << result.status.toString();
  };

  "path dependency can depend on another path dependency"_test = [] {
    const tests::TempDir tmp;

    const fs::path utilRoot = tmp.path / "util";
    fs::create_directories(utilRoot / "include" / "util");
    tests::writeFile(utilRoot / "cabin.toml",
                     R"([package]
name = "util"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(utilRoot / "include" / "util" / "util.hpp",
                     R"(#pragma once

inline int util_value() { return 42; }
)");

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"

[dependencies]
util = {path = "../util"}
)");
    tests::writeFile(depRoot / "include" / "dep" / "dep.hpp",
                     R"(#pragma once

#include "util/util.hpp"

inline int dep_value() { return util_value(); }
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

int main() {
  return dep_value() == 42 ? 0 : 1;
}
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();
    expect(result.status.success()) << result.status.toString();
  };

  "path dependency uses root when include/ is absent"_test = [] {
    const tests::TempDir tmp;

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot);
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(depRoot / "dep.hpp",
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
                     R"(#include "dep.hpp"

int main() { return dep_value() == 7 ? 0 : 1; }
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();
    expect(result.status.success()) << result.status.toString();
  };

  "root and dep agree on shared dep"_test = [] {
    const tests::TempDir tmp;

    const fs::path sharedRoot = tmp.path / "shared";
    fs::create_directories(sharedRoot / "include" / "shared");
    tests::writeFile(sharedRoot / "cabin.toml",
                     R"([package]
name = "shared"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(sharedRoot / "include" / "shared" / "shared.hpp",
                     R"(#pragma once

inline int shared_value() { return 11; }
)");

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"

[dependencies]
fmt = {path = "../shared"}
)");
    tests::writeFile(depRoot / "include" / "dep" / "dep.hpp",
                     R"(#pragma once

#include "shared/shared.hpp"

inline int dep_value() { return shared_value(); }
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
fmt = {path = "../shared"}
)");
    tests::writeFile(appRoot / "src" / "main.cc",
                     R"(#include "dep/dep.hpp"
#include "shared/shared.hpp"

int main() {
  return dep_value() == shared_value() ? 0 : 1;
}
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();
    expect(result.status.success()) << result.status.toString();
  };

  "root and dep conflict on shared dep"_test = [] {
    const tests::TempDir tmp;

    const fs::path sharedRoot = tmp.path / "shared";
    fs::create_directories(sharedRoot / "include" / "shared");
    tests::writeFile(sharedRoot / "cabin.toml",
                     R"([package]
name = "shared"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(sharedRoot / "include" / "shared" / "shared.hpp",
                     R"(#pragma once

inline int shared_value() { return 11; }
)");

    const fs::path otherRoot = tmp.path / "other";
    fs::create_directories(otherRoot / "include" / "other");
    tests::writeFile(otherRoot / "cabin.toml",
                     R"([package]
name = "other"
version = "0.1.0"
edition = "23"
)");
    tests::writeFile(otherRoot / "include" / "other" / "other.hpp",
                     R"(#pragma once

inline int other_value() { return 22; }
)");

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot / "include" / "dep");
    tests::writeFile(depRoot / "cabin.toml",
                     R"([package]
name = "dep"
version = "0.1.0"
edition = "23"

[dependencies]
fmt = {path = "../other"}
)");
    tests::writeFile(depRoot / "include" / "dep" / "dep.hpp",
                     R"(#pragma once

#include "other/other.hpp"

inline int dep_value() { return other_value(); }
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
fmt = {path = "../shared"}
)");
    tests::writeFile(appRoot / "src" / "main.cc",
                     R"(#include "dep/dep.hpp"
#include "shared/shared.hpp"

int main() {
  return dep_value() == shared_value() ? 0 : 1;
}
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();
    expect(!result.status.success());
    const auto err = tests::sanitizeOutput(result.err);
    expect(err.contains("dependency `fmt` conflicts across manifests"));
  };

  "path dependency without manifest fails"_test = [] {
    const tests::TempDir tmp;

    const fs::path depRoot = tmp.path / "dep";
    fs::create_directories(depRoot);
    tests::writeFile(depRoot / "dep.hpp",
                     R"(#pragma once

inline int dep_value() { return 1; }
)");

    const fs::path appRoot = tmp.path / "app";
    fs::create_directories(appRoot / "src");
    tests::writeFile(
        appRoot / "cabin.toml",
        R"([package]
name = "app"
version = "0.1.0"
edition = "23"

[dependencies]
dep = {path = "../dep"}
)");
    tests::writeFile(appRoot / "src" / "main.cc",
                     R"(#include "dep.hpp"

int main() { return dep_value(); }
)");

    const auto result = tests::runCabin({ "build" }, appRoot).unwrap();
    expect(!result.status.success());
    const auto err = tests::sanitizeOutput(result.err);
    expect(err.contains("missing `cabin.toml` in path dependency"));
  };
}
