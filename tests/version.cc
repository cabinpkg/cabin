#include "Git2.hpp"
#include "Manifest.hpp"
#include "helpers.hpp"

#include <boost/ut.hpp>
#include <fmt/format.h>
#include <regex>
#include <string>

static std::string readVersion() {
  auto manifest = cabin::Manifest::tryParse().unwrap();
  return manifest.package.version.toString();
}

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin version"_test = [] {
    const auto version = readVersion();
    expect(!version.empty());

    auto trim = [](std::string s) {
      while (!s.empty() && (s.back() == '\n' || s.back() == '\r')) {
        s.pop_back();
      }
      return s;
    };

    const auto result = tests::runCabin({ "version" }).unwrap();
    expect(result.status.success());
    const auto actual = trim(result.out);
    static const std::regex pattern(
        R"(^cabin ([^\s]+) \(([0-9a-f]{8}) ([0-9]{4}-[0-9]{2}-[0-9]{2})\)$)");
    std::smatch match;
    expect(std::regex_match(actual, match, pattern));
    expect(match[1].str() == version);

    auto sanitizedOut = tests::sanitizeOutput(result.out);
    const std::string expectedOut =
        fmt::format("cabin {} (<SHORT_HASH> <DATE>)\n", version);
    expect(sanitizedOut == expectedOut);
    auto sanitizedErr = tests::sanitizeOutput(result.err);
    expect(sanitizedErr.empty());
  };
}
