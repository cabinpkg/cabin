#include "helpers.hpp"

#include <boost/ut.hpp>
#include <chrono>
#include <fmt/format.h>
#include <string>
#include <utility>

namespace {

std::size_t countFiles(const tests::fs::path& root,
                       std::string_view extension) {
  if (!tests::fs::exists(root)) {
    return 0;
  }
  std::size_t count = 0;
  for (const auto& entry : tests::fs::recursive_directory_iterator(root)) {
    if (entry.path().extension() == extension) {
      ++count;
    }
  }
  return count;
}

std::string expectedTestSummary(std::string_view projectName) {
  return fmt::format(
      "   Compiling {} v0.1.0 (<PROJECT>)\n"
      "    Finished `test` profile [unoptimized + debuginfo] target(s) in "
      "<DURATION>s\n"
      "     Running unit test src/main.cc (cabin-out/test/unit/main.cc.test)\n"
      "          Ok 1 passed; 0 failed; finished in <DURATION>s\n",
      projectName);
}

} // namespace

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin test basic"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "test_project" }, tmp.path).unwrap();

    const auto project = tmp.path / "test_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();
    tests::writeFile(project / "src/main.cc",
                     R"( #include <iostream>

#ifdef CABIN_TEST
void test_addition() {
  int result = 2 + 2;
  if (result != 4) {
    std::cerr << "Test failed: 2 + 2 = " << result << ", expected 4" << std::endl;
    std::exit(1);
  }
  std::cout << "test test addition ... ok" << std::endl;
}

int main() {
  test_addition();
  return 0;
}
#else
int main() {
  std::cout << "Hello, world!" << std::endl;
  return 0;
}
#endif
)");

    const auto result = tests::runCabin({ "test" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut == "test test addition ... ok\n");
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedErr == expectedTestSummary("test_project"));

    expect(tests::fs::is_directory(project / "cabin-out" / "test"));
    expect(tests::fs::is_directory(project / "cabin-out" / "test" / "unit"));
  };

  "cabin test help"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "test_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "test_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    const auto result = tests::runCabin({ "test", "--help" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut.contains("--coverage"));
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    expect(sanitizedErr.empty());
  };

  "cabin test coverage"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "coverage_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "coverage_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    tests::writeFile(project / "src/main.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
  std::cout << "test coverage function ... ok" << std::endl;
}

int main() {
  test_function();
  return 0;
}
#else
int main() {
  std::cout << "Hello, world!" << std::endl;
  return 0;
}
#endif
)");

    const auto result =
        tests::runCabin({ "test", "--coverage" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut == "test coverage function ... ok\n");
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedErr == expectedTestSummary("coverage_project"));

    const auto outDir = project / "cabin-out" / "test";
    expect(countFiles(outDir, ".gcda") > 0);
    expect(countFiles(outDir, ".gcno") > 0);
  };

  "cabin test verbose coverage"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "verbose_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "verbose_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    tests::writeFile(project / "src/main.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
int main() {
  std::cout << "test verbose compilation ... ok" << std::endl;
  return 0;
}
#else
int main() {
  std::cout << "Hello, world!" << std::endl;
  return 0;
}
#endif
)");

    tests::fs::remove_all(project / "cabin-out");

    const auto result =
        tests::runCabin({ "test", "--coverage", "-vv" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut.contains("--coverage"));
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedErr == expectedTestSummary("verbose_project"));
  };

  "cabin test without coverage"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "no_coverage_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "no_coverage_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    tests::writeFile(project / "src/main.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
int main() {
  std::cout << "test no coverage ... ok" << std::endl;
  return 0;
}
#else
int main() {
  std::cout << "Hello, world!" << std::endl;
  return 0;
}
#endif
)");

    const auto result = tests::runCabin({ "test" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut == "test no coverage ... ok\n");
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedErr == expectedTestSummary("no_coverage_project"));

    const auto outDir = project / "cabin-out" / "test";
    expect(countFiles(outDir, ".gcda") == 0u);
  };
}
