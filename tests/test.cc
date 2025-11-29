#include "helpers.hpp"

#include <boost/ut.hpp>
#include <chrono>
#include <fmt/format.h>
#include <string>
#include <utility>
#include <vector>

static std::size_t countFiles(const tests::fs::path& root,
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

struct TestInfo {
  std::string projectName;
  std::vector<std::string> testTargets;
  bool hasLib = false;

  std::size_t numPassed = 0;
  std::size_t numFailed = 0;
  std::size_t numFiltered = 0;
};

static std::string expectedTestSummary(const TestInfo& testInfo) {
  std::string summary = "   Analyzing project dependencies...\n";
  if (testInfo.hasLib) {
    summary += fmt::format("   Compiling {}(lib) v0.1.0 (<PROJECT>)\n",
                           testInfo.projectName);
  }
  summary += fmt::format(
      "   Compiling {}(test) v0.1.0 (<PROJECT>)\n"
      "    Finished `test` profile [unoptimized + debuginfo] target(s) in "
      "<DURATION>s\n",
      testInfo.projectName);

  for (std::string_view testTarget : testInfo.testTargets) {
    summary += fmt::format("     Running unit test src/{0}.cc "
                           "(cabin-out/test/unit/src/{0}.cc.test)\n",
                           testTarget);
  }

  summary += fmt::format(
      "          Ok {} passed; {} failed; {} filtered out; finished in "
      "<DURATION>s\n",
      testInfo.numPassed, testInfo.numFailed, testInfo.numFiltered);

  return summary;
}

static std::string expectedTestSummary(std::string_view projectName,
                                       bool hasLib) {
  std::string summary = "   Analyzing project dependencies...\n";
  if (hasLib) {
    summary +=
        fmt::format("   Compiling {}(lib) v0.1.0 (<PROJECT>)\n", projectName);
  }
  summary +=
      fmt::format("   Compiling {}(test) v0.1.0 (<PROJECT>)\n", projectName);
  summary +=
      "    Finished `test` profile [unoptimized + debuginfo] target(s) in "
      "<DURATION>s\n"
      "     Running unit test src/main.cc "
      "(cabin-out/test/unit/src/main.cc.test)\n"
      "          Ok 1 passed; 0 failed; 0 filtered out; finished in "
      "<DURATION>s\n";
  return summary;
}

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin test basic"_test = [] {
    const tests::TempDir tmp;
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
    expect(sanitizedErr == expectedTestSummary("test_project", false));

    expect(tests::fs::is_directory(project / "cabin-out" / "test"));
    expect(tests::fs::is_directory(project / "cabin-out" / "test" / "unit"));
  };

  "cabin test help"_test = [] {
    const tests::TempDir tmp;
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
    const tests::TempDir tmp;
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
    expect(sanitizedErr == expectedTestSummary("coverage_project", false));

    const auto outDir = project / "cabin-out" / "test";
    expect(countFiles(outDir, ".gcda") > 0);
    expect(countFiles(outDir, ".gcno") > 0);
  };

  "cabin test verbose coverage"_test = [] {
    const tests::TempDir tmp;
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
    expect(sanitizedErr == expectedTestSummary("verbose_project", false));
  };

  "cabin test without coverage"_test = [] {
    const tests::TempDir tmp;
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
    expect(sanitizedErr == expectedTestSummary("no_coverage_project", false));

    const auto outDir = project / "cabin-out" / "test";
    expect(countFiles(outDir, ".gcda") == 0U);
  };

  "cabin test integration without lib"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "bin_integration" }, tmp.path).unwrap();
    const auto project = tmp.path / "bin_integration";
    tests::fs::remove_all(project / "lib");
    const auto testsDir = project / "tests";
    tests::fs::create_directories(testsDir);
    tests::writeFile(testsDir / "smoke.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
int main() {
  std::cout << "integration smoke ... ok" << std::endl;
  return 0;
}
#else
int main() { return 0; }
#endif
)");

    const auto result = tests::runCabin({ "test" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();
    const auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.contains("integration smoke ... ok"));
    const auto testBinary = project / "cabin-out" / "test" / "intg" / "smoke";
    expect(tests::fs::is_regular_file(testBinary));
  };

  "cabin test library-only"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "--lib", "lib_only" }, tmp.path).unwrap();
    const auto project = tmp.path / "lib_only";
    tests::fs::remove_all(project / "src");
    tests::writeFile(project / "lib" / "lib_only.cc",
                     R"(int libFunction() { return 1; }

#ifdef CABIN_TEST
int main() {
  return libFunction() == 1 ? 0 : 1;
}
#endif
)");

    const auto result = tests::runCabin({ "test" }, project).unwrap();
    expect(result.status.success()) << result.status.toString();
    const auto outDir = project / "cabin-out" / "test" / "unit" / "lib";
    expect(tests::fs::is_regular_file(outDir / "lib_only.cc.test"));
  };

  "cabin test testname filters single test"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "testname_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "testname_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    tests::writeFile(project / "src/main.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
  std::cout << "main test function ... ok" << std::endl;
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

    tests::writeFile(project / "src/Testname.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
  std::cout << "testname test function ... ok" << std::endl;
}

int main() {
  test_function();
  return 0;
}
#endif
)");

    const auto result =
        tests::runCabin({ "test", "Testname" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut == "testname test function ... ok\n");
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT

    expect(sanitizedErr
           == expectedTestSummary(TestInfo{ .projectName = "testname_project",
                                            .testTargets = { "Testname" },
                                            .numPassed = 1,
                                            .numFailed = 0,
                                            .numFiltered = 1 }));
  };

  "cabin test testname filters multiple tests"_test = [] {
    const tests::TempDir tmp;
    tests::runCabin({ "new", "testname_project" }, tmp.path).unwrap();
    const auto project = tmp.path / "testname_project";
    const auto projectPath = tests::fs::weakly_canonical(project).string();

    tests::writeFile(project / "src/main.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
  std::cout << "main test function ... ok" << std::endl;
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

    tests::writeFile(project / "src/TestnameFirst.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
  std::cout << "testname first function ... ok" << std::endl;
}

int main() {
  test_function();
  return 0;
}
#endif
)");

    tests::writeFile(project / "src/TestnameSecond.cc",
                     R"(#include <iostream>

#ifdef CABIN_TEST
void test_function() {
std::cout << "testname second function ... ok" << std::endl;
}

int main() {
test_function();
return 0;
}
#endif
)");

    const auto result =
        tests::runCabin({ "test", "Testname" }, project).unwrap();
    expect(result.status.success());

    auto sanitizedOut = tests::sanitizeOutput(
        result.out, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedOut
           == "testname first function ... ok\n"
              "testname second function ... ok\n");

    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { projectPath, "<PROJECT>" } }); // NOLINT
    expect(sanitizedErr
           == expectedTestSummary(
               TestInfo{ .projectName = "testname_project",
                         .testTargets = { "TestnameFirst", "TestnameSecond" },
                         .numPassed = 2,
                         .numFailed = 0,
                         .numFiltered = 1 }));
  };
}
