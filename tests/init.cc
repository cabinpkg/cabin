#include "helpers.hpp"

#include <boost/ut.hpp>
#include <string>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin init"_test = [] {
    tests::TempDir tmp;
    const auto project = tmp.path / "pkg";
    tests::fs::create_directories(project);

    const auto result = tests::runCabin({ "init" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr =
        "     Created binary (application) `pkg` package\n";
    expect(sanitizedErr == expectedErr);
    expect(tests::fs::is_regular_file(project / "cabin.toml"));
  };

  "cabin init existing"_test = [] {
    tests::TempDir tmp;
    const auto project = tmp.path / "pkg";
    tests::fs::create_directories(project);

    const auto first = tests::runCabin({ "init" }, project).unwrap();
    expect(first.status.success());
    auto firstOut = tests::sanitizeOutput(first.out);
    expect(firstOut.empty());
    auto firstErr = tests::sanitizeOutput(first.err);
    const std::string expectedFirstErr =
        "     Created binary (application) `pkg` package\n";
    expect(firstErr == expectedFirstErr);

    const auto second = tests::runCabin({ "init" }, project).unwrap();
    expect(!second.status.success());
    auto secondOut = tests::sanitizeOutput(second.out);
    expect(secondOut.empty());
    auto secondErr = tests::sanitizeOutput(second.err);
    const std::string expectedSecondErr =
        "Error: cannot initialize an existing cabin package\n";
    expect(secondErr == expectedSecondErr);
    expect(tests::fs::is_regular_file(project / "cabin.toml"));
  };
}
