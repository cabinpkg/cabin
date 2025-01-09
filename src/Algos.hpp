#pragma once

#include "Command.hpp"

#include <cstddef>
#include <optional>
#include <span>
#include <string>
#include <string_view>

namespace cabin {

std::string toUpper(std::string_view str) noexcept;
std::string toMacroName(std::string_view name) noexcept;
std::string
replaceAll(std::string str, std::string_view from, std::string_view to);

int execCmd(const Command& cmd) noexcept;
std::string getCmdOutput(const Command& cmd, std::size_t retry = 3);
bool commandExists(std::string_view cmd) noexcept;

// ref: https://reviews.llvm.org/differential/changeset/?ref=3315514
/// Find a similar string in `candidates`.
///
/// \param lhs a string for a similar string in `Candidates`
///
/// \param candidates the candidates to find a similar string.
///
/// \returns a similar string if exists. If no similar string exists,
/// returns std::nullopt.
std::optional<std::string_view> findSimilarStr(
    std::string_view lhs, std::span<const std::string_view> candidates
) noexcept;

}  // namespace cabin
