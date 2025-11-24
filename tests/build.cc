#include "helpers.hpp"

#include <boost/ut.hpp>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin build emits ninja"_test = [] {
    const tests::TempDir tmp;
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
    expect(!tests::fs::exists(outDir / "libninja_project.a"));
    expect(!tests::fs::exists(outDir / "Makefile"));
  };

  "cabin build handles src-only packages"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "binary_only" }, tmp.path).unwrap();
    const auto project = tmp.path / "binary_only";
    tests::fs::remove_all(project / "lib");
    expect(!tests::fs::exists(project / "lib"));

    const auto result = tests::runCabin({ "build" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();

    const auto outDir = project / "cabin-out" / "dev";
    expect(tests::fs::is_regular_file(outDir / "binary_only"));
    expect(!tests::fs::exists(outDir / "libbinary_only.a"));
  };

  "cabin build handles library-only packages"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "--lib", "widget" }, tmp.path).unwrap();
    const auto project = tmp.path / "widget";
    tests::fs::remove_all(project / "src");
    expect(!tests::fs::exists(project / "src"));

    const auto result = tests::runCabin({ "build" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();

    const auto outDir = project / "cabin-out" / "dev";
    expect(tests::fs::is_regular_file(outDir / "libwidget.a"));
    expect(!tests::fs::exists(outDir / "widget"));
  };
}
