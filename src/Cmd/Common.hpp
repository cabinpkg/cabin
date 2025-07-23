#pragma once

#include "Cli.hpp"
#include "Parallelism.hpp"

namespace cabin {

inline constinit const Opt OPT_RELEASE =
    Opt{ "--release" }.setShort("-r").setDesc("Build with optimizations");

inline constinit const Opt OPT_BIN = Opt{ "--bin" }.setShort("-b").setDesc(
    "Use a binary (application) template [default]");
inline constinit const Opt OPT_LIB =
    Opt{ "--lib" }.setShort("-l").setDesc("Use a library template");

inline const Opt OPT_JOBS =
    Opt{ "--jobs" }
        .setShort("-j")
        .setDesc("Set the number of jobs to run in parallel")
        .setPlaceholder("<NUM>")
        .setDefault(NUM_DEFAULT_THREADS);

}  // namespace cabin
