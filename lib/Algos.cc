#include "Algos.hpp"

#include "Command.hpp"

#include <cctype>
#include <chrono>
#include <cstdlib>
#include <optional>
#include <rs/result.hpp>
#include <spdlog/spdlog.h>
#include <string>
#include <string_view>
#include <thread>
#include <utility>

namespace cabin {

std::string toMacroName(const std::string_view name) noexcept {
  std::string macroName;
  for (const unsigned char c : name) {
    if (std::isalpha(c)) {
      macroName += static_cast<char>(std::toupper(c));
    } else if (std::isdigit(c)) {
      macroName += static_cast<char>(c);
    } else {
      macroName += '_';
    }
  }
  return macroName;
}

std::string replaceAll(std::string str, const std::string_view from,
                       const std::string_view to) noexcept {
  if (from.empty()) {
    return str; // If the substring to replace is empty, return the original
                // string
  }

  std::size_t startPos = 0;
  while ((startPos = str.find(from, startPos)) != std::string::npos) {
    str.replace(startPos, from.length(), to);
    startPos += to.length(); // Move past the last replaced substring
  }
  return str;
}

rs::Result<ExitStatus> execCmd(const Command& cmd) noexcept {
  spdlog::debug("Running `{}`", cmd.toString());
  return rs_try(cmd.spawn()).wait();
}

rs::Result<std::string> getCmdOutput(const Command& cmd,
                                     const std::size_t retry) noexcept {
  spdlog::trace("Running `{}`", cmd.toString());

  ExitStatus exitStatus;
  std::string stdErr;
  int waitTime = 1;
  for (std::size_t i = 0; i < retry; ++i) {
    const auto cmdOut = rs_try(cmd.output());
    if (cmdOut.exitStatus.success()) {
      return rs::Ok(cmdOut.stdOut);
    }
    exitStatus = cmdOut.exitStatus;
    stdErr = cmdOut.stdErr;

    // Sleep for an exponential backoff.
    std::this_thread::sleep_for(std::chrono::seconds(waitTime));
    waitTime *= 2;
  }

  return rs::Result<std::string>(
             rs::Err(rs::anyhow("Command `{}` {}", cmd.toString(), exitStatus)))
      .with_context(
          [stdErr = std::move(stdErr)] { return rs::anyhow(stdErr); });
}

bool commandExists(const std::string_view cmd) noexcept {
  return Command("which")
      .addArg(cmd)
      .setStdOutConfig(Command::IOConfig::Null)
      .spawn()
      .and_then(&Child::wait)
      .map(&ExitStatus::success)
      .unwrap_or(false);
}

} // namespace cabin

#ifdef CABIN_TEST

#  include <array>
#  include <limits>
#  include <rs/tests.hpp>

using namespace cabin; // NOLINT(build/namespaces,google-build-using-namespace)
using std::string_view_literals::operator""sv;

static void testToLower() {
  static_assert(toLower('A') == 'a');
  static_assert(toLower('Z') == 'z');
  static_assert(toLower('M') == 'm');

  static_assert(toLower('a') == 'a');
  static_assert(toLower('z') == 'z');

  static_assert(toLower('@') == '@');
  static_assert(toLower('[') == '[');

  static_assert(toLower('0') == '0');
  static_assert(toLower('9') == '9');

  static_assert(toLower('!') == '!');
  static_assert(toLower(' ') == ' ');
  static_assert(toLower('~') == '~');

  rs::pass();
}

static void testLevDistance() {
  // Test bytelength agnosticity
  for (char c = 0; c < std::numeric_limits<char>::max(); ++c) {
    const std::string str(1, c);
    rs::assertEq(levDistance(str, str), 0UL);
  }

  rs::pass();
}

static void testLevDistance2() {
  constexpr std::string_view str1 = "\nMäry häd ä little lämb\n\nLittle lämb\n";
  constexpr std::string_view str2 = "\nMary häd ä little lämb\n\nLittle lämb\n";
  constexpr std::string_view str3 = "Mary häd ä little lämb\n\nLittle lämb\n";

  static_assert(levDistance(str1, str2) == 2UL);
  static_assert(levDistance(str2, str1) == 2UL);
  static_assert(levDistance(str1, str3) == 3UL);
  static_assert(levDistance(str3, str1) == 3UL);
  static_assert(levDistance(str2, str3) == 1UL);
  static_assert(levDistance(str3, str2) == 1UL);

  static_assert(levDistance("b", "bc") == 1UL);
  static_assert(levDistance("ab", "abc") == 1UL);
  static_assert(levDistance("aab", "aabc") == 1UL);
  static_assert(levDistance("aaab", "aaabc") == 1UL);

  static_assert(levDistance("a", "b") == 1UL);
  static_assert(levDistance("ab", "ac") == 1UL);
  static_assert(levDistance("aab", "aac") == 1UL);
  static_assert(levDistance("aaab", "aaac") == 1UL);

  rs::pass();
}

// ref:
// https://github.com/llvm/llvm-project/commit/a247ba9d15635d96225ef39c8c150c08f492e70a#diff-fd993637669817b267190e7de029b75af5a0328d43d9b70c2e8dd512512091a2

static void testFindSimilarStr() {
  constexpr std::array<std::string_view, 8> candidates{
    "if", "ifdef", "ifndef", "elif", "else", "endif", "elifdef", "elifndef"
  };

  static_assert(findSimilarStr("id", candidates) == "if"sv);
  static_assert(findSimilarStr("ifd", candidates) == "if"sv);
  static_assert(findSimilarStr("ifde", candidates) == "ifdef"sv);
  static_assert(findSimilarStr("elf", candidates) == "elif"sv);
  static_assert(findSimilarStr("elsif", candidates) == "elif"sv);
  static_assert(findSimilarStr("elseif", candidates) == "elif"sv);
  static_assert(findSimilarStr("elfidef", candidates) == "elifdef"sv);
  static_assert(findSimilarStr("elfindef", candidates) == "elifdef"sv);
  static_assert(findSimilarStr("elfinndef", candidates) == "elifndef"sv);
  static_assert(findSimilarStr("els", candidates) == "else"sv);
  static_assert(findSimilarStr("endi", candidates) == "endif"sv);

  static_assert(!findSimilarStr("i", candidates).has_value());
  static_assert(
      !findSimilarStr("special_compiler_directive", candidates).has_value());

  rs::pass();
}

static void testFindSimilarStr2() {
  constexpr std::array<std::string_view, 2> candidates{ "aaab", "aaabc" };
  static_assert(findSimilarStr("aaaa", candidates) == "aaab"sv);
  static_assert(!findSimilarStr("1111111111", candidates).has_value());

  constexpr std::array<std::string_view, 1> candidates2{ "AAAA" };
  static_assert(findSimilarStr("aaaa", candidates2) == "AAAA"sv);

  rs::pass();
}

int main() {
  testToLower();
  testLevDistance();
  testLevDistance2();
  testFindSimilarStr();
  testFindSimilarStr2();
}

#endif
