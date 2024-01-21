#include "Help.hpp"

#include "Global.hpp"

static constexpr auto helpCli = Subcommand<1>("help")
                                    .setDesc(helpDesc)
                                    .setUsage("[OPTIONS] [COMMAND]")
                                    .setArgs("[COMMAND]");

void
helpHelp() noexcept {
  helpCli.printHelp();
}
