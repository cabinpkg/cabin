#include "Help.hpp"

#include "../Cli.hpp"

#include <span>
#include <string_view>

static int helpMain(std::span<const std::string_view> args) noexcept;

const Subcmd HELP_CMD =  //
    Subcmd{ "help" }
        .setDesc("Displays help for a poac subcommand")
        .setArg(Arg{ "COMMAND" }.setRequired(false))
        .setMainFn(helpMain);

static int
helpMain(const std::span<const std::string_view> args) noexcept {
  return getCli().printHelp(args);
}
