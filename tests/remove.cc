#include "helpers.hpp"

#include <boost/ut.hpp>
#include <fmt/format.h>
#include <fstream>
#include <string>
#include <utility>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin remove"_test = [] {
    tests::TempDir tmp;
    tests::runCabin({ "new", "remove_test" }, tmp.path).unwrap();

    const auto project = tmp.path / "remove_test";
    const auto manifest = project / "cabin.toml";

    std::ofstream ofs(manifest, std::ios::app);
    ofs << "[dependencies]\n";
    ofs << "tbb = {}\n";
    ofs << "toml11 = {}\n";
    ofs.close();

    const auto result =
        tests::runCabin({ "remove", "tbb", "mydep", "toml11" }, project)
            .unwrap();

    expect(result.status.success());

    const auto manifestContent = tests::readFile(manifest);
    expect(manifestContent.find("tbb") == std::string::npos);
    expect(manifestContent.find("toml11") == std::string::npos);

    const auto manifestPath = tests::fs::weakly_canonical(manifest).string();
    auto sanitizedOut = tests::sanitizeOutput(result.out);
    expect(sanitizedOut.empty());
    auto sanitizedErr =
        tests::sanitizeOutput(result.err, { { manifestPath, "<MANIFEST>" } });
    const std::string expectedErr =
        "Warning: Dependency `mydep` not found in <MANIFEST>\n"
        "     Removed tbb, toml11 from <MANIFEST>\n";
    expect(sanitizedErr == expectedErr);
  };
}
