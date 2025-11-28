#pragma once

#include "Command.hpp"

#include <algorithm>
#include <cstddef>
#include <optional>
#include <rs/result.hpp>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace cabin {

std::string toMacroName(std::string_view name) noexcept;
std::string replaceAll(std::string str, std::string_view from,
                       std::string_view to) noexcept;

Result<ExitStatus> execCmd(const Command& cmd) noexcept;
Result<std::string> getCmdOutput(const Command& cmd,
                                 std::size_t retry = 3) noexcept;
bool commandExists(std::string_view cmd) noexcept;

constexpr char toLower(char c) noexcept {
  return (c >= 'A' && c <= 'Z') ? static_cast<char>(c + ('a' - 'A')) : c;
}

// ref: https://wandbox.org/permlink/zRjT41alOHdwcf00
constexpr std::size_t levDistance(const std::string_view lhs,
                                  const std::string_view rhs) noexcept {
  const std::size_t lhsSize = lhs.size();
  const std::size_t rhsSize = rhs.size();

  // for all i and j, d[i,j] will hold the Levenshtein distance between the
  // first i characters of s and the first j characters of t
  std::vector<std::vector<std::size_t>> dist(
      lhsSize + 1, std::vector<std::size_t>(rhsSize + 1));
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
          dist[i - 1][j] + 1,            // deletion
          dist[i][j - 1] + 1,            // insertion
          dist[i - 1][j - 1] + substCost // substitution
      });
    }
  }

  return dist[lhsSize][rhsSize];
}

constexpr bool equalsInsensitive(const std::string_view lhs,
                                 const std::string_view rhs) noexcept {
  return std::ranges::equal(lhs, rhs, [](char lhs, char rhs) {
    return toLower(lhs) == toLower(rhs);
  });
}

// ref: https://reviews.llvm.org/differential/changeset/?ref=3315514
/// Find a similar string in `candidates`.
///
/// \param lhs a string for a similar string in `Candidates`
///
/// \param candidates the candidates to find a similar string.
///
/// \returns a similar string if exists. If no similar string exists,
/// returns std::nullopt.
constexpr std::optional<std::string_view>
findSimilarStr(std::string_view lhs,
               std::span<const std::string_view> candidates) noexcept {
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

} // namespace cabin
