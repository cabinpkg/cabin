#include "helpers.hpp"

#include <boost/ut.hpp>
#include <regex>
#include <string>
#include <utility>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin run"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "hello_world" }, tmp.path).unwrap();

    const auto project = tmp.path / "hello_world";
    const auto result = tests::runCabin({ "run" }, project).unwrap();

    expect(result.status.success()) << result.status.toString();
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut == "Hello, world!\n");
    const auto projectPath = tests::fs::weakly_canonical(project).string();
    auto sanitizedErr =
        tests::sanitizeOutput(result.err, { { projectPath, "<PROJECT>" } });
    const std::string expectedErr =
        "   Compiling hello_world v0.1.0 (<PROJECT>)\n"
        "    Finished `dev` profile [unoptimized + debuginfo] target(s) in "
        "<DURATION>s\n"
        "     Running `cabin-out/dev/hello_world`\n";
    expect(sanitizedErr == expectedErr);

    expect(tests::fs::is_directory(project / "cabin-out"));
    expect(tests::fs::is_directory(project / "cabin-out/dev"));
    expect(tests::fs::is_regular_file(project / "cabin-out/dev/hello_world"));

    expect(result.err.contains("Compiling hello_world v0.1.0"));
    expect(result.err.contains("Finished `dev` profile"));
    expect(result.err.contains("Running `cabin-out/dev/hello_world`"));
  };
}
