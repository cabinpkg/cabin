#include "helpers.hpp"

#include <boost/ut.hpp>
#include <string>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin new binary"_test = [] {
    const tests::TempDir tmp;
    const auto result =
        tests::runCabin({ "new", "hello_world" }, tmp.path).unwrap();

    expect(result.status.success()) << result.status.toString();

    const auto project = tmp.path / "hello_world";
    expect(tests::fs::is_directory(project)) << "project directory";
    expect(tests::fs::is_directory(project / ".git")) << "git repo";
    expect(tests::fs::is_regular_file(project / ".gitignore"));
    expect(tests::fs::is_regular_file(project / "cabin.toml"));
    expect(tests::fs::is_directory(project / "src"));
    expect(tests::fs::is_regular_file(project / "src/main.cc"));
    expect(!tests::fs::exists(project / "lib"));
    expect(!tests::fs::exists(project / "include"));

    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr =
        "     Created binary (application) `hello_world` package\n";
    expect(sanitizedErr == expectedErr);
  };

  "cabin new library"_test = [] {
    const tests::TempDir tmp;
    const auto result =
        tests::runCabin({ "new", "--lib", "hello_world" }, tmp.path).unwrap();

    expect(result.status.success()) << result.status.toString();

    const auto project = tmp.path / "hello_world";
    expect(tests::fs::is_directory(project));
    expect(tests::fs::is_directory(project / ".git"));
    expect(tests::fs::is_regular_file(project / ".gitignore"));
    expect(tests::fs::is_regular_file(project / "cabin.toml"));
    expect(tests::fs::is_directory(project / "include"));
    expect(tests::fs::is_regular_file(project
                                      / "include/hello_world/hello_world.hpp"));
    expect(tests::fs::is_regular_file(project / "lib/hello_world.cc"));

    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr =
        "     Created library `hello_world` package\n";
    expect(sanitizedErr == expectedErr);
  };

  "cabin new hyphenated library"_test = [] {
    const tests::TempDir tmp;
    const auto result =
        tests::runCabin({ "new", "--lib", "my-lib" }, tmp.path).unwrap();

    expect(result.status.success()) << result.status.toString();

    const auto project = tmp.path / "my-lib";
    const auto header = project / "include" / "my-lib" / "my-lib.hpp";
    const auto impl = project / "lib" / "my-lib.cc";
    expect(tests::fs::is_regular_file(header));
    expect(tests::fs::is_regular_file(impl));

    const auto headerContent = tests::readFile(header);
    expect(headerContent.contains("namespace my_lib"));
    const auto implContent = tests::readFile(impl);
    expect(implContent.contains("namespace my_lib"));
  };

  "cabin new requires name"_test = [] {
    const tests::TempDir tmp;
    const auto result = tests::runCabin({ "new" }, tmp.path).unwrap();

    expect(!result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr = "Error: package name must not be empty\n";
    expect(sanitizedErr == expectedErr);
  };

  "cabin new existing"_test = [] {
    const tests::TempDir tmp;
    const auto project = tmp.path / "existing";
    tests::fs::create_directories(project);

    const auto result =
        tests::runCabin({ "new", "existing" }, tmp.path).unwrap();

    expect(!result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr =
        "Error: directory `existing` already exists\n";
    expect(sanitizedErr == expectedErr);
  };
}
