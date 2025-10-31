#pragma once

#include "Cli.hpp"
#include "Rustify/Result.hpp"

#include <filesystem>
#include <string>
#include <string_view>

namespace cabin {

extern const Subcmd NEW_CMD;
std::string createCabinToml(std::string_view projectName) noexcept;
Result<void> createProjectFiles(bool isBin, const std::filesystem::path& root,
                                std::string_view projectName,
                                bool skipExisting = false);

} // namespace cabin
