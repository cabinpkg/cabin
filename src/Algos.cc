#include "Algos.hpp"

#include "Command.hpp"
#include "Logger.hpp"
#include "Rustify/Result.hpp"

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstdlib>
#include <optional>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

namespace cabin {

std::string
toUpper(const std::string_view str) noexcept {
  std::string res;
  for (const unsigned char c : str) {
    res += static_cast<char>(std::toupper(c));
  }
  return res;
}

std::string
toMacroName(const std::string_view name) noexcept {
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

std::string
replaceAll(
    std::string str, const std::string_view from, const std::string_view to
) noexcept {
  if (from.empty()) {
    return str;  // If the substring to replace is empty, return the original
                 // string
  }

  std::size_t startPos = 0;
  while ((startPos = str.find(from, startPos)) != std::string::npos) {
    str.replace(startPos, from.length(), to);
    startPos += to.length();  // Move past the last replaced substring
  }
  return str;
}

Result<int>
execCmd(const Command& cmd) noexcept {
  logger::debug("Running `{}`", cmd.toString());
  return Try(cmd.spawn()).wait();
}

Result<std::string>
getCmdOutput(const Command& cmd, const std::size_t retry) noexcept {
  logger::trace("Running `{}`", cmd.toString());

  int exitCode = EXIT_SUCCESS;
  std::string stdErr;
  int waitTime = 1;
  for (std::size_t i = 0; i < retry; ++i) {
    const auto cmdOut = Try(cmd.output());
    if (cmdOut.exitCode == EXIT_SUCCESS) {
      return Ok(cmdOut.stdOut);
    }
    exitCode = cmdOut.exitCode;
    stdErr = cmdOut.stdErr;

    // Sleep for an exponential backoff.
    std::this_thread::sleep_for(std::chrono::seconds(waitTime));
    waitTime *= 2;
  }

  return Result<std::string>(Err(Anyhow(
                                 "Command `{}` failed with exit code {}",
                                 cmd.toString(), exitCode
                             )))
      .with_context([stdErr = std::move(stdErr)] {
        return anyhow::anyhow(stdErr);
      });
}

bool
commandExists(const std::string_view cmd) noexcept {
  return Command("which")
      .addArg(cmd)
      .setStdOutConfig(Command::IOConfig::Null)
      .spawn()
      .and_then(&Child::wait)
      .map([](const int exitCode) { return exitCode == 0; })
      .unwrap_or(false);
}

// ref: https://wandbox.org/permlink/zRjT41alOHdwcf00
static std::size_t
levDistance(const std::string_view lhs, const std::string_view rhs) noexcept {
  const std::size_t lhsSize = lhs.size();
  const std::size_t rhsSize = rhs.size();

  // for all i and j, d[i,j] will hold the Levenshtein distance between the
  // first i characters of s and the first j characters of t
  std::vector<std::vector<std::size_t>> dist(
      lhsSize + 1, std::vector<std::size_t>(rhsSize + 1)
  );
  dist[0][0] = 0;

  // source prefixes can be transformed into empty string by dropping all
  // characters
  for (std::size_t i = 1; i <= lhsSize; ++i) {
    dist[i][0] = i;
  }

  // target prefixes can be reached from empty source prefix by inserting every
  // character
  for (std::size_t j = 1; j <= rhsSize; ++j) {
    dist[0][j] = j;
  }

  for (std::size_t i = 1; i <= lhsSize; ++i) {
    for (std::size_t j = 1; j <= rhsSize; ++j) {
      const std::size_t substCost = lhs[i - 1] == rhs[j - 1] ? 0 : 1;
      dist[i][j] = std::min({
          dist[i - 1][j] + 1,             // deletion
          dist[i][j - 1] + 1,             // insertion
          dist[i - 1][j - 1] + substCost  // substitution
      });
    }
  }

  return dist[lhsSize][rhsSize];
}

static bool
equalsInsensitive(
    const std::string_view lhs, const std::string_view rhs
) noexcept {
  return std::ranges::equal(lhs, rhs, [](char lhs, char rhs) {
    return std::tolower(lhs) == std::tolower(rhs);
  });
}

std::optional<std::string_view>
findSimilarStr(
    const std::string_view lhs, std::span<const std::string_view> candidates
) noexcept {
  // We need to check if `Candidates` has the exact case-insensitive string
  // because the Levenshtein distance match does not care about it.
  for (const std::string_view str : candidates) {
    if (equalsInsensitive(lhs, str)) {
      return str;
    }
  }

  // Keep going with the Levenshtein distance match.
  // If the LHS size is less than 3, use the LHS size minus 1 and if not,
  // use the LHS size divided by 3.
  const std::size_t length = lhs.size();
  const std::size_t maxDist = length < 3 ? length - 1 : length / 3;

  std::optional<std::pair<std::string_view, std::size_t>> similarStr =
      std::nullopt;
  for (const std::string_view str : candidates) {
    const std::size_t curDist = levDistance(lhs, str);
    if (curDist <= maxDist) {
      // The first similar string found || More similar string found
      if (!similarStr.has_value() || curDist < similarStr->second) {
        similarStr = { str, curDist };
      }
    }
  }

  if (similarStr.has_value()) {
    return similarStr->first;
  } else {
    return std::nullopt;
  }
}

}  // namespace cabin

#ifdef CABIN_TEST

#  include "Rustify/Aliases.hpp"
#  include "Rustify/Tests.hpp"

#  include <array>
#  include <limits>

namespace tests {

using namespace cabin;  // NOLINT(build/namespaces,google-build-using-namespace)

static void
testLevDistance() {
  // Test bytelength agnosticity
  for (char c = 0; c < std::numeric_limits<char>::max(); ++c) {
    const std::string str(1, c);
    assertEq(levDistance(str, str), 0UL);
  }

  pass();
}

static void
testLevDistance2() {
  constexpr std::string_view str1 = "\nMäry häd ä little lämb\n\nLittle lämb\n";
  constexpr std::string_view str2 = "\nMary häd ä little lämb\n\nLittle lämb\n";
  constexpr std::string_view str3 = "Mary häd ä little lämb\n\nLittle lämb\n";

  assertEq(levDistance(str1, str2), 2UL);
  assertEq(levDistance(str2, str1), 2UL);
  assertEq(levDistance(str1, str3), 3UL);
  assertEq(levDistance(str3, str1), 3UL);
  assertEq(levDistance(str2, str3), 1UL);
  assertEq(levDistance(str3, str2), 1UL);

  assertEq(levDistance("b", "bc"), 1UL);
  assertEq(levDistance("ab", "abc"), 1UL);
  assertEq(levDistance("aab", "aabc"), 1UL);
  assertEq(levDistance("aaab", "aaabc"), 1UL);

  assertEq(levDistance("a", "b"), 1UL);
  assertEq(levDistance("ab", "ac"), 1UL);
  assertEq(levDistance("aab", "aac"), 1UL);
  assertEq(levDistance("aaab", "aaac"), 1UL);

  pass();
}

// ref:
// https://github.com/llvm/llvm-project/commit/a247ba9d15635d96225ef39c8c150c08f492e70a#diff-fd993637669817b267190e7de029b75af5a0328d43d9b70c2e8dd512512091a2

static void
testFindSimilarStr() {
  constexpr std::array<std::string_view, 8> candidates{
    "if", "ifdef", "ifndef", "elif", "else", "endif", "elifdef", "elifndef"
  };

  assertEq(findSimilarStr("id", candidates), "if"sv);
  assertEq(findSimilarStr("ifd", candidates), "if"sv);
  assertEq(findSimilarStr("ifde", candidates), "ifdef"sv);
  assertEq(findSimilarStr("elf", candidates), "elif"sv);
  assertEq(findSimilarStr("elsif", candidates), "elif"sv);
  assertEq(findSimilarStr("elseif", candidates), "elif"sv);
  assertEq(findSimilarStr("elfidef", candidates), "elifdef"sv);
  assertEq(findSimilarStr("elfindef", candidates), "elifdef"sv);
  assertEq(findSimilarStr("elfinndef", candidates), "elifndef"sv);
  assertEq(findSimilarStr("els", candidates), "else"sv);
  assertEq(findSimilarStr("endi", candidates), "endif"sv);

  assertEq(findSimilarStr("i", candidates), std::nullopt);
  assertEq(
      findSimilarStr("special_compiler_directive", candidates), std::nullopt
  );

  pass();
}

static void
testFindSimilarStr2() {
  constexpr std::array<std::string_view, 2> candidates{ "aaab", "aaabc" };
  assertEq(findSimilarStr("aaaa", candidates), "aaab"sv);
  assertEq(findSimilarStr("1111111111", candidates), std::nullopt);

  constexpr std::array<std::string_view, 1> candidateS2{ "AAAA" };
  assertEq(findSimilarStr("aaaa", candidateS2), "AAAA"sv);

  pass();
}

}  // namespace tests

int
main() {
  tests::testLevDistance();
  tests::testLevDistance2();
  tests::testFindSimilarStr();
  tests::testFindSimilarStr2();
}

#endif
