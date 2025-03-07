#include "Help.hpp"

#include "Cli.hpp"

namespace cabin {

static Result<void> helpMain(CliArgsView args) noexcept;

const Subcmd HELP_CMD =  //
    Subcmd{ "help" }
        .setDesc("Displays help for a cabin subcommand")
        .setArg(Arg{ "COMMAND" }.setRequired(false))
        .setMainFn(helpMain);

static Result<void>
helpMain(const CliArgsView args) noexcept {
  return getCli().printHelp(args);
}

}  // namespace cabin
