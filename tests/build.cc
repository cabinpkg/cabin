#include "helpers.hpp"

#include <boost/ut.hpp>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin build emits ninja"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "ninja_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "ninja_project";

    const auto result = tests::runCabin({ "build" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();

    const auto outDir = project / "cabin-out" / "dev";
    expect(tests::fs::is_regular_file(outDir / "build.ninja"));
    expect(tests::fs::is_regular_file(outDir / "config.ninja"));
    expect(tests::fs::is_regular_file(outDir / "rules.ninja"));
    expect(tests::fs::is_regular_file(outDir / "targets.ninja"));
    expect(tests::fs::is_regular_file(outDir / "ninja_project"));
    expect(tests::fs::is_directory(outDir / "ninja_project.d"));
    expect(!tests::fs::exists(outDir / "Makefile"));
  };
}
