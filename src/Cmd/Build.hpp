#pragma once

#include "../Cli.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

#include <string>

namespace cabin {

extern const Subcmd BUILD_CMD;
Result<void>
buildImpl(const Manifest& manifest, std::string& outDir, bool isDebug);

}  // namespace cabin
