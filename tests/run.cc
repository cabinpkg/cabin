#include "helpers.hpp"

#include <boost/ut.hpp>
#include <regex>
#include <string>
#include <utility>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin run"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "hello_world" }, tmp.path).unwrap();

    const auto project = tmp.path / "hello_world";
    const auto result = tests::runCabin({ "run" }, project).unwrap();

    expect(result.status.success()) << result.status.toString();
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut == "Hello, world!\n") << sanitizedOut;
    const auto projectPath = tests::fs::weakly_canonical(project).string();
    auto sanitizedErr =
        tests::sanitizeOutput(result.err, { { projectPath, "<PROJECT>" } });
    const std::string analyzing = "   Analyzing project dependencies...\n";
    const std::string binErr = "   Compiling hello_world v0.1.0 (<PROJECT>)\n";
    const std::string libErr =
        "   Compiling hello_world(lib) v0.1.0 (<PROJECT>)\n";
    const std::string tailErr =
        "    Finished `dev` profile [unoptimized + debuginfo] target(s) in "
        "<DURATION>s\n"
        "     Running `cabin-out/dev/hello_world`\n";

    const bool errMatchesWithLib =
        sanitizedErr == analyzing + binErr + libErr + tailErr;
    const bool errMatchesWithLibFirst =
        sanitizedErr == analyzing + libErr + binErr + tailErr;
    const bool errMatchesWithoutLib = sanitizedErr == analyzing + tailErr;
    expect(errMatchesWithLib || errMatchesWithLibFirst || errMatchesWithoutLib)
        << sanitizedErr;

    expect(tests::fs::is_directory(project / "cabin-out"));
    expect(tests::fs::is_directory(project / "cabin-out/dev"));
    expect(tests::fs::is_regular_file(project / "cabin-out/dev/hello_world"));

    expect(result.err.contains("Compiling hello_world v0.1.0"));
    expect(result.err.contains("Finished `dev` profile"));
    expect(result.err.contains("Running `cabin-out/dev/hello_world`"));
  };
}
