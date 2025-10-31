#include "helpers.hpp"

#include <boost/ut.hpp>
#include <filesystem>
#include <string>
#include <utility>

static bool hasClangFormat() { return tests::hasCommand("clang-format"); }

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "fmt without clang-format"_test = [] {
    if (hasClangFormat()) {
      expect(true) << "clang-format available";
      return;
    }

    const tests::TempDir tmp;
    const auto out = tests::runCabin({ "new", "pkg" }, tmp.path).unwrap();
    expect(out.status.success());

    const auto project = tmp.path / "pkg";
    const auto fmtResult = tests::runCabin({ "fmt" }, project).unwrap();
    expect(!fmtResult.status.success());
    auto sanitizedOut = tests::sanitizeOutput(fmtResult.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(fmtResult.err);
    const std::string expectedErr =
        "Error: fmt command requires clang-format; try installing it by:\n"
        "  apt/brew install clang-format\n";
    expect(sanitizedErr == expectedErr);
  };

  "fmt formats source"_test = [] {
    if (!hasClangFormat()) {
      expect(true) << "skipped: clang-format unavailable";
      return;
    }

    const tests::TempDir tmp;
    tests::runCabin({ "new", "pkg" }, tmp.path).unwrap();

    const auto project = tmp.path / "pkg";
    const auto mainFile = project / "src/main.cc";
    tests::writeFile(mainFile, "int main(){}\n");

    const auto before = tests::readFile(mainFile);
    const auto firstFmt = tests::runCabin({ "fmt" }, project).unwrap();
    expect(firstFmt.status.success()) << firstFmt.status.toString();
    auto sanitizedFirstOut = tests::sanitizeOutput(firstFmt.out);
    expect(sanitizedFirstOut.empty());
    auto sanitizedFirstErr = tests::sanitizeOutput(firstFmt.err);
    const std::string expectedFirstErr = "   Formatted 1 out of 2 files\n";
    expect(sanitizedFirstErr == expectedFirstErr);

    const auto afterFirst = tests::readFile(mainFile);
    expect(afterFirst != before) << "file should be reformatted";

    const auto secondFmt = tests::runCabin({ "fmt" }, project).unwrap();
    expect(secondFmt.status.success());
    auto sanitizedSecondOut = tests::sanitizeOutput(secondFmt.out);
    expect(sanitizedSecondOut.empty());
    auto sanitizedSecondErr = tests::sanitizeOutput(secondFmt.err);
    const std::string expectedSecondErr = "   Formatted 0 out of 2 files\n";
    expect(sanitizedSecondErr == expectedSecondErr);

    const auto afterSecond = tests::readFile(mainFile);
    expect(afterSecond == afterFirst);
  };

  "fmt without targets"_test = [] {
    if (!hasClangFormat()) {
      expect(true) << "skipped: clang-format unavailable";
      return;
    }

    const tests::TempDir tmp;
    tests::runCabin({ "new", "pkg" }, tmp.path).unwrap();

    const auto project = tmp.path / "pkg";
    tests::fs::remove(project / "src/main.cc");
    tests::fs::remove(project / "lib/lib.cc");

    const auto result = tests::runCabin({ "fmt" }, project).unwrap();
    expect(result.status.success());
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    const std::string expectedErr = "Warning: no files to format\n";
    expect(sanitizedErr == expectedErr);
  };

  "fmt missing manifest"_test = [] {
    if (!hasClangFormat()) {
      expect(true) << "skipped: clang-format unavailable";
      return;
    }

    const tests::TempDir tmp;
    tests::runCabin({ "new", "pkg" }, tmp.path).unwrap();

    const auto project = tests::fs::path(tmp.path) / "pkg";
    tests::fs::remove(project / "cabin.toml");

    const auto result = tests::runCabin({ "fmt" }, project).unwrap();
    expect(!result.status.success());

    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    const auto canonical = tests::fs::weakly_canonical(project);
    auto sanitizedErr = tests::sanitizeOutput(
        result.err, { { canonical.string(), "<PROJECT>" } });
    const std::string expectedErr =
        "Error: cabin.toml not find in `<PROJECT>` and its parents\n";
    expect(sanitizedErr == expectedErr);
  };
}
