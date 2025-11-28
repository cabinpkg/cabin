#pragma once

#include "Cli.hpp"

#include <filesystem>
#include <rs/result.hpp>
#include <string>
#include <string_view>

namespace cabin {

extern const Subcmd NEW_CMD;
std::string createCabinToml(std::string_view projectName) noexcept;
rs::Result<void> createProjectFiles(bool isBin,
                                    const std::filesystem::path& root,
                                    std::string_view projectName,
                                    bool skipExisting = false);

} // namespace cabin
