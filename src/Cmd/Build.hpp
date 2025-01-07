#pragma once

#include "../Cli.hpp"
#include "../Manifest.hpp"

#include <string>

extern const Subcmd BUILD_CMD;
int buildImpl(const Manifest& manifest, std::string& outDir, bool isDebug);
