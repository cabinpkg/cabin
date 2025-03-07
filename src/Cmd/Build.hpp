#pragma once

#include "../Builder/BuildProfile.hpp"
#include "../Cli.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

#include <string>

namespace cabin {

extern const Subcmd BUILD_CMD;
Result<void> buildImpl(
    const Manifest& manifest, std::string& outDir, const BuildProfile& profile
);

}  // namespace cabin
